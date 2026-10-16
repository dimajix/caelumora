//! What part of a channel's history a participant sees, read from the store
//! (issues 46, 379): a view begins at a position, and the history is read in
//! pages from its end only as far back as is needed.

use super::ChannelLoop;
use caelumora_model::actor::ActorId;
use caelumora_model::channel::{ChannelId, HistoryAccess, HistoryStart};
use caelumora_model::history::HistoryEntry;
use caelumora_model::message::{Message, MessageId};
use caelumora_model::store::history::{HistoryKey, HistoryStore};
use caelumora_model::store::StoreResult;
use chrono::{DateTime, Utc};
use tracing::warn;

/// How many items a page holds that is read to find something in a history
/// rather than to show it.
const READ_PAGE: usize = 200;

/// The position from which on the history of `channel` is seen from `start`:
/// its beginning if there is none; `None` if the message it begins at is not
/// there, so that nothing is seen rather than more than was granted.
///
/// # Errors
/// If the store cannot be read.
pub(crate) async fn visible_from(histories: &dyn HistoryStore, channel: ChannelId, start: Option<HistoryStart>) -> StoreResult<Option<u64>> {
    let Some(start) = start else { return Ok(Some(0)) };
    Ok(histories.said(HistoryKey::Channel(channel), start.message()).await?.map(|(at, _)| start.from(at)))
}

/// Where the view of the history of `channel` begins for a participant
/// granted `access` now (`HistoryAccess::start_in`): its messages read from
/// the end, newest first, only as far back as the grant reaches.
///
/// # Errors
/// If the store cannot be read.
async fn start_of(histories: &dyn HistoryStore, channel: ChannelId, access: HistoryAccess, now: DateTime<Utc>) -> StoreResult<Option<HistoryStart>> {
    let said = |item: &HistoryEntry| item.said().is_some();
    let mut newest: Vec<Message> = Vec::new();
    let mut before = None;
    while !access.read_enough(&newest, now) {
        let page = histories.page(HistoryKey::Channel(channel), 0, before, READ_PAGE, &said).await?;
        newest.extend(page.items.iter().rev().filter_map(HistoryEntry::said).cloned());
        match page.earlier {
            Some(earlier) => before = Some(earlier),
            None => break,
        }
    }
    Ok(access.start_in(&newest, now))
}

impl ChannelLoop {
    /// The message `id` of the history as it stands, or `None` if it was
    /// never said here - read from the store, where the history is as it is
    /// in memory: what the checks before a change ask (issue 379). A store
    /// that cannot be read is logged, and the message is not found.
    pub(crate) async fn message(&self, id: MessageId) -> Option<Message> {
        match self.history_store.said(HistoryKey::Channel(self.id), id).await {
            Ok(said) => said.and_then(|(_, item)| item.said().cloned()),
            Err(e) => {
                warn!("Cannot read message {} of channel {}: {}", id, self.id, e);
                None
            }
        }
    }

    /// Where the view of the history begins for a participant granted
    /// `access` now (`start_of`).
    ///
    /// # Errors
    /// If the store cannot be read.
    pub(crate) async fn history_start(&self, access: HistoryAccess) -> StoreResult<Option<HistoryStart>> {
        start_of(self.history_store.as_ref(), self.id, access, Utc::now()).await
    }

    /// The position from which on `participant` sees the history - where
    /// what it missed is caught up from when it joins (issue 190); `None` if
    /// it sees nothing of it.
    ///
    /// # Errors
    /// If the store cannot be read.
    pub(crate) async fn visible_from(&self, participant: ActorId) -> StoreResult<Option<u64>> {
        visible_from(self.history_store.as_ref(), self.id, self.channel.load().history_start_of(participant)).await
    }
}
