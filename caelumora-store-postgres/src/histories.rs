//! The histories of the channels (issue 379), as the Postgres store keeps
//! them: per channel a chain in `history_entries`, its head and who was a
//! member when (`Memberships`, issue 417) in `histories`, and the items as
//! they stand in `history_items`, found by the message each said. A put
//! writes the chain and derives the items and the memberships from what it
//! added, in its transaction: the items it changes - every one, for a
//! fitting - are read, taken further by the model's rule (`StandingItems`)
//! and written back. What a reader sees of them is judged by the model, too
//! (`seen`); a filter of a page is asked of the database besides, as far as
//! SQL can judge it (`Condition`).

use crate::chains::{failed, position, seq, Chains, Entry, BATCH};
use crate::connection::sql;
use crate::filter::Condition;
use crate::PostgresStore;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::chain::{Node, Position};
use caelumora_model::channel::ChannelId;
use caelumora_model::history::{History, HistoryEntry, HistoryPage, Reader, Recorded};
use caelumora_model::message::{Message, MessageId};
use caelumora_model::store::history::{answer_of, page_of, seen, seen_nodes, HistoryField, HistoryStore, Memberships, StandingItems};
use caelumora_model::store::query::Filter;
use caelumora_model::store::{StoreError, StoreResult};
use serde_json::Value;
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use std::ops::Range;

/// The chains of the histories, by channel.
const HISTORIES: Chains<Recorded> = Chains::new("history_entries", "histories", &["channel_id"], false);

/// The tables of the histories, each by channel.
pub(crate) const TABLES: &[&str] = &["histories", "history_entries", "history_items"];

/// A history is not searched by its chain, but by its items.
impl Entry for Recorded {
    fn about(&self) -> Option<MessageId> {
        None
    }
}

/// The key of the history of `channel`, and how it is named where
/// something about it fails.
fn key(channel: ChannelId) -> ([String; 1], String) {
    ([channel.to_string()], format!("the history of channel {channel}"))
}

/// An item as its row holds it.
type ItemRow = (i64, Json<Recorded>);

fn item((seq, Json(item)): ItemRow) -> (Position, Recorded) {
    (position(seq), item)
}

/// Writes what `history`, that of `channel` as its state holds it, added
/// since it was stored, and derives the items and the memberships of what
/// it added. Marks nothing: that is for when the transaction is committed.
pub(crate) async fn append(conn: &mut PgConnection, channel: ChannelId, history: &History) -> StoreResult<()> {
    let Some(changes) = history.changes() else { return Ok(()) };
    let (key, name) = key(channel);
    let added = HISTORIES.append(conn, &key, &name, changes).await?;
    if added.is_empty() {
        return Ok(());
    }
    let mut items = StandingItems::from(changed_by(conn, &key, &name, &added).await?);
    let mut memberships = read_memberships(conn, &key, &name).await?;
    for node in &added {
        items.take(node);
        memberships.take(node);
    }
    write_items(conn, &key, &name, items.into()).await?;
    write_memberships(conn, &key, &name, &memberships).await
}

/// The history of `channel` from the position `from` on, as its state
/// holds it; empty if none is kept.
pub(crate) async fn load(conn: &mut PgConnection, channel: ChannelId, from: Position) -> StoreResult<History> {
    let (key, name) = key(channel);
    HISTORIES.load(conn, &key, &name, from).await
}

/// The items kept that the entries `added` change, oldest first: every
/// one if one of them fits the history (`HistoryEntry::Fitted`), else those
/// of the messages they change - said before them, or with them, then not
/// kept yet.
async fn changed_by(conn: &mut PgConnection, key: &[String; 1], name: &str, added: &[Node<Recorded>]) -> StoreResult<Vec<(Position, Recorded)>> {
    let read: Vec<ItemRow> = match added.iter().any(|node| matches!(node.entry.entry, HistoryEntry::Fitted { .. })) {
        true => sqlx::query_as("SELECT seq, item FROM history_items WHERE channel_id = $1 ORDER BY seq")
            .bind(&key[0]).fetch_all(&mut *conn).await.map_err(failed(name))?,
        false => {
            let changed: Vec<String> = added.iter().filter_map(|node| node.entry.entry.changed()).map(|id| id.to_string()).collect();
            if changed.is_empty() {
                return Ok(Vec::new());
            }
            sqlx::query_as("SELECT seq, item FROM history_items WHERE channel_id = $1 AND message_id = ANY($2) ORDER BY seq")
                .bind(&key[0]).bind(changed).fetch_all(&mut *conn).await.map_err(failed(name))?
        }
    };
    Ok(read.into_iter().map(item).collect())
}

/// Writes `items` in their places, over what is there.
async fn write_items(conn: &mut PgConnection, key: &[String; 1], name: &str, items: Vec<(Position, Recorded)>) -> StoreResult<()> {
    if items.is_empty() {
        return Ok(());
    }
    let seqs: Vec<i64> = items.iter().map(|(at, _)| seq(*at)).collect();
    let messages: Vec<Option<String>> = items.iter().map(|(_, item)| item.entry.said().map(|message| message.id.to_string())).collect();
    let rows = items.iter().map(|(_, item)| serde_json::to_value(item)).collect::<Result<Vec<Value>, _>>()
        .map_err(|e| StoreError::Backend(format!("the items of {name} cannot be written: {e}")))?;
    let upsert = "INSERT INTO history_items (channel_id, seq, message_id, item) \
        SELECT $1, seq, message_id, item FROM UNNEST($2::bigint[], $3::text[], $4::jsonb[]) AS given(seq, message_id, item) \
        ON CONFLICT (channel_id, seq) DO UPDATE SET message_id = EXCLUDED.message_id, item = EXCLUDED.item";
    sqlx::query(upsert).bind(&key[0]).bind(seqs).bind(messages).bind(rows).execute(&mut *conn).await.map_err(failed(name))?;
    Ok(())
}

/// Who was a member when, as far as the chain was taken in; nobody, if no
/// entry was.
async fn read_memberships(conn: &mut PgConnection, key: &[String; 1], name: &str) -> StoreResult<Memberships> {
    let read: Option<Option<Json<Memberships>>> = sqlx::query_scalar("SELECT memberships FROM histories WHERE channel_id = $1")
        .bind(&key[0]).fetch_optional(&mut *conn).await.map_err(failed(name))?;
    Ok(read.flatten().map(|Json(memberships)| memberships).unwrap_or_default())
}

/// Writes `memberships` beside the head, which the chain's put wrote.
async fn write_memberships(conn: &mut PgConnection, key: &[String; 1], name: &str, memberships: &Memberships) -> StoreResult<()> {
    sqlx::query("UPDATE histories SET memberships = $2 WHERE channel_id = $1")
        .bind(&key[0]).bind(Json(memberships)).execute(&mut *conn).await.map_err(failed(name))?;
    Ok(())
}

#[async_trait]
impl HistoryStore for PostgresStore {
    /// Reads the items backwards in batches of those the filter may meet,
    /// until the page is filled.
    async fn page(&self, channel: ChannelId, reader: &Reader, from: Position, before: Option<Position>, limit: usize, filter: Option<&Filter<HistoryField>>) -> StoreResult<HistoryPage> {
        let mut conn = self.connection.acquire().await?;
        let (key, name) = key(channel);
        let head = HISTORIES.head(&mut conn, &key, &name).await?;
        let ranges = read_memberships(&mut conn, &key, &name).await?.of_reader(reader);
        let condition = filter.map(|filter| Condition::of(&filter.by_path(), "item", 5));
        let meeting = condition.as_ref().map_or("true", |condition| condition.sql.as_str());
        let statement = format!("SELECT seq, item FROM history_items WHERE channel_id = $1 AND seq >= $2 AND seq < $3 AND {meeting} ORDER BY seq DESC LIMIT $4");
        let mut before = before.unwrap_or(Position::MAX);
        let mut newest = Vec::new();
        loop {
            let query = sqlx::query_as(sql(statement.clone())).bind(&key[0]).bind(seq(from)).bind(seq(before)).bind(BATCH);
            let query = match &condition {
                Some(condition) => condition.bound(query),
                None => query,
            };
            let read: Vec<ItemRow> = query.fetch_all(&mut *conn).await.map_err(failed(&name))?;
            let whole = i64::try_from(read.len()).is_ok_and(|count| count == BATCH);
            for row in read {
                let item = item(row);
                before = item.0;
                newest.extend(seen(reader, &ranges, filter, &item)?);
            }
            if newest.len() > limit || !whole {
                break;
            }
        }
        page_of(newest.into_iter().map(|item| Ok(Some(item))), from, limit, head)
    }

    async fn said(&self, channel: ChannelId, reader: &Reader, id: MessageId) -> StoreResult<Option<(Position, HistoryEntry)>> {
        let mut conn = self.connection.acquire().await?;
        let (key, name) = key(channel);
        let found: Option<ItemRow> = sqlx::query_as("SELECT seq, item FROM history_items WHERE channel_id = $1 AND message_id = $2")
            .bind(&key[0]).bind(id.to_string()).fetch_optional(&mut *conn).await.map_err(failed(&name))?;
        let Some(found) = found else { return Ok(None) };
        let ranges = read_memberships(&mut conn, &key, &name).await?.of_reader(reader);
        seen(reader, &ranges, None, &item(found))
    }

    async fn entries(&self, channel: ChannelId, reader: &Reader, range: Range<Position>) -> StoreResult<Vec<Node<Recorded>>> {
        let mut conn = self.connection.acquire().await?;
        let (key, name) = key(channel);
        let nodes = HISTORIES.nodes(&mut conn, &key, &name, range, false, None).await?;
        let ranges = read_memberships(&mut conn, &key, &name).await?.of_reader(reader);
        Ok(seen_nodes(reader, &ranges, &nodes))
    }

    /// Reads the chain backwards in batches, until one holds the end of the
    /// turn before.
    async fn answer(&self, channel: ChannelId, reader: &Reader, end: Position) -> StoreResult<Vec<Message>> {
        let mut conn = self.connection.acquire().await?;
        let (key, name) = key(channel);
        let Some(ended) = HISTORIES.nodes(&mut conn, &key, &name, end..end.next(), false, None).await?.pop() else { return Ok(Vec::new()) };
        let HistoryEntry::TurnFinished { actor, .. } = &ended.entry.entry else { return Ok(Vec::new()) };
        let mut older: Vec<Node<Recorded>> = Vec::new();
        let mut before = end;
        loop {
            let read = HISTORIES.nodes(&mut conn, &key, &name, Position::START..before, true, Some(BATCH)).await?;
            let Some(oldest) = read.last() else { break };
            before = oldest.seq;
            let reached = read.iter().any(|node| node.entry.entry.ends_turn_of(*actor));
            older.extend(read);
            if reached {
                break;
            }
        }
        let ranges = read_memberships(&mut conn, &key, &name).await?.of_reader(reader);
        Ok(answer_of(&ended.entry.entry, &older, reader, &ranges))
    }

    async fn memberships(&self, channel: ChannelId, actor: ActorId) -> StoreResult<Vec<Range<Position>>> {
        let mut conn = self.connection.acquire().await?;
        let (key, name) = key(channel);
        Ok(read_memberships(&mut conn, &key, &name).await?.of_member(actor))
    }

    /// In one transaction: the items and the memberships of the whole chain.
    async fn rebuild(&self, channel: ChannelId) -> StoreResult<()> {
        let mut tx = self.connection.begin().await?;
        let (key, name) = key(channel);
        let nodes = HISTORIES.nodes(&mut tx, &key, &name, Position::START..Position::MAX, false, None).await?;
        sqlx::query("DELETE FROM history_items WHERE channel_id = $1").bind(&key[0]).execute(&mut *tx).await.map_err(failed(&name))?;
        write_items(&mut tx, &key, &name, StandingItems::of(&nodes).into()).await?;
        write_memberships(&mut tx, &key, &name, &Memberships::of(&nodes)).await?;
        tx.commit().await.map_err(failed(&name))
    }
}
