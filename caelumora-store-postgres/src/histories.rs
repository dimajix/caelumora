//! The histories of the channels (issue 379), as the Postgres store keeps
//! them: per channel a chain in `history_entries`, and its head in
//! `histories`.

use crate::chains::{Chains, Entry};
use caelumora_model::chain::Position;
use caelumora_model::channel::ChannelId;
use caelumora_model::history::{History, Recorded};
use caelumora_model::message::MessageId;
use caelumora_model::store::StoreResult;
use sqlx::postgres::PgConnection;

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

/// How the history of `channel` is named where something about it fails.
fn history_of(channel: ChannelId) -> String {
    format!("the history of channel {channel}")
}

/// Writes what `history`, that of `channel` as its state holds it, added
/// since it was stored. Marks nothing: that is for when the transaction is
/// committed.
pub(crate) async fn append(conn: &mut PgConnection, channel: ChannelId, history: &History) -> StoreResult<()> {
    if let Some(changes) = history.changes() {
        HISTORIES.append(conn, &[channel.to_string()], &history_of(channel), changes).await?;
    }
    Ok(())
}

/// The history of `channel` from the position `from` on, as its state
/// holds it; empty if none is kept.
pub(crate) async fn load(conn: &mut PgConnection, channel: ChannelId, from: Position) -> StoreResult<History> {
    HISTORIES.load(conn, &[channel.to_string()], &history_of(channel), from).await
}
