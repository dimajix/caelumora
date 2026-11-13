//! The history of a channel as an agent keeps it (issue 06): the chain of what happened in it -
//! messages said, each of the agent's own with what it was produced from, reclassifications (issue
//! 133), changes to what was said (issue 234), where the steps of a plan began (issue 263), how
//! turns ended (issue 258) and who joined or left (issue 408) - read into the messages as they
//! stand (issue 197). Every entry is recorded with whom it was sent by and whom it went to
//! (`Recorded`, issue 417), and with the head of the envelope it came in, so that it can be
//! delivered again from here (`Recorded::envelope`, issue 223); and read only for a reader, who is
//! given what of it they may see (`Recorded::as_seen_by`), and who stands somewhere in it
//! (`Heard`).
//!
//! Its parts: what happened (`entry`), what is recorded of it and whom it went to (`recorded`), who
//! reads it and where they stand (`reader`), and what a page of it is filtered by (`filter`); here
//! the history itself, what is read from it and how it is changed (`History`, `fold`,
//! `HistoryPage`).

mod entry;
mod filter;
mod reader;
mod recorded;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use entry::HistoryEntry;
pub use filter::{HistoryField, KeptItem};
pub use reader::{Heard, Reader, EVERY_POSITION};
pub use recorded::{Audience, Recipients, Recorded};

use crate::actor::ActorId;
use crate::chain::{Chain, Node, Position};
use crate::confidentiality::Classification;
use crate::digest;
use crate::digest::Digest;
use crate::message::{Message, MessageId};
use crate::turn::GivenUpReason;
use crate::work::TurnId;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// The conversation as it happened - a chain like a `Transcript`. What it
/// records is read from start to end into the messages as they stand: a
/// reclassification, a modification or a revocation applies to a message on
/// any branch, as a change made in the channel does, so it is folded over
/// every branch in the order things happened (`fold`). A message taken back
/// stays, without what it said (`Message::revoked`), so that whatever names
/// it by id finds it.
pub type History = Chain<Recorded>;

/// The messages `nodes` say - a range of a history, in the order its
/// entries were added - each with the node that said it, as they stand
/// after every later node of the range (`HistoryEntry::fold_into`), in the
/// order they were said. A change always comes after what it changes, so a
/// range that goes on to the end of its history gives every message in it
/// as it stands; a change of a message before the range is left out.
pub fn fold<'a, E: AsRef<HistoryEntry> + 'a>(
    nodes: impl IntoIterator<Item = &'a Node<E>>,
) -> Vec<(&'a Node<E>, Message)> {
    let mut messages: Vec<(&Node<E>, Message)> = Vec::new();
    let mut at: HashMap<MessageId, usize> = HashMap::new();
    for node in nodes {
        match node.entry.as_ref() {
            HistoryEntry::Said { message, .. } => {
                at.insert(message.id, messages.len());
                messages.push((node, message.clone()));
            }
            HistoryEntry::Fitted { .. } => {
                for (_, message) in &mut messages {
                    node.entry.as_ref().fold_into(message);
                }
            }
            entry => {
                if let Some(&i) = entry.changed().and_then(|id| at.get(&id)) {
                    entry.fold_into(&mut messages[i].1);
                }
            }
        }
    }
    messages
}

/// How many entries of a history its holder keeps in memory at least (issue
/// 379): the rest is read from the store.
pub const WINDOW: u64 = 200;

/// A page of a history's items (issues 218, 379): the last ones before a
/// position, as `items` gives them, where the page before it ends, and which
/// history it was read from.
#[derive(Debug, Clone, Default)]
pub struct HistoryPage {
    /// The items, oldest first, each at the position of the entry that
    /// added it (issue 417): what a client tells an item by, and the live
    /// event that announced it (`EventContext::position`).
    pub items: Vec<(Position, HistoryEntry)>,
    /// The position of the oldest of `items`: what the page before this one
    /// is read before (`HistoryStore::page`); `None` if there is no older
    /// item to be had.
    pub earlier: Option<Position>,
    /// The digest of the history when the page was read: up to where it was
    /// read.
    pub digest: Option<Digest>,
}

impl HistoryPage {
    /// The items without their positions, oldest first.
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &HistoryEntry> {
        self.items.iter().map(|(_, entry)| entry)
    }
}

impl Chain<Recorded> {
    /// The turn of `author` given up, and why, if its end is the last thing
    /// of the conversation on the active path (issue 259,
    /// `HistoryEntry::is_conversation`): what may be asked for anew as long
    /// as nothing followed it - a notice or a request does not count.
    pub fn turn_given_up_last(&self, author: ActorId) -> Option<(TurnId, GivenUpReason)> {
        match self
            .entries()
            .rev()
            .map(|node| &node.entry.entry)
            .find(|entry| entry.is_conversation())
        {
            Some(HistoryEntry::TurnGivenUp { turn_id, actor, reason, .. }) if *actor == author => {
                Some((*turn_id, reason.clone()))
            }
            _ => None,
        }
    }

    /// The message `id` as it stands - on whichever branch - or `None` if
    /// it was never said here: as it was said, with every entry recorded
    /// after it folded into it (`HistoryEntry::fold_into`), which only
    /// changes it if it names it or fits the channel. Read from where it
    /// was said on, not from the start, and nothing but it is cloned.
    pub fn message(&self, id: MessageId) -> Option<Message> {
        let said = self
            .all_entries()
            .rev()
            .find(|node| node.entry.entry.said().is_some_and(|m| m.id == id))?;
        let mut message = said.entry.entry.said()?.clone();
        for node in self.from(said.seq.next()) {
            node.entry.entry.fold_into(&mut message);
        }
        Some(message)
    }

    /// Appends `msgs`, each as `push` does.
    pub fn extend(&mut self, msgs: Vec<Message>) {
        for m in msgs {
            self.push(m);
        }
    }

    /// Appends `msg`, said - with what it was produced from, if it says.
    pub fn push(&mut self, msg: Message) {
        self.record(HistoryEntry::Said { message: msg });
    }

    /// Records that message `id` - on whichever branch - is classified
    /// `classification` from now on (issue 133). `false`, and nothing is
    /// recorded, if there is no such message or it was taken back
    /// (`Message::reclassify`); one before what it holds is recorded
    /// (`is_changed`).
    pub fn reclassify(&mut self, id: MessageId, classification: Classification) -> bool {
        self.enter(Recorded::delivered(HistoryEntry::Reclassified { message: id, classification }))
    }

    /// Records that the message `modified.id` - on whichever branch -
    /// reads as `modified` from now on (issue 234). `false`, and nothing is
    /// recorded, if there is no such message or it cannot be modified so as
    /// it stands (`Message::modify`): taken back, or by another sender; one
    /// before what it holds is recorded (`is_changed`).
    pub fn modify(&mut self, modified: Message) -> bool {
        self.enter(Recorded::delivered(HistoryEntry::Modified { message: modified }))
    }

    /// Records that the message `id` - on whichever branch - was taken back
    /// at `at` (issue 234). `false`, and nothing is recorded, if there is no
    /// such message or it was taken back already; one before what it holds
    /// is recorded (`is_changed`).
    pub fn revoke(&mut self, id: MessageId, at: DateTime<Utc>) -> bool {
        self.enter(Recorded::delivered(HistoryEntry::Revoked { message: id, at }))
    }

    /// Whether what `change` does to the message `id` is a change to
    /// record: whether it changes the message as it stands, if it holds
    /// it. Held from a position on (issue 379), it may not hold a message
    /// said before: the change is recorded then, and the store applies it to
    /// the message if it holds it - a change of a message it never held
    /// changes nothing there either.
    fn is_changed(&self, id: MessageId, change: impl FnOnce(&mut Message) -> bool) -> bool {
        match self.message(id) {
            Some(mut standing) => change(&mut standing),
            None => self.base() > Position::START,
        }
    }

    /// Records that the channel is classified `channel` now, which every
    /// message said so far - on every branch: all of them were said in the
    /// channel - fits into from now on (`Classification::fitted_into`).
    /// Recorded whether or not it changes a message: telling which would
    /// fold the whole history.
    pub fn fit_into(&mut self, channel: &Classification) {
        self.enter(Recorded::delivered(HistoryEntry::Fitted { channel: channel.clone() }));
    }

    /// Records `item`, what happened that is no change of a message (`is_item`):
    /// a message said - as `push` records it, with what it was produced
    /// from -, how a turn ended (issue 258), after its messages, where a step
    /// of its plan began (issue 263), before the messages of the step, a
    /// command carried out (issue 269) or an activity ended (issue 284),
    /// before the turn's end, a compaction (issue 16), before what replaced
    /// the older messages, someone added or removed (issue 408). A change is recorded by its own
    /// method - `modify`, `revoke`, `reclassify`, `fit_into` -, which says whether it
    /// applies.
    pub fn record(&mut self, item: HistoryEntry) {
        debug_assert!(item.is_item(), "a change is recorded by its own method");
        self.enter(Recorded::delivered(item));
    }

    /// Records `recorded`, as the channel records what enters it (issue
    /// 417): an item after the last; a change - a modification, a
    /// revocation, a reclassification - only if it applies, as `modify`,
    /// `revoke` and `reclassify` say; a fitting always. Returns whether it
    /// recorded it.
    pub fn enter(&mut self, recorded: Recorded) -> bool {
        let applies = match &recorded.entry {
            HistoryEntry::Modified { message: modified } => {
                self.is_changed(modified.id, |m| m.modify(modified))
            }
            HistoryEntry::Revoked { message, at } => self.is_changed(*message, |m| m.revoke(*at)),
            HistoryEntry::Reclassified { message, classification } => {
                self.is_changed(*message, |m| m.reclassify(classification))
            }
            HistoryEntry::Fitted { .. }
            | HistoryEntry::Said { .. }
            | HistoryEntry::Given { .. }
            | HistoryEntry::Withdrawn { .. }
            | HistoryEntry::TurnFinished { .. }
            | HistoryEntry::TurnGivenUp { .. }
            | HistoryEntry::PlanStepStarted { .. }
            | HistoryEntry::CommandExecuted { .. }
            | HistoryEntry::Compacted { .. }
            | HistoryEntry::ActivityFinished { .. }
            | HistoryEntry::ParticipantJoined { .. }
            | HistoryEntry::ParticipantLeft { .. }
            | HistoryEntry::HistoryShared { .. }
            | HistoryEntry::CommandFailed { .. }
            | HistoryEntry::RetryAccepted { .. }
            | HistoryEntry::RetryRejected { .. }
            | HistoryEntry::ChainCutOff { .. }
            | HistoryEntry::ResponderUndecided { .. }
            | HistoryEntry::MessageRejected { .. }
            | HistoryEntry::MessageRefused { .. }
            | HistoryEntry::ResponseRequested { .. }
            | HistoryEntry::CommandRequested { .. }
            | HistoryEntry::RetryRequested { .. }
            | HistoryEntry::ApprovalRequested { .. }
            | HistoryEntry::ApprovalAnswered { .. }
            | HistoryEntry::ApprovalDecided { .. }
            | HistoryEntry::ApprovalRequestWithdrawn { .. }
            | HistoryEntry::QuestionRequested { .. }
            | HistoryEntry::QuestionClosed { .. } => true,
        };
        if applies {
            self.push_with(recorded, digest::history_chain);
        }
        applies
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{items_of, messages_of};
    use super::*;
    use crate::actor::{RequestReason, RetryTarget};
    use crate::agent::{AgentId, RetryRefused};
    use crate::confidentiality::Confidentiality;
    use crate::message::testing::{id, make_message};

    #[test]
    fn a_turn_given_up_is_found_while_its_end_is_the_last_thing() {
        let (agent, other) = (ActorId::Agent(AgentId::new()), ActorId::Agent(AgentId::new()));
        let turn = TurnId::new();
        let mut history = History::default();
        history.push(make_message(id(1)));
        assert_eq!(history.turn_given_up_last(agent), None, "no turn ended");
        history.record(HistoryEntry::TurnGivenUp {
            turn_id: turn,
            actor: agent,
            reason: GivenUpReason::Left,
            at: Utc::now(),
        });
        assert_eq!(history.turn_given_up_last(agent), Some((turn, GivenUpReason::Left)));
        assert_eq!(history.turn_given_up_last(other), None, "another agent's");
        // What was only told or asked of it is no part of the conversation
        // (issue 417).
        history.record(HistoryEntry::RetryRejected {
            target: RetryTarget::Turn(turn),
            reason: RetryRefused::NotLast,
            actor: other,
            at: Utc::now(),
        });
        history.record(HistoryEntry::ResponseRequested {
            by: other,
            reason: RequestReason::Chosen,
            at: Utc::now(),
        });
        assert_eq!(
            history.turn_given_up_last(agent),
            Some((turn, GivenUpReason::Left)),
            "a notice and a request do not count"
        );
        history.push(make_message(id(2)));
        assert_eq!(history.turn_given_up_last(agent), None, "something followed it");
    }

    #[test]
    fn its_items_are_the_messages_as_they_stand_and_how_turns_ended() {
        let (finished, given_up) = (TurnId::new(), TurnId::new());
        let at = Utc::now();
        let mut history = History::default();
        history.push(make_message(id(1)));
        history.record(HistoryEntry::PlanStepStarted {
            turn_id: finished,
            actor: ActorId::default(),
            step_index: 0,
            total_steps: 1,
            description: "first".to_string(),
            at,
        });
        history.push(Message { turn_id: Some(finished), ..make_message(id(2)) });
        history.record(HistoryEntry::TurnFinished {
            turn_id: finished,
            actor: ActorId::default(),
            at,
        });
        history.modify(Message { content: Some("changed".to_string()), ..make_message(id(1)) });
        history.push(Message { turn_id: Some(given_up), ..make_message(id(3)) });
        history.record(HistoryEntry::TurnGivenUp {
            turn_id: given_up,
            actor: ActorId::default(),
            reason: GivenUpReason::Left,
            at,
        });
        history.fit_into(&Classification::CHANNEL_DEFAULT);

        let items = items_of(&history);
        let kinds: Vec<String> = items
            .iter()
            .map(|item| match item {
                HistoryEntry::Said { message, .. } => {
                    format!("said {}", message.content.clone().unwrap_or_default())
                }
                HistoryEntry::TurnFinished { turn_id, .. } => {
                    format!("finished {}", *turn_id == finished)
                }
                HistoryEntry::TurnGivenUp { turn_id, .. } => {
                    format!("given up {}", *turn_id == given_up)
                }
                HistoryEntry::PlanStepStarted { turn_id, description, .. } => {
                    format!("step {description} {}", *turn_id == finished)
                }
                other => format!("{other:?}"),
            })
            .collect();
        let text = |n: u128| format!("said {}", make_message(id(n)).content.unwrap_or_default());
        assert_eq!(kinds, ["said changed".to_string(), "step first true".to_string(), text(2), "finished true".to_string(), text(3), "given up true".to_string()],
            "in the order they happened, the change folded into its message, the fitting in every one");
        assert_eq!(messages_of(&history).len(), 3, "the messages alone, as before");
    }

    #[test]
    fn a_change_of_a_message_before_what_is_held_is_recorded_for_the_store() {
        let mut history = History::default();
        history.extend(vec![make_message(id(1)), make_message(id(2)), make_message(id(3))]);
        assert!(!history.revoke(id(9), Utc::now()), "a message it never held, whole");
        history.mark_stored();
        history.keep_from(Position::new(2));
        assert!(
            history.revoke(id(1), Utc::now()),
            "before what it holds: recorded, the store applies it"
        );
        assert!(
            history.modify(Message { content: Some("changed".to_string()), ..make_message(id(3)) }),
            "one it holds as before"
        );
        assert!(
            history.revoke(id(3), Utc::now()) && !history.revoke(id(3), Utc::now()),
            "and taken back once"
        );
        assert_eq!(history.end().get(), 6);
    }

    #[test]
    fn a_range_folded_gives_its_messages_as_the_whole_history_does() {
        let public = Classification::general(Confidentiality::Public);
        let mut history = History::default();
        history.extend(vec![make_message(id(1)), make_message(id(2))]);
        let from = history.end();
        history.extend(vec![make_message(id(3)), make_message(id(4))]);
        history.modify(Message { content: Some("changed".to_string()), ..make_message(id(3)) });
        history.revoke(id(1), Utc::now());
        history.fit_into(&public);
        history.reclassify(id(4), Classification::general(Confidentiality::Internal));

        let whole: Vec<Message> = fold(history.all_entries()).into_iter().map(|(_, m)| m).collect();
        let range: Vec<(Position, Message)> =
            fold(history.from(from)).into_iter().map(|(node, m)| (node.seq, m)).collect();
        assert_eq!(
            range.iter().map(|(seq, _)| seq.get()).collect::<Vec<_>>(),
            [2, 3],
            "the messages of the range, where they were said"
        );
        assert_eq!(
            serde_json::to_value(range.into_iter().map(|(_, m)| m).collect::<Vec<_>>()).unwrap(),
            serde_json::to_value(&whole[2..]).unwrap(),
            "as they stand"
        );
        assert_eq!(history.message(id(3)).and_then(|m| m.content).as_deref(), Some("changed"));
        assert_eq!(
            history.message(id(3)).map(|m| m.classification),
            Some(public),
            "fitted after it"
        );
        assert!(history.message(id(1)).is_some_and(|m| m.revoked.is_some()));
    }

    #[test]
    fn a_reclassification_is_recorded_and_read_into_the_messages_as_they_stand() {
        let public = Classification::general(Confidentiality::Public);
        let internal = Classification::general(Confidentiality::Internal);
        let said = |n, classification: &Classification| Message {
            classification: classification.clone(),
            ..make_message(id(n))
        };
        let mut history = History::default();
        history.extend(vec![said(1, &internal)]);
        let fork = history.digest();
        history.extend(vec![said(2, &internal)]);
        history.move_head(fork.as_ref());
        history.extend(vec![said(3, &internal)]);
        let before = history.digest();

        assert!(history.reclassify(id(2), public.clone()), "on the branch no longer active");
        assert!(!history.reclassify(id(9), public.clone()), "never said here");
        let mut revoked = history.clone();
        revoked.revoke(id(1), Utc::now());
        assert!(!revoked.reclassify(id(1), public.clone()), "taken back: nothing left to release");
        assert_ne!(history.digest(), before, "recorded: the digest covers it");
        assert_eq!(history.all_entries().len(), 4, "one entry");
        assert_eq!(
            history.message(id(2)).map(|m| m.classification),
            Some(public.clone()),
            "read as it stands, on whichever branch"
        );
        assert_eq!(
            messages_of(&history).iter().map(|m| m.id).collect::<Vec<_>>(),
            [id(1), id(3)],
            "the active path says what was said"
        );

        let entries = history.all_entries().len();
        history.fit_into(&public);
        history.fit_into(&public);
        assert_eq!(
            history.all_entries().len(),
            entries + 2,
            "recorded, also when nothing is left to fit"
        );
        assert!(messages_of(&history)
            .iter()
            .chain(history.message(id(2)).iter())
            .all(|m| m.classification == public));

        let loaded: History =
            serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        assert_eq!(
            messages_of(&loaded).iter().map(|m| &m.classification).collect::<Vec<_>>(),
            [&public, &public]
        );
    }

    #[test]
    fn what_was_recorded_since_the_history_was_stored_replays_onto_it() {
        let public = Classification::general(Confidentiality::Public);
        let mut history = History::default();
        history.extend(vec![make_message(id(1)), make_message(id(2))]);
        let mut stored: History =
            serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        history.mark_stored();

        history.reclassify(id(2), public.clone());
        history.fit_into(&public);
        let changes = history.changes().expect("changed");
        assert_eq!(changes.entries.len(), 2, "one entry each: {changes:?}");
        stored.apply(changes).unwrap();
        assert_eq!(stored.digest(), history.digest());
        assert!(messages_of(&stored).iter().all(|m| m.classification == public));
    }

    #[test]
    fn a_change_is_recorded_and_read_into_the_messages_as_they_stand() {
        let at = Utc::now();
        let modified = |n, content: &str| Message {
            content: Some(content.to_string()),
            timestamp: at,
            ..make_message(id(n))
        };
        let mut history = History::default();
        history.extend(vec![make_message(id(1))]);
        let fork = history.digest();
        history.extend(vec![make_message(id(2))]);
        history.move_head(fork.as_ref());
        history.extend(vec![make_message(id(3))]);
        let before = history.digest();

        assert!(history.modify(modified(1, "corrected")), "on the active path");
        assert!(history.revoke(id(2), at), "on a branch no longer active");
        assert!(!history.modify(modified(9, "never said")), "never said here");
        assert!(!history.modify(modified(2, "too late")), "taken back already");
        assert!(
            !history.modify(Message { sender_id: ActorId::System, ..make_message(id(3)) }),
            "by another sender"
        );
        assert_ne!(history.digest(), before, "recorded: the digest covers it");
        assert_eq!(history.all_entries().len(), 5, "one entry each, nothing rewritten");

        let messages = messages_of(&history);
        assert_eq!(
            messages.iter().map(|m| m.id).collect::<Vec<_>>(),
            [id(1), id(3)],
            "in their places"
        );
        assert_eq!(
            (messages[0].content.as_deref(), messages[0].modified),
            (Some("corrected"), Some(at))
        );
        let revoked = history.message(id(2)).expect("a message taken back is still found");
        assert!(revoked.content.is_none() && revoked.revoked == Some(at));

        let loaded: History =
            serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        assert_eq!(messages_of(&loaded)[0].content.as_deref(), Some("corrected"));
        assert_eq!(loaded.digest(), history.digest());
    }

    #[test]
    fn a_change_since_the_history_was_stored_replays_onto_it() {
        let mut history = History::default();
        history.extend(vec![make_message(id(1)), make_message(id(2))]);
        let mut stored: History =
            serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        history.mark_stored();

        history.revoke(id(2), Utc::now());
        stored.apply(history.changes().expect("changed")).unwrap();
        assert_eq!(stored.digest(), history.digest());
        assert!(stored.message(id(2)).is_some_and(|m| m.revoked.is_some()));
    }
}
