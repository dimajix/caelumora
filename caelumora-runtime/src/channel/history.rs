//! What part of a channel's history a participant sees, read from the store
//! (issues 46, 379): a view begins at a position, and the history is read in
//! pages from its end only as far back as is needed.

use super::ChannelLoop;
use caelumora_model::actor::ActorId;
use caelumora_model::channel::{ChannelId, HistoryAccess, HistoryStart};
use caelumora_model::history::{HistoryEntry, Reader};
use caelumora_model::message::{Message, MessageId};
use caelumora_model::store::history::HistoryStore;
use caelumora_model::store::StoreResult;
use chrono::{DateTime, Utc};
use tracing::warn;

/// How many items a page holds that is read to find something in a history
/// rather than to show it.
const READ_PAGE: usize = 200;

/// Where the view of the history of `channel` begins for a participant
/// granted `access` now (`HistoryAccess::start_in`): its messages read from
/// the end, newest first, only as far back as the grant reaches - those that
/// went to everybody, which is what a grant shares.
///
/// # Errors
/// If the store cannot be read.
async fn start_of(histories: &dyn HistoryStore, channel: ChannelId, access: HistoryAccess, now: DateTime<Utc>) -> StoreResult<Option<HistoryStart>> {
    let said = |item: &HistoryEntry| item.said().is_some();
    let mut newest: Vec<Message> = Vec::new();
    let mut before = None;
    while !access.read_enough(&newest, now) {
        let page = histories.page(channel, &Reader::Outsider, 0, before, READ_PAGE, &said).await?;
        newest.extend(page.items.iter().rev().filter_map(HistoryEntry::said).cloned());
        match page.earlier {
            Some(earlier) => before = Some(earlier),
            None => break,
        }
    }
    Ok(access.start_in(&newest, now))
}

impl ChannelLoop {
    /// The message `id` of the history as it stands, as it was recorded, or
    /// `None` if it was never said here - read from the store, where the
    /// history is as it is in memory: what the checks before a change ask
    /// (issue 379). A store that cannot be read is logged, and the message
    /// is not found.
    pub(crate) async fn message(&self, id: MessageId) -> Option<Message> {
        match self.history_store.said(self.id, &Reader::Channel, id).await {
            Ok(said) => said.and_then(|(_, item)| item.said().cloned()),
            Err(e) => {
                warn!("Cannot read message {} of channel {}: {}", id, self.id, e);
                None
            }
        }
    }

    /// The position from which on a participant granted `access` now sees
    /// the history (`start_of`, issue 417): fixed as it joins, so that what
    /// it is given does not move with what is said after. If the message
    /// the view begins at is gone meanwhile, from the history's end, so that
    /// nothing is seen rather than more than was granted.
    ///
    /// # Errors
    /// If the store cannot be read.
    pub(crate) async fn history_start(&self, access: HistoryAccess) -> StoreResult<u64> {
        let Some(start) = start_of(self.history_store.as_ref(), self.id, access, Utc::now()).await? else { return Ok(0) };
        Ok(match self.history_store.said(self.id, &Reader::Channel, start.message()).await? {
            Some((at, _)) => start.from(at),
            None => self.state.lock().await.history.end(),
        })
    }

    /// The position from which on `participant` sees the history (issues
    /// 46, 417) - where what it missed is caught up from when it joins
    /// (issue 190) -, as the channel's document keeps it.
    pub(crate) fn visible_from(&self, participant: ActorId) -> u64 {
        self.channel.load().history_from(participant)
    }
}
