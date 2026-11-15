//! How much of a channel's history a participant is given (issue 46, `HistoryAccess`), and where
//! their view of it begins (`HistoryStart`, issue 417).

use crate::message::{Message, MessageId};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// How much of a channel's history a new participant may see, as asked for when adding them (issue
/// 46). The `ChannelLoop` fixes it as a position when they join (`start_in`, `HistoryStart`) and
/// applies that.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryAccess {
    #[default]
    All,
    None,
    LastDays {
        count: u32,
    },
    LastMessages {
        count: u32,
    },
}

impl HistoryAccess {
    /// Whether `newest` - a channel's messages, newest first, as far as they were read - is enough
    /// to tell where the view begins (`start_in`): the history is read from its end only as far
    /// back as the grant reaches (issue 379), not at all for all of it or nothing.
    pub fn read_enough(&self, newest: &[Message], now: DateTime<Utc>) -> bool {
        match *self {
            HistoryAccess::All | HistoryAccess::None | HistoryAccess::LastMessages { count: 0 } => {
                true
            }
            HistoryAccess::LastMessages { count } => newest.len() > count as usize,
            HistoryAccess::LastDays { count } => {
                let since = now - Duration::days(i64::from(count));
                newest.last().is_some_and(|m| m.timestamp < since)
            }
        }
    }

    /// Where the view of a channel's history begins for a participant granted this, fixed `now` -
    /// so that "the last three days" does not move day by day. `newest` are its messages, newest
    /// first, as many as `read_enough` asks for, or all there are.
    pub fn start_in(&self, newest: &[Message], now: DateTime<Utc>) -> HistoryStart {
        // From the `n`-th newest, if an older one is there; all of it if not.
        let from = |n: usize| {
            newest.get(n).map_or(HistoryStart::Beginning, |_| HistoryStart::From(newest[n - 1].id))
        };
        match *self {
            HistoryAccess::All => HistoryStart::Beginning,
            HistoryAccess::None | HistoryAccess::LastMessages { count: 0 } => HistoryStart::Join,
            HistoryAccess::LastMessages { count } => from(count as usize),
            HistoryAccess::LastDays { count } => {
                let since = now - Duration::days(i64::from(count));
                match newest.iter().take_while(|m| m.timestamp >= since).count() {
                    0 => HistoryStart::Join,
                    recent => from(recent),
                }
            }
        }
    }
}

/// Where a participant's view of a channel's history begins, as a grant reads it off the messages
/// (`HistoryAccess::start_in`); fixed as a position as they join (`ChannelLoop::history_start`),
/// which the `HistoryShared` they are given records (issue 417).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryStart {
    /// From the beginning: all of it is visible.
    Beginning,
    /// From this message on.
    From(MessageId),
    /// At the join, where nothing is granted: nothing recorded before it is visible - no message,
    /// nor who joined before or how turns ended (issue 568).
    Join,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidentiality::Classification;

    /// Five messages, one a day, the last one `now`.
    fn daily(now: DateTime<Utc>) -> Vec<Message> {
        (0..5)
            .rev()
            .map(|days| Message {
                id: MessageId::new(),
                timestamp: now - Duration::days(days),
                ..Message::new(Classification::CHANNEL_DEFAULT)
            })
            .collect()
    }

    fn ids(history: &[Message]) -> Vec<MessageId> {
        history.iter().map(|m| m.id).collect()
    }

    /// What `access` grants of `history`, oldest first, read from the end
    /// only as far as it asks: the ids from where the view begins.
    fn granted(access: HistoryAccess, history: &[Message], now: DateTime<Utc>) -> Vec<MessageId> {
        let mut newest: Vec<Message> = Vec::new();
        for message in history.iter().rev() {
            if access.read_enough(&newest, now) {
                break;
            }
            newest.push(message.clone());
        }
        let at = |start: MessageId| history.iter().position(|m| m.id == start).unwrap();
        let from = match access.start_in(&newest, now) {
            HistoryStart::Beginning => 0,
            HistoryStart::From(start) => at(start),
            HistoryStart::Join => history.len(),
        };
        ids(&history[from.min(history.len())..])
    }

    #[test]
    fn history_access_all_and_none_grant_everything_and_nothing() {
        let now = Utc::now();
        let history = daily(now);
        assert_eq!(
            granted(HistoryAccess::All, &history, now),
            ids(&history),
            "All sees every message"
        );
        assert!(
            granted(HistoryAccess::None, &history, now).is_empty(),
            "None sees no earlier message"
        );
        let newest: Vec<Message> = history.iter().rev().cloned().collect();
        for nothing in [
            HistoryAccess::None,
            HistoryAccess::LastMessages { count: 0 },
            HistoryAccess::LastDays { count: 3 },
        ] {
            for messages in [&newest[..], &[]] {
                assert_eq!(
                    nothing.start_in(messages, now + Duration::days(10)),
                    HistoryStart::Join,
                    "{nothing:?}: from the join - not after the last message, whatever followed \
                     it, nor from the beginning where none was said (issue 568)"
                );
            }
        }
        for read_none in
            [HistoryAccess::All, HistoryAccess::None, HistoryAccess::LastMessages { count: 0 }]
        {
            assert!(read_none.read_enough(&[], now), "{read_none:?}: granted without reading any");
        }
    }

    #[test]
    fn history_access_last_messages_and_days_cut_the_history() {
        let now = Utc::now();
        let history = daily(now);
        assert_eq!(
            granted(HistoryAccess::LastMessages { count: 2 }, &history, now),
            ids(&history[3..])
        );
        assert_eq!(
            granted(HistoryAccess::LastMessages { count: 9 }, &history, now),
            ids(&history),
            "more than there is grants all of it"
        );
        assert!(granted(HistoryAccess::LastMessages { count: 0 }, &history, now).is_empty());
        // Messages 1, 2 and 3 days ago plus today's - within the last three days.
        assert_eq!(
            granted(HistoryAccess::LastDays { count: 3 }, &history, now),
            ids(&history[1..])
        );
        assert!(
            granted(HistoryAccess::LastDays { count: 3 }, &history, now + Duration::days(10))
                .is_empty(),
            "nothing within the last days sees nothing"
        );
        assert!(
            !HistoryAccess::LastDays { count: 3 }
                .read_enough(&history[3..].iter().rev().cloned().collect::<Vec<_>>(), now),
            "read back until one older than the grant"
        );
    }
}
