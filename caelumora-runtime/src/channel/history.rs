//! What part of a channel's history a participant sees.

use caelumora_model::actor::ActorId;
use caelumora_model::channel::HistoryStart;
use caelumora_model::message::Message;
use caelumora_model::message_history::HistoryEntry;
use super::ChannelLoop;

/// The part of a channel's history a participant sees (issue 46), as
/// `ChannelLoop::visible_history` gives it.
#[derive(Debug, Clone)]
pub struct VisibleHistory {
    /// What they see, from `start` on: the messages said, each as it
    /// stands, and how turns ended (`MessageHistory::items`, issue 258).
    pub items: Vec<HistoryEntry>,
    /// The digest of the channel's history at the time: up to where it was
    /// read. It names the whole chain, so it is recorded with `start`.
    pub digest: Option<caelumora_model::digest::Digest>,
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

impl ChannelLoop {
    /// The part of the history `participant` may see (issue 46): all of it,
    /// unless they joined later with a limit - with what a reader records as
    /// read (issue 72). The one place the limits are applied: for users over
    /// REST, for agents reading the channel and for the transcript of an
    /// agent that joins.
    pub async fn visible_history(&self, participant: ActorId) -> VisibleHistory {
        let (history, digest) = {
            let state = self.state.lock().await;
            (state.history.items(), state.history.digest())
        };
        let start = self.history_start_of(participant);
        VisibleHistory { items: visible_part(start, history), digest, start }
    }

    /// Where `participant`'s view of the history begins; `None`: all of it.
    fn history_start_of(&self, participant: ActorId) -> Option<HistoryStart> {
        self.channel.load().history_limits.iter().find(|l| l.participant == participant).map(|l| l.start)
    }
}
