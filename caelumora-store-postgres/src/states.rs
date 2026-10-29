//! The states, as the Postgres store keeps them (issue 04): each a document
//! in a table of its kind - as serde writes it, without its chain and what
//! else is kept apart -, written whole by every put, in one transaction with
//! what is kept apart: the transcript and what was given (`transcripts`),
//! the history (`histories`). What is marked stored is marked once the
//! transaction is committed.

use crate::chains::failed;
use crate::connection::sql;
use crate::{histories, transcripts, PostgresStore};
use async_trait::async_trait;
use caelumora_model::agent::{AgentChannelState, AgentId, AgentState};
use caelumora_model::channel::{ChannelId, ChannelState};
use caelumora_model::store::transcript::load_named_by_turn;
use caelumora_model::store::{Journaled, StateStore, StoreError, StoreResult};
use caelumora_model::user::{UserChannelState, UserId};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use std::str::FromStr;

/// The tables of the states of the channels, of the agents and of the users
/// in them, by `channel_id`.
const OF_CHANNELS: &[&str] = &["channel_state", "agent_channel_state", "user_channel_state"];

/// The tables of the states of the agents, and of them in the channels, by
/// `agent_id`.
const OF_AGENTS: &[&str] = &["agent_state", "agent_channel_state"];

/// The document of `state`, which `name` names where it fails.
fn document<T: Serialize>(state: &T, name: &str) -> StoreResult<Value> {
    serde_json::to_value(state)
        .map_err(|e| StoreError::Backend(format!("{name} cannot be written: {e}")))
}

/// The document of the row of `table` with `key` in the columns `columns`.
async fn read<T: DeserializeOwned + Send + Unpin + 'static>(
    conn: &mut PgConnection,
    table: &str,
    columns: &[&str],
    key: &[String],
    name: &str,
) -> StoreResult<Option<T>> {
    let named: Vec<String> =
        columns.iter().enumerate().map(|(at, column)| format!("{column} = ${}", at + 1)).collect();
    let mut query =
        sqlx::query_scalar(sql(format!("SELECT body FROM {table} WHERE {}", named.join(" AND "))));
    for value in key {
        query = query.bind(value);
    }
    let body: Option<Json<T>> = query.fetch_optional(&mut *conn).await.map_err(failed(name))?;
    Ok(body.map(|Json(body)| body))
}

/// Writes `body` as the document of the row of `table` with `key` in the
/// columns `columns`, over what is there.
async fn write(
    conn: &mut PgConnection,
    table: &str,
    columns: &[&str],
    key: &[String],
    name: &str,
    body: Value,
) -> StoreResult<()> {
    let values: Vec<String> = (1..=columns.len()).map(|at| format!("${at}")).collect();
    let columns = columns.join(", ");
    let upsert = format!(
        "INSERT INTO {table} ({columns}, body) VALUES ({}, ${}) ON CONFLICT ({columns}) DO UPDATE SET body = EXCLUDED.body",
        values.join(", "), values.len() + 1,
    );
    let mut query = sqlx::query(sql(upsert));
    for value in key {
        query = query.bind(value);
    }
    query.bind(body).execute(&mut *conn).await.map_err(failed(name))?;
    Ok(())
}

/// Deletes the rows of every one of `tables` whose `column` is `id`, in one
/// transaction.
async fn delete(
    store: &PostgresStore,
    tables: &[&str],
    column: &str,
    id: String,
    name: &str,
) -> StoreResult<()> {
    let mut tx = store.connection.begin().await?;
    for table in tables {
        sqlx::query(sql(format!("DELETE FROM {table} WHERE {column} = $1")))
            .bind(&id)
            .execute(&mut *tx)
            .await
            .map_err(failed(name))?;
    }
    tx.commit().await.map_err(failed(name))
}

fn of_agent(agent: AgentId) -> String {
    format!("the state of agent {agent}")
}

fn of_agent_in(agent: AgentId, channel: ChannelId) -> String {
    format!("the state of agent {agent} in channel {channel}")
}

fn of_channel(channel: ChannelId) -> String {
    format!("the state of channel {channel}")
}

fn of_user_in(user: UserId, channel: ChannelId) -> String {
    format!("the state of user {user} in channel {channel}")
}

#[async_trait]
impl StateStore for PostgresStore {
    async fn get_agent_channel_state(
        &self,
        agent_id: AgentId,
        channel_id: ChannelId,
    ) -> StoreResult<AgentChannelState> {
        let mut conn = self.connection.acquire().await?;
        let key = [agent_id.to_string(), channel_id.to_string()];
        let read: Option<AgentChannelState> = read(
            &mut conn,
            "agent_channel_state",
            &["agent_id", "channel_id"],
            &key,
            &of_agent_in(agent_id, channel_id),
        )
        .await?;
        let mut state = read.unwrap_or_default();
        state.transcript =
            transcripts::load(&mut conn, agent_id, channel_id, state.transcript_from).await?;
        drop(conn);
        load_named_by_turn(self, agent_id, channel_id, &mut state).await?;
        Ok(state)
    }

    /// What was given and the transcript, then the document, with whether
    /// it has work under way.
    async fn put_agent_channel_state(
        &self,
        agent_id: AgentId,
        channel_id: ChannelId,
        state: &mut AgentChannelState,
    ) -> StoreResult<()> {
        let name = of_agent_in(agent_id, channel_id);
        let mut tx = self.connection.begin().await?;
        transcripts::append(&mut tx, agent_id, channel_id, state).await?;
        let upsert = "INSERT INTO agent_channel_state (agent_id, channel_id, has_work_under_way, body) VALUES ($1, $2, $3, $4) \
            ON CONFLICT (agent_id, channel_id) DO UPDATE SET has_work_under_way = EXCLUDED.has_work_under_way, body = EXCLUDED.body";
        sqlx::query(upsert)
            .bind(agent_id.to_string())
            .bind(channel_id.to_string())
            .bind(state.has_work_under_way())
            .bind(document(state, &name)?)
            .execute(&mut *tx)
            .await
            .map_err(failed(&name))?;
        tx.commit().await.map_err(failed(&name))?;
        transcripts::stored(state);
        state.mark_stored();
        Ok(())
    }

    /// Read from the partial index of `has_work_under_way`.
    async fn channels_with_work_under_way(&self) -> StoreResult<Vec<ChannelId>> {
        let pool = self.connection.pool().await?;
        let name = "the index of the work under way";
        let channels: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT channel_id FROM agent_channel_state WHERE has_work_under_way",
        )
        .fetch_all(pool)
        .await
        .map_err(failed(name))?;
        channels
            .iter()
            .map(|channel| {
                ChannelId::from_str(channel).map_err(|_| {
                    StoreError::Backend(format!("{name} holds a channel that is none"))
                })
            })
            .collect()
    }

    async fn get_agent_state(&self, agent_id: AgentId) -> StoreResult<AgentState> {
        let mut conn = self.connection.acquire().await?;
        let read: Option<AgentState> = read(
            &mut conn,
            "agent_state",
            &["agent_id"],
            &[agent_id.to_string()],
            &of_agent(agent_id),
        )
        .await?;
        Ok(read.unwrap_or_default())
    }

    async fn put_agent_state(&self, agent_id: AgentId, state: &mut AgentState) -> StoreResult<()> {
        let name = of_agent(agent_id);
        let mut conn = self.connection.acquire().await?;
        write(
            &mut conn,
            "agent_state",
            &["agent_id"],
            &[agent_id.to_string()],
            &name,
            document(state, &name)?,
        )
        .await?;
        state.mark_stored();
        Ok(())
    }

    async fn get_channel_state(&self, channel_id: ChannelId) -> StoreResult<ChannelState> {
        let mut conn = self.connection.acquire().await?;
        let read: Option<ChannelState> = read(
            &mut conn,
            "channel_state",
            &["channel_id"],
            &[channel_id.to_string()],
            &of_channel(channel_id),
        )
        .await?;
        let mut state = read.ok_or_else(|| StoreError::NotFound {
            entity: "channel_state",
            id: channel_id.to_string(),
        })?;
        state.history = histories::load(&mut conn, channel_id, state.history_from).await?;
        Ok(state)
    }

    /// The history, then the document.
    async fn put_channel_state(
        &self,
        channel_id: ChannelId,
        state: &mut ChannelState,
    ) -> StoreResult<()> {
        let name = of_channel(channel_id);
        let mut tx = self.connection.begin().await?;
        histories::append(&mut tx, channel_id, &state.history).await?;
        write(
            &mut tx,
            "channel_state",
            &["channel_id"],
            &[channel_id.to_string()],
            &name,
            document(state, &name)?,
        )
        .await?;
        tx.commit().await.map_err(failed(&name))?;
        state.history.mark_stored();
        state.mark_stored();
        Ok(())
    }

    async fn get_user_channel_state(
        &self,
        user_id: UserId,
        channel_id: ChannelId,
    ) -> StoreResult<UserChannelState> {
        let mut conn = self.connection.acquire().await?;
        let key = [user_id.to_string(), channel_id.to_string()];
        let read: Option<UserChannelState> = read(
            &mut conn,
            "user_channel_state",
            &["user_id", "channel_id"],
            &key,
            &of_user_in(user_id, channel_id),
        )
        .await?;
        Ok(read.unwrap_or_default())
    }

    async fn put_user_channel_state(
        &self,
        user_id: UserId,
        channel_id: ChannelId,
        state: &mut UserChannelState,
    ) -> StoreResult<()> {
        let name = of_user_in(user_id, channel_id);
        let mut conn = self.connection.acquire().await?;
        let key = [user_id.to_string(), channel_id.to_string()];
        write(
            &mut conn,
            "user_channel_state",
            &["user_id", "channel_id"],
            &key,
            &name,
            document(state, &name)?,
        )
        .await?;
        state.mark_stored();
        Ok(())
    }

    /// Its own, of the agents and users in it, the transcripts and what was
    /// given there, and its history.
    async fn delete_channel_state(&self, channel_id: ChannelId) -> StoreResult<()> {
        let tables = [OF_CHANNELS, transcripts::TABLES, histories::TABLES].concat();
        delete(self, &tables, "channel_id", channel_id.to_string(), &of_channel(channel_id)).await
    }

    /// Its own, in every channel, its transcripts and what was given to it.
    async fn delete_agent_state(&self, agent_id: AgentId) -> StoreResult<()> {
        let tables = [OF_AGENTS, transcripts::TABLES].concat();
        delete(self, &tables, "agent_id", agent_id.to_string(), &of_agent(agent_id)).await
    }
}
