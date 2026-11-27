//! How much of a channel's history a participant is given (issue 46, `HistoryAccess`), and where
//! their view of it begins (`HistoryStart`, issue 417).

use crate::message::{Message, MessageId};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// How much of a channel's history a new participant may see, as asked for when adding them (issue
/// 46). The `ChannelLoop` fixes it as a position when they join (`start_in`, `HistoryStart`) and
/// applies that. A grant counts and dates messages alone - joins and other events are none: the
/// view begins at the oldest message granted, with the joins and other events that follow it, and
/// at the join if none is (issue 568); only `All` reaches back to the beginning.
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
            HistoryAccess::All | HistoryAccess::None => true,
            HistoryAccess::LastMessages { count } => newest.len() >= count as usize,
            HistoryAccess::LastDays { count } => Self::since(count, now)
                .is_some_and(|since| newest.last().is_some_and(|m| m.timestamp < since)),
        }
    }

    /// Where the view of a channel's history begins for a participant granted this, fixed `now` -
    /// so that "the last three days" does not move day by day. `newest` are its messages, newest
    /// first, as many as `read_enough` asks for, or all there are.
    pub fn start_in(&self, newest: &[Message], now: DateTime<Utc>) -> HistoryStart {
        // From the oldest of the `granted` newest, as far as there are; at the join if none is.
        let from = |granted: usize| {
            newest[..granted.min(newest.len())]
                .last()
                .map_or(HistoryStart::Join, |m| HistoryStart::From(m.id))
        };
        match *self {
            HistoryAccess::All => HistoryStart::Beginning,
            HistoryAccess::None => HistoryStart::Join,
            HistoryAccess::LastMessages { count } => from(count as usize),
            HistoryAccess::LastDays { count } => {
                let since = Self::since(count, now);
                from(newest.iter().take_while(|m| since.is_none_or(|t| m.timestamp >= t)).count())
            }
        }
    }

    /// When the last `days` days before `now` began; `None` if that lies before any time there can
    /// be - a grant reaching so far back reaches every message.
    fn since(days: u32, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        now.checked_sub_signed(Duration::days(i64::from(days)))
    }
}

/// Where a participant's view of a channel's history begins, as a grant reads it off the messages
/// (`HistoryAccess::start_in`); fixed as a position as they join (`ChannelLoop::history_start`),
/// which the `HistoryShared` they are given records (issue 417).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryStart {
    /// From the beginning: all of it is visible, granted whole.
    Beginning,
    /// From this message on, the oldest granted: nothing recorded before it is visible.
    From(MessageId),
    /// At the join, where no message is granted: nothing recorded before it is visible - no
    /// message, nor who joined before or how turns ended (issue 568).
    Join,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::testing::make_message;
    use crate::message::MessageRole;

    /// Five messages, one a day, the last one `now`.
    fn daily(now: DateTime<Utc>) -> Vec<Message> {
        (0..5)
            .rev()
            .map(|days| Message {
                timestamp: now - Duration::days(days),
                content: None,
                role: MessageRole::System,
                ..make_message(MessageId::new())
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
        assert_eq!(
            HistoryAccess::All.start_in(&[], now),
            HistoryStart::Beginning,
            "All alone reaches back to the beginning, before every message"
        );
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
            "more than there is grants every message"
        );
        // A grant counts messages alone (issue 568): it reaches back to the oldest message it
        // grants, not past it to the joins and other events before.
        let newest: Vec<Message> = history.iter().rev().cloned().collect();
        let oldest = HistoryStart::From(history[0].id);
        for more in [
            HistoryAccess::LastMessages { count: 9 },
            HistoryAccess::LastDays { count: 9 },
            HistoryAccess::LastDays { count: u32::MAX },
        ] {
            assert_eq!(more.start_in(&newest, now), oldest, "{more:?}: from the oldest message");
            assert_eq!(more.start_in(&[], now), HistoryStart::Join, "{more:?}: none to grant");
        }
        assert_eq!(
            granted(HistoryAccess::LastDays { count: u32::MAX }, &history, now),
            ids(&history),
            "days reaching further back than any time there can be: every message, read to the end"
        );
        assert!(
            HistoryAccess::LastMessages { count: 2 }.read_enough(&newest[..2], now),
            "as many read as are granted"
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
