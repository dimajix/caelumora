//! A chain a state holds, as the Postgres store keeps it (issues 379, 380):
//! a row per node in the table of its kind, by its position (`seq`) - every
//! branch, a node changed in its place written over -, and its head in a
//! table of its own. A put writes both in the transaction of the state, so
//! that no node is there past what its head was written with.

use crate::connection::sql;
use caelumora_model::chain::{Chain, Node, Position, Stored};
use caelumora_model::digest::Digest;
use caelumora_model::message::MessageId;
use caelumora_model::store::kept_chain::{fits, follow_back, followed};
use caelumora_model::store::{StoreError, StoreResult};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use std::marker::PhantomData;
use std::ops::Range;
use std::str::FromStr;

/// How many nodes are read at once where a chain is read backwards.
pub(crate) const BATCH: i64 = 256;

/// An entry of a chain, as its row keeps it.
pub(crate) trait Entry:
    Serialize + DeserializeOwned + Clone + Send + Unpin + 'static
{
    /// The message it holds or changes, if it does: what the chain is
    /// searched by, where its table keeps it (`Chains::by_message`).
    fn about(&self) -> Option<MessageId>;
}

/// The tables of the chains of one kind: the nodes, and the heads, each
/// with the key of its chain in its first columns.
#[derive(Debug)]
pub(crate) struct Chains<E> {
    nodes: &'static str,
    heads: &'static str,
    key: &'static [&'static str],
    /// Whether the table of the nodes keeps the message each is about, in
    /// `message_id`.
    by_message: bool,
    entries: PhantomData<fn() -> E>,
}

impl<E: Entry> Chains<E> {
    /// The chains in the table `nodes`, with the heads in `heads`, by the
    /// columns `key`.
    pub(crate) const fn new(
        nodes: &'static str,
        heads: &'static str,
        key: &'static [&'static str],
        by_message: bool,
    ) -> Self {
        Self { nodes, heads, key, by_message, entries: PhantomData }
    }

    /// The condition that names the chain `key` holds, its values bound as
    /// the first parameters.
    fn of_key(&self) -> String {
        let named: Vec<String> = self
            .key
            .iter()
            .enumerate()
            .map(|(at, column)| format!("{column} = ${}", at + 1))
            .collect();
        named.join(" AND ")
    }

    /// Writes `changes` - what the holder's chain added, or changed in its
    /// place, since it was stored - into the chain `key` names, which `name`
    /// names where it fails: the nodes, then the head. Returns the nodes
    /// added.
    ///
    /// # Errors
    /// If they do not fit what is kept (`fits`), or cannot be written.
    pub(crate) async fn append(
        &self,
        conn: &mut PgConnection,
        key: &[String],
        name: &str,
        changes: Stored<E>,
    ) -> StoreResult<Vec<Node<E>>> {
        let end = self.end(conn, key, name).await?;
        fits(end, &changes.entries)
            .map_err(|e| StoreError::Backend(format!("what {name} added does not fit: {e}")))?;
        self.write_nodes(conn, key, name, &changes.entries).await?;
        let columns = self.key.join(", ");
        let values: Vec<String> = (1..=self.key.len()).map(|at| format!("${at}")).collect();
        let upsert = format!(
            "INSERT INTO {} ({columns}, head) VALUES ({}, ${}) ON CONFLICT ({columns}) DO UPDATE SET head = EXCLUDED.head",
            self.heads, values.join(", "), self.key.len() + 1,
        );
        let mut query = sqlx::query(sql(upsert));
        for value in key {
            query = query.bind(value);
        }
        query
            .bind(changes.head.map(|head| head.to_string()))
            .execute(&mut *conn)
            .await
            .map_err(failed(name))?;
        Ok(changes.entries.into_iter().filter(|node| node.seq >= end).collect())
    }

    /// Writes `nodes` in their places, over what is there.
    async fn write_nodes(
        &self,
        conn: &mut PgConnection,
        key: &[String],
        name: &str,
        nodes: &[Node<E>],
    ) -> StoreResult<()> {
        if nodes.is_empty() {
            return Ok(());
        }
        let columns = self.key.join(", ");
        let keys: Vec<String> = (1..=self.key.len()).map(|at| format!("${at}::text")).collect();
        let first = self.key.len() + 1;
        let about = if self.by_message { ", message_id" } else { "" };
        let given = match self.by_message {
            true => format!("${first}::bigint[], ${}::text[], ${}::jsonb[]", first + 1, first + 2),
            false => format!("${first}::bigint[], ${}::jsonb[]", first + 1),
        };
        let upsert = format!(
            "INSERT INTO {nodes} ({columns}, seq{about}, node) SELECT {keys}, seq{about}, node FROM UNNEST({given}) AS given(seq{about}, node) \
             ON CONFLICT ({columns}, seq) DO UPDATE SET {set}node = EXCLUDED.node",
            nodes = self.nodes, keys = keys.join(", "), set = if self.by_message { "message_id = EXCLUDED.message_id, " } else { "" },
        );
        let seqs: Vec<i64> = nodes.iter().map(|node| seq(node.seq)).collect();
        let rows = nodes
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<Value>, _>>()
            .map_err(|e| StoreError::Backend(format!("{name} cannot be written: {e}")))?;
        let mut query = sqlx::query(sql(upsert));
        for value in key {
            query = query.bind(value);
        }
        query = query.bind(seqs);
        if self.by_message {
            query = query.bind(
                nodes
                    .iter()
                    .map(|node| node.entry.about().map(|id| id.to_string()))
                    .collect::<Vec<_>>(),
            );
        }
        query.bind(rows).execute(&mut *conn).await.map_err(failed(name))?;
        Ok(())
    }

    /// The position after the last node of the chain `key` names.
    async fn end(
        &self,
        conn: &mut PgConnection,
        key: &[String],
        name: &str,
    ) -> StoreResult<Position> {
        let statement =
            format!("SELECT COALESCE(MAX(seq) + 1, 0) FROM {} WHERE {}", self.nodes, self.of_key());
        let mut query = sqlx::query_scalar(sql(statement));
        for value in key {
            query = query.bind(value);
        }
        let end: i64 = query.fetch_one(&mut *conn).await.map_err(failed(name))?;
        Ok(position(end))
    }

    /// The head of the chain `key` names; `None` if it has none, or none is
    /// kept.
    pub(crate) async fn head(
        &self,
        conn: &mut PgConnection,
        key: &[String],
        name: &str,
    ) -> StoreResult<Option<Digest>> {
        let statement = format!("SELECT head FROM {} WHERE {}", self.heads, self.of_key());
        let mut query = sqlx::query_scalar(sql(statement));
        for value in key {
            query = query.bind(value);
        }
        let head: Option<Option<String>> =
            query.fetch_optional(&mut *conn).await.map_err(failed(name))?;
        head.flatten()
            .map(|head| {
                Digest::from_str(&head).map_err(|e| {
                    StoreError::Backend(format!("the head of {name} is no digest: {e}"))
                })
            })
            .transpose()
    }

    /// The nodes of the chain `key` names at the positions `range` holds,
    /// oldest first or `newest_first`, at most `limit`.
    pub(crate) async fn nodes(
        &self,
        conn: &mut PgConnection,
        key: &[String],
        name: &str,
        range: Range<Position>,
        newest_first: bool,
        limit: Option<i64>,
    ) -> StoreResult<Vec<Node<E>>> {
        let start = self.key.len() + 1;
        let statement = format!(
            "SELECT node FROM {} WHERE {} AND seq >= ${start} AND seq < ${} ORDER BY seq {} LIMIT ${}",
            self.nodes, self.of_key(), start + 1, if newest_first { "DESC" } else { "ASC" }, start + 2,
        );
        let mut query = sqlx::query_scalar(sql(statement));
        for value in key {
            query = query.bind(value);
        }
        let nodes: Vec<Json<Node<E>>> = query
            .bind(seq(range.start))
            .bind(seq(range.end))
            .bind(limit)
            .fetch_all(&mut *conn)
            .await
            .map_err(failed(name))?;
        Ok(nodes.into_iter().map(|Json(node)| node).collect())
    }

    /// The chain `key` names, which `name` names where it fails, from the
    /// position `from` on, as a holder keeps it; empty if none is kept.
    ///
    /// # Errors
    /// If it cannot be read, or what is read is no chain.
    pub(crate) async fn load(
        &self,
        conn: &mut PgConnection,
        key: &[String],
        name: &str,
        from: Position,
    ) -> StoreResult<Chain<E>> {
        let head = self.head(conn, key, name).await?;
        let entries = self.nodes(conn, key, name, from..Position::MAX, false, None).await?;
        Chain::suffix(from, Stored { entries, head }).map_err(|e| {
            StoreError::Backend(format!("{name} cannot be read from position {from}: {e}"))
        })
    }

    /// The path from the root up to the entry with the digest `digest` of
    /// the chain `key` names, on whichever branch - the newest such entry if
    /// one is there twice -, read backwards in batches; `None` if there is
    /// none.
    ///
    /// # Errors
    /// If it cannot be read, or the path does not lead back to a root.
    pub(crate) async fn path(
        &self,
        conn: &mut PgConnection,
        key: &[String],
        name: &str,
        digest: Digest,
    ) -> StoreResult<Option<Vec<Node<E>>>> {
        let (mut want, mut path) = (Some(digest), Vec::new());
        let mut before = Position::MAX;
        while want.is_some() {
            let nodes =
                self.nodes(conn, key, name, Position::START..before, true, Some(BATCH)).await?;
            let Some(oldest) = nodes.last() else { break };
            before = oldest.seq;
            follow_back(&mut want, &mut path, nodes.iter());
        }
        followed(want, path)
            .map_err(|e| StoreError::Backend(format!("{name} does not lead back to its root: {e}")))
    }

    /// The nodes of the chain `key` names that are about one of the
    /// messages `ids` (`Entry::about`), in the order they were added.
    pub(crate) async fn about(
        &self,
        conn: &mut PgConnection,
        key: &[String],
        name: &str,
        ids: &[MessageId],
    ) -> StoreResult<Vec<Node<E>>> {
        let statement = format!(
            "SELECT node FROM {} WHERE {} AND message_id = ANY(${}) ORDER BY seq",
            self.nodes,
            self.of_key(),
            self.key.len() + 1
        );
        let mut query = sqlx::query_scalar(sql(statement));
        for value in key {
            query = query.bind(value);
        }
        let nodes: Vec<Json<Node<E>>> = query
            .bind(ids.iter().map(ToString::to_string).collect::<Vec<_>>())
            .fetch_all(&mut *conn)
            .await
            .map_err(failed(name))?;
        Ok(nodes.into_iter().map(|Json(node)| node).collect())
    }
}

/// `position` as a column keeps it.
pub(crate) fn seq(position: Position) -> i64 {
    i64::try_from(position.get()).unwrap_or(i64::MAX)
}

/// The position a column keeps.
pub(crate) fn position(seq: i64) -> Position {
    Position::new(u64::try_from(seq).unwrap_or(0))
}

/// An error of the database, as the failure of what `name` names.
pub(crate) fn failed(name: &str) -> impl FnOnce(sqlx::Error) -> StoreError + '_ {
    move |err| StoreError::Backend(format!("{name}: {err}"))
}
