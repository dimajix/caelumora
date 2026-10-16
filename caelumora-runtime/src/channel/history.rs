//! What part of a channel's history a participant sees.

use super::ChannelLoop;
use caelumora_model::actor::ActorId;
use caelumora_model::channel::HistoryStart;
use caelumora_model::history::{History, HistoryEntry, HistoryPage};
use caelumora_model::message::{Message, MessageId};
use caelumora_model::digest::Digest;

/// The part of a channel's history a participant sees (issue 46), as
/// `ChannelLoop::visible_history` gives it.
#[derive(Debug, Clone)]
pub struct VisibleHistory {
    /// What they see, from `start` on: the messages said, each as it
    /// stands, and how turns ended (`History::items`, issue 258).
    pub items: Vec<HistoryEntry>,
    /// The digest of the channel's history at the time: up to where it was
    /// read. It names the whole chain, so it is recorded with `start`.
    pub digest: Option<Digest>,
    /// Where their view begins - from where it was read; `None`: all of it.
    pub start: Option<HistoryStart>,
}

impl VisibleHistory {
    /// The messages of it alone, for what reads only what was said.
    pub fn messages(&self) -> Vec<Message> {
        self.items.iter().filter_map(HistoryEntry::said).cloned().collect()
    }
}

/// `history` as seen from `start`: all of it if there is none.
pub(crate) fn visible_part(start: Option<HistoryStart>, history: Vec<HistoryEntry>) -> Vec<HistoryEntry> {
    match start {
        Some(start) => start.visible(history),
        None => history,
    }
}

/// A page of `history` as seen from `start` (issue 218): the last `limit`
/// items before the position `before` - the last ones without it - that
/// `keep` keeps, none before where the view begins.
pub(crate) fn visible_page(history: &History, start: Option<HistoryStart>, before: Option<u64>, limit: usize, keep: impl Fn(&HistoryEntry) -> bool) -> HistoryPage {
    let Some(from) = visible_from(history, start) else { return HistoryPage::default() };
    history.items_before(before.unwrap_or(history.end()), from, limit, keep)
}

/// The message `id` of `history` as it stands, if it is seen from `start`.
pub(crate) fn visible_message(history: &History, start: Option<HistoryStart>, id: MessageId) -> Option<Message> {
    let from = visible_from(history, start)?;
    history.position_of(id).filter(|at| *at >= from)?;
    history.message(id)
}

/// The position from which on `history` is seen from `start`; `None` if
/// nothing of it is.
fn visible_from(history: &History, start: Option<HistoryStart>) -> Option<u64> {
    match start {
        Some(start) => start.position_in(history),
        None => Some(history.base()),
    }
}

impl ChannelLoop {
    /// The part of the history `participant` may see (issue 46): all of it,
    /// unless they joined later with a limit - with what a reader records as
    /// read (issue 72). Where the limits are applied - here, for agents
    /// reading the channel and for the transcript of an agent that joins,
    /// and in `visible_page` and `visible_message`, for users over REST.
    pub async fn visible_history(&self, participant: ActorId) -> VisibleHistory {
        let (history, digest) = {
            let state = self.state.lock().await;
            (state.history.items(), state.history.digest())
        };
        let start = self.channel.load().history_start_of(participant);
        VisibleHistory { items: visible_part(start, history), digest, start }
    }

    /// A page of the history as `participant` sees it (issue 218,
    /// `visible_page`).
    pub async fn visible_page(&self, participant: ActorId, before: Option<u64>, limit: usize, keep: impl Fn(&HistoryEntry) -> bool) -> HistoryPage {
        let start = self.channel.load().history_start_of(participant);
        visible_page(&self.state.lock().await.history, start, before, limit, keep)
    }

    /// The message `id` as it stands, if `participant` sees it.
    pub async fn visible_message(&self, participant: ActorId, id: MessageId) -> Option<Message> {
        let start = self.channel.load().history_start_of(participant);
        visible_message(&self.state.lock().await.history, start, id)
    }
}
