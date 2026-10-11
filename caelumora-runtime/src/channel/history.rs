//! What part of a channel's history a participant sees.

use caelumora_model::actor::ActorId;
use caelumora_model::channel::{HistoryAccess, HistoryStart};
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
        self.items.iter().filter_map(said).cloned().collect()
    }
}

/// The message of `item`, if it is one said.
fn said(item: &HistoryEntry) -> Option<&Message> {
    match item {
        HistoryEntry::Said { message, .. } => Some(message),
        _ => None,
    }
}

/// `history` as seen from `start`: all of it if there is none.
pub(crate) fn visible_part(start: Option<HistoryStart>, history: Vec<HistoryEntry>) -> Vec<HistoryEntry> {
    match start {
        Some(start) => visible_from(start, history),
        None => history,
    }
}

/// Where the view of `history` begins for a participant granted `access`,
/// fixed now - so that "the last three days" does not move day by day.
/// `None` if all of it is visible.
pub(super) fn history_start(access: HistoryAccess, history: &[Message], now: chrono::DateTime<chrono::Utc>) -> Option<HistoryStart> {
    let last = || history.last().map(|m| HistoryStart::After(m.id));
    let from = |index: usize| match index {
        0 => None,
        i if i >= history.len() => last(),
        i => Some(HistoryStart::From(history[i].id)),
    };
    match access {
        HistoryAccess::All => None,
        HistoryAccess::None => last(),
        HistoryAccess::LastMessages { count } => from(history.len().saturating_sub(count as usize)),
        HistoryAccess::LastDays { count } => {
            let since = now - chrono::Duration::days(i64::from(count));
            from(history.iter().position(|m| m.timestamp >= since).unwrap_or(history.len()))
        }
    }
}

/// The part of `history` visible from `start` - nothing if the message it
/// begins at is not there, rather than more than was granted.
fn visible_from(start: HistoryStart, history: Vec<HistoryEntry>) -> Vec<HistoryEntry> {
    let (id, skip) = match start {
        HistoryStart::From(id) => (id, 0),
        HistoryStart::After(id) => (id, 1),
    };
    match history.iter().position(|item| said(item).is_some_and(|m| m.id == id)) {
        Some(index) => history.into_iter().skip(index + skip).collect(),
        None => Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::channel::{HistoryAccess, HistoryStart};
    use caelumora_model::message::Message;
    use caelumora_model::message_history::HistoryEntry;
    

    /// Five messages, one a day, the last one `now`.
    fn daily(now: chrono::DateTime<chrono::Utc>) -> Vec<Message> {
        (0..5).rev()
            .map(|days| Message { id: caelumora_model::message::MessageId::new(), timestamp: now - chrono::Duration::days(days), ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) })
            .collect()
    }

    fn ids(history: &[Message]) -> Vec<caelumora_model::message::MessageId> {
        history.iter().map(|m| m.id).collect()
    }

    /// `messages` as the items of a history.
    fn said(messages: &[Message]) -> Vec<HistoryEntry> {
        messages.iter().map(|message| HistoryEntry::Said { message: message.clone(), context: None }).collect()
    }

    /// The ids of the messages of `items`.
    fn ids_of(items: &[HistoryEntry]) -> Vec<caelumora_model::message::MessageId> {
        items.iter().filter_map(super::said).map(|m| m.id).collect()
    }

    fn granted(access: HistoryAccess, history: &[Message], now: chrono::DateTime<chrono::Utc>) -> Vec<caelumora_model::message::MessageId> {
        let items = match history_start(access, history, now) {
            Some(start) => visible_from(start, said(history)),
            None => said(history),
        };
        ids_of(&items)
    }

    #[test]
    fn history_access_all_and_none_grant_everything_and_nothing() {
        let now = chrono::Utc::now();
        let history = daily(now);
        assert_eq!(granted(HistoryAccess::All, &history, now), ids(&history), "All sees every message");
        assert!(granted(HistoryAccess::None, &history, now).is_empty(), "None sees no earlier message");
        assert_eq!(history_start(HistoryAccess::None, &[], now), None, "an empty channel has nothing to hide");
    }

    #[test]
    fn history_access_last_messages_and_days_cut_the_history() {
        let now = chrono::Utc::now();
        let history = daily(now);
        assert_eq!(granted(HistoryAccess::LastMessages { count: 2 }, &history, now), ids(&history[3..]));
        assert_eq!(granted(HistoryAccess::LastMessages { count: 9 }, &history, now), ids(&history),
                   "more than there is grants all of it");
        assert!(granted(HistoryAccess::LastMessages { count: 0 }, &history, now).is_empty());
        // Messages 1, 2 and 3 days ago plus today's - within the last three days.
        assert_eq!(granted(HistoryAccess::LastDays { count: 3 }, &history, now), ids(&history[1..]));
        assert!(granted(HistoryAccess::LastDays { count: 3 }, &history, now + chrono::Duration::days(10)).is_empty(),
                "nothing within the last days sees nothing");
    }

    #[test]
    fn a_history_start_shows_later_messages_and_nothing_if_its_message_is_gone() {
        let now = chrono::Utc::now();
        let mut later = daily(now);
        let start = HistoryStart::After(later[1].id);
        later.push(Message { id: caelumora_model::message::MessageId::new(), ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) });
        assert_eq!(ids_of(&visible_from(start, said(&later))), ids(&later[2..]), "messages sent after joining stay visible");
        assert!(visible_from(HistoryStart::From(caelumora_model::message::MessageId::new()), said(&later)).is_empty(),
                "an unknown start shows nothing rather than everything");
    }
}
