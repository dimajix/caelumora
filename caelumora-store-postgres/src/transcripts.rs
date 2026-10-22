//! The transcripts of the agents (issue 380), as the Postgres store keeps
//! them: per agent and channel a chain in `transcript_entries`, each node
//! with the message it holds or changes, by which it is found, and its head
//! in `transcripts`. Beside them what the agent's model was given (issues
//! 383, 426) - the prompt texts, the tool sets and the frames -, each once
//! by its digest: a value there already is not written again.

use crate::chains::{failed, Chains, Entry};
use crate::connection::sql;
use crate::PostgresStore;
use async_trait::async_trait;
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::chain::{Node, Position};
use caelumora_model::channel::ChannelId;
use caelumora_model::context::FrameRef;
use caelumora_model::digest::Digest;
use caelumora_model::kept::Kept;
use caelumora_model::message::{Message, MessageId};
use caelumora_model::store::transcript::{transcript_of, TranscriptStore};
use caelumora_model::store::{StoreError, StoreResult};
use caelumora_model::tool::ToolSet;
use caelumora_model::transcript::{known, Transcript, TranscriptEntry};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use std::collections::{BTreeMap, HashSet};

/// The chains of the transcripts, by agent and channel.
const TRANSCRIPTS: Chains<TranscriptEntry> = Chains::new("transcript_entries", "transcripts", &["agent_id", "channel_id"], true);

/// The tables of the transcripts and of what was given, each by agent and
/// channel: what goes with an agent's state in a channel.
pub(crate) const TABLES: &[&str] = &["transcripts", "transcript_entries", "prompt_texts", "offered_tools", "frames"];

impl Entry for TranscriptEntry {
    fn about(&self) -> Option<MessageId> {
        self.message().map(|message| message.id).or_else(|| self.changed())
    }
}

/// The key of the transcript of `agent` in `channel`.
fn key(agent: AgentId, channel: ChannelId) -> [String; 2] {
    [agent.to_string(), channel.to_string()]
}

/// Writes what `state`, that of `agent` in `channel`, kept and added since
/// it was stored: the prompt texts, the tool sets and the frames, which
/// what follows names, then the transcript. Marks nothing: that is for
/// when the transaction is committed (`stored`).
pub(crate) async fn append(conn: &mut PgConnection, agent: AgentId, channel: ChannelId, state: &AgentChannelState) -> StoreResult<()> {
    let name = transcript_of(agent, channel);
    let key = key(agent, channel);
    write_kept(conn, "prompt_texts", "text", "value #>> '{}'", &key, &name, &state.prompts).await?;
    write_kept(conn, "offered_tools", "sets", "value", &key, &name, &state.offered_tools).await?;
    write_kept(conn, "frames", "frame", "value", &key, &name, &state.frames).await?;
    if let Some(changes) = state.transcript.changes() {
        TRANSCRIPTS.append(conn, &key, &name, changes).await?;
    }
    Ok(())
}

/// Marks what `append` wrote of `state` stored.
pub(crate) fn stored(state: &mut AgentChannelState) {
    state.prompts.mark_stored();
    state.offered_tools.mark_stored();
    state.frames.mark_stored();
    state.transcript.mark_stored();
}

/// The transcript of `agent` in `channel` from the position `from` on, as
/// its state holds it; empty if none is kept.
pub(crate) async fn load(conn: &mut PgConnection, agent: AgentId, channel: ChannelId, from: Position) -> StoreResult<Transcript> {
    TRANSCRIPTS.load(conn, &key(agent, channel), &transcript_of(agent, channel), from).await
}

/// Writes what `kept` added since it was last stored into `table`, each
/// digest once: the value, as JSON, into `column` as `value` - an
/// expression over it - makes it.
async fn write_kept<V: Serialize + Clone>(conn: &mut PgConnection, table: &str, column: &str, value: &str, key: &[String; 2], name: &str, kept: &Kept<V>) -> StoreResult<()> {
    let changes = kept.changes();
    if changes.is_empty() {
        return Ok(());
    }
    let digests: Vec<String> = changes.iter().map(|(digest, _)| digest.to_string()).collect();
    let values = changes.iter().map(|(_, value)| serde_json::to_value(value)).collect::<Result<Vec<Value>, _>>()
        .map_err(|e| StoreError::Backend(format!("what was given beside {name} cannot be written: {e}")))?;
    let insert = format!(
        "INSERT INTO {table} (agent_id, channel_id, digest, {column}) SELECT $1, $2, digest, {value} FROM UNNEST($3::text[], $4::jsonb[]) AS given(digest, value) \
         ON CONFLICT DO NOTHING",
    );
    sqlx::query(sql(insert)).bind(&key[0]).bind(&key[1]).bind(digests).bind(values).execute(&mut *conn).await.map_err(failed(name))?;
    Ok(())
}

impl PostgresStore {
    /// The value of `column` of the row of `table` with the digest
    /// `digest`, given to `agent` in `channel`.
    async fn given<T: DeserializeOwned + Send + Unpin + 'static>(&self, table: &str, column: &str, agent: AgentId, channel: ChannelId, digest: Digest) -> StoreResult<Option<T>> {
        let pool = self.connection.pool().await?;
        let key = key(agent, channel);
        let statement = format!("SELECT {column} FROM {table} WHERE agent_id = $1 AND channel_id = $2 AND digest = $3");
        let value: Option<Json<T>> = sqlx::query_scalar(sql(statement)).bind(&key[0]).bind(&key[1]).bind(digest.to_string())
            .fetch_optional(pool).await.map_err(failed(&transcript_of(agent, channel)))?;
        Ok(value.map(|Json(value)| value))
    }
}

#[async_trait]
impl TranscriptStore for PostgresStore {
    async fn path(&self, agent: AgentId, channel: ChannelId, digest: Digest) -> StoreResult<Option<Vec<Node<TranscriptEntry>>>> {
        let mut conn = self.connection.acquire().await?;
        TRANSCRIPTS.path(&mut conn, &key(agent, channel), &transcript_of(agent, channel), digest).await
    }

    /// The nodes about it, from the one that holds it on.
    async fn message(&self, agent: AgentId, channel: ChannelId, id: MessageId) -> StoreResult<Option<Message>> {
        let mut conn = self.connection.acquire().await?;
        let nodes = TRANSCRIPTS.about(&mut conn, &key(agent, channel), &transcript_of(agent, channel), &[id]).await?;
        Ok(known(nodes.iter(), id))
    }

    async fn in_transcript(&self, agent: AgentId, channel: ChannelId, ids: &[MessageId]) -> StoreResult<HashSet<MessageId>> {
        let mut conn = self.connection.acquire().await?;
        let nodes = TRANSCRIPTS.about(&mut conn, &key(agent, channel), &transcript_of(agent, channel), ids).await?;
        Ok(nodes.iter().filter_map(|node| node.entry.message().map(|message| message.id)).collect())
    }

    async fn prompts(&self, agent: AgentId, channel: ChannelId, digests: &[Digest]) -> StoreResult<BTreeMap<Digest, String>> {
        let pool = self.connection.pool().await?;
        let key = key(agent, channel);
        let wanted: Vec<String> = digests.iter().map(ToString::to_string).collect();
        let found: Vec<(String, String)> = sqlx::query_as("SELECT digest, text FROM prompt_texts WHERE agent_id = $1 AND channel_id = $2 AND digest = ANY($3)")
            .bind(&key[0]).bind(&key[1]).bind(&wanted).fetch_all(pool).await.map_err(failed(&transcript_of(agent, channel)))?;
        let found: BTreeMap<String, String> = found.into_iter().collect();
        Ok(digests.iter().zip(&wanted).filter_map(|(digest, wanted)| found.get(wanted).map(|text| (*digest, text.clone()))).collect())
    }

    async fn offered_tools(&self, agent: AgentId, channel: ChannelId, digest: Digest) -> StoreResult<Option<Vec<ToolSet>>> {
        self.given("offered_tools", "sets", agent, channel, digest).await
    }

    async fn frame(&self, agent: AgentId, channel: ChannelId, digest: Digest) -> StoreResult<Option<FrameRef>> {
        self.given("frames", "frame", agent, channel, digest).await
    }
}
