//! The history of a channel as an agent keeps it (issue 06): the chain of
//! what happened in it - messages said, each of the agent's own with what it
//! was produced from, reclassifications (issue 133), changes to what was
//! said (issue 234), where the steps of a plan began (issue 263) and how
//! turns ended (issue 258) - read into the messages as they stand (issue
//! 197).

use crate::actor::ActorId;
use crate::chain::{Chain, Node};
use crate::confidentiality::Classification;
use crate::context::ContextRef;
use crate::digest::Digest;
use crate::message::{Message, MessageId};
use crate::turn::GivenUpReason;
use crate::work::TurnId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::digest;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

/// What happened in a `History`; its node carries the digest of the
/// history up to and including it.
// Nearly every entry is a message said: boxing it would cost an allocation
// per message and save nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryEntry {
    /// A message said and, for one this agent produced, what it was
    /// produced from.
    Said { message: Message, context: Option<ContextRef> },
    /// The message `message` - on whichever branch - is classified
    /// `classification` from here on (issue 133).
    Reclassified { message: MessageId, classification: Classification },
    /// The message `message.id` - on whichever branch - reads as `message`
    /// from here on (issue 234, `Message::modify`).
    Modified { message: Message },
    /// The message `message` - on whichever branch - was taken back by its
    /// author at `at` (issue 234, `Message::revoke`).
    Revoked { message: MessageId, at: DateTime<Utc> },
    /// The channel was classified `channel`, and every message said so far
    /// fits into it from here on (`Classification::fitted_into`).
    Fitted { channel: Classification },
    /// The turn `turn_id` of `actor` ended at `at` and was taken over
    /// (issue 258).
    TurnFinished { turn_id: TurnId, actor: ActorId, at: DateTime<Utc> },
    /// The turn `turn_id` of `actor` was given up at `at`, for `reason`
    /// (issue 258): what it said before stays said. Whose turn it was is
    /// recorded, since a turn may end before it said anything.
    TurnGivenUp { turn_id: TurnId, actor: ActorId, reason: GivenUpReason, at: DateTime<Utc> },
    /// Step `step_index` of `total_steps` of the plan of `actor`'s turn
    /// `turn_id` began at `at` (issue 263): what the turn says from here on
    /// is said in that step, until the next one begins or the turn gives
    /// its final answer. `description` is the step as the plan words it.
    PlanStepStarted { turn_id: TurnId, actor: ActorId, step_index: usize, total_steps: usize, description: String, at: DateTime<Utc> },
    /// `actor`, an agent, carried out the command `command` with `args` in
    /// its turn `turn_id` at `at`, on the word of `issuer` (issue 269):
    /// recorded before the turn's end, without what it answered - a private
    /// answer is the issuer's, the history everybody's. A command refused
    /// or failed did nothing and leaves no entry; a command the session
    /// runs is never recorded.
    CommandExecuted { turn_id: TurnId, actor: ActorId, issuer: ActorId, command: String, args: String, at: DateTime<Utc> },
    /// `actor`, an agent, replaced its context in its turn `turn_id` at
    /// `at` (issue 16): `from`, the transcript as it stood, by `to`, the
    /// head of a branch of its own that begins anew - what replaces the
    /// older messages, then those kept. Every reference recorded before
    /// names the old branch, which stays; what is said afterwards is
    /// produced from the new one. What replaced the older messages follows
    /// it, as messages given to the agent. Kept in the agent's history
    /// alone: the transcript's digests do not record where a branch came
    /// from.
    Compacted { turn_id: TurnId, actor: ActorId, from: Digest, to: Digest, at: DateTime<Utc> },
    /// `actor`, an agent, ended `activity` - something of its own accord,
    /// besides answering - in its turn `turn_id` at `at` (issue 284), and
    /// `output` says what came of it: recorded where it happened, with what
    /// it said, which is everybody's.
    ActivityFinished { turn_id: TurnId, actor: ActorId, activity: String, output: String, at: DateTime<Utc> },
}

/// The conversation as it happened - a chain like a `Transcript`. What it
/// records is read from start to end into the messages as they stand: a
/// reclassification, a modification or a revocation applies to a message on
/// any branch, as a change made in the channel does, so it is folded over
/// every branch in the order things happened (`fold`). A message taken back
/// stays, without what it said (`Message::revoked`), so that whatever names
/// it by id finds it.
pub type History = Chain<HistoryEntry>;

/// The messages `nodes` say - a range of a history, in the order its
/// entries were added - each with the node that said it, as they stand
/// after every later node of the range (`HistoryEntry::fold_into`), in the
/// order they were said. A change always comes after what it changes, so a
/// range that goes on to the end of its history gives every message in it
/// as it stands; a change of a message before the range is left out.
pub fn fold<'a>(nodes: impl IntoIterator<Item = &'a Node<HistoryEntry>>) -> Vec<(&'a Node<HistoryEntry>, Message)> {
    let mut messages: Vec<(&Node<HistoryEntry>, Message)> = Vec::new();
    let mut at: HashMap<MessageId, usize> = HashMap::new();
    for node in nodes {
        match &node.entry {
            HistoryEntry::Said { message, .. } => {
                at.insert(message.id, messages.len());
                messages.push((node, message.clone()));
            }
            HistoryEntry::Fitted { .. } => {
                for (_, message) in &mut messages {
                    node.entry.fold_into(message);
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
    /// The items, oldest first.
    pub items: Vec<HistoryEntry>,
    /// The position of the oldest of `items`: what the page before this one
    /// is read before (`HistoryStore::page`); `None` if there is no older
    /// item to be had.
    pub earlier: Option<u64>,
    /// The digest of the history when the page was read: up to where it was
    /// read.
    pub digest: Option<Digest>,
}

impl Chain<HistoryEntry> {
    /// What the turn `turn_id` of `actor` said, as it stands, in the order
    /// it was said (issue 284): its messages recorded with the context they
    /// were produced from - not what it was given, nor what replaced older
    /// messages in a compaction -, on every branch, none taken back. Its
    /// answer, whatever a compaction left of it in the transcript.
    ///
    /// Read from where the turn began (`turn_began`) to the end, not from
    /// the start: what is read grows with how long ago the turn was.
    pub fn said_in_turn(&self, actor: ActorId, turn_id: TurnId) -> Vec<Message> {
        let Some(began) = self.turn_began(actor, turn_id) else { return Vec::new() };
        fold(self.from(began)).into_iter()
            .filter(|(node, message)| matches!(&node.entry, HistoryEntry::Said { context: Some(_), .. }) && message.turn_id == Some(turn_id) && message.revoked.is_none())
            .map(|(_, message)| message)
            .collect()
    }

    /// The position from which on everything the turn `turn_id` of `actor`
    /// recorded lies: after the end of the turn of `actor` before it, where
    /// what is held begins (`base`) if none came before there - the end
    /// before may lie before a window (issue 387). An actor is in one turn at a time, and a turn
    /// answered again is a new turn (issue 259), so nothing of the turn lies
    /// before that end. Read from the end backwards, up to the turn and the
    /// end before it; `None` if the turn recorded nothing - neither a message
    /// nor its end.
    fn turn_began(&self, actor: ActorId, turn_id: TurnId) -> Option<u64> {
        let mut reached = false;
        for node in self.all_entries().rev() {
            match &node.entry {
                HistoryEntry::TurnFinished { turn_id: ended, actor: of, .. } | HistoryEntry::TurnGivenUp { turn_id: ended, actor: of, .. } if *of == actor => {
                    if *ended == turn_id {
                        reached = true;
                    } else if reached {
                        return Some(node.seq + 1);
                    }
                }
                HistoryEntry::Said { message, .. } if message.turn_id == Some(turn_id) => reached = true,
                _ => {}
            }
        }
        reached.then_some(self.base())
    }

    /// The turn of `author` given up, and why, if its end is the last thing
    /// on the active path (issue 259): what may be asked for anew as long as
    /// nothing followed it.
    pub fn turn_given_up_last(&self, author: ActorId) -> Option<(TurnId, GivenUpReason)> {
        match self.last().map(|node| &node.entry) {
            Some(HistoryEntry::TurnGivenUp { turn_id, actor, reason, .. }) if *actor == author => Some((*turn_id, reason.clone())),
            _ => None,
        }
    }

    /// The message `id` as it stands - on whichever branch - or `None` if
    /// it was never said here. Folded from where it was said on, not from
    /// the start.
    pub fn message(&self, id: MessageId) -> Option<Message> {
        let said = self.all_entries().rev().find(|node| node.entry.said().is_some_and(|m| m.id == id))?;
        fold(self.from(said.seq)).into_iter().next().map(|(_, message)| message)
    }

    /// The id of the last message said on the active path,
    /// `MessageId::default()` if none was.
    pub fn last_message_id(&self) -> MessageId {
        self.entries().rev().find_map(|n| n.entry.said()).map(|m| m.id).unwrap_or_default()
    }

    /// Appends messages that were not produced from a context of this
    /// history's owner - what others said.
    pub fn extend(&mut self, msgs: Vec<Message>) {
        for m in msgs {
            self.push(m, None);
        }
    }

    /// Appends `msg` with what it was produced from, if known.
    pub fn push(&mut self, msg: Message, context: Option<ContextRef>) {
        self.record(HistoryEntry::Said { message: msg, context });
    }

    /// Records that message `id` - on whichever branch - is classified
    /// `classification` from now on (issue 133). `false`, and nothing is
    /// recorded, if there is no such message or it was taken back
    /// (`Message::reclassify`); one before what it holds is recorded
    /// (`is_changed`).
    pub fn reclassify(&mut self, id: MessageId, classification: Classification) -> bool {
        if !self.is_changed(id, |m| m.reclassify(&classification)) {
            return false;
        }
        self.append(HistoryEntry::Reclassified { message: id, classification });
        true
    }

    /// Records that the message `modified.id` - on whichever branch -
    /// reads as `modified` from now on (issue 234). `false`, and nothing is
    /// recorded, if there is no such message or it cannot be modified so as
    /// it stands (`Message::modify`): taken back, or by another sender; one
    /// before what it holds is recorded (`is_changed`).
    pub fn modify(&mut self, modified: Message) -> bool {
        if !self.is_changed(modified.id, |m| m.modify(&modified)) {
            return false;
        }
        self.append(HistoryEntry::Modified { message: modified });
        true
    }

    /// Records that the message `id` - on whichever branch - was taken back
    /// at `at` (issue 234). `false`, and nothing is recorded, if there is no
    /// such message or it was taken back already; one before what it holds
    /// is recorded (`is_changed`).
    pub fn revoke(&mut self, id: MessageId, at: DateTime<Utc>) -> bool {
        if !self.is_changed(id, |m| m.revoke(at)) {
            return false;
        }
        self.append(HistoryEntry::Revoked { message: id, at });
        true
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
            None => self.base() > 0,
        }
    }

    /// Records that the channel is classified `channel` now, which every
    /// message said so far - on every branch: all of them were said in the
    /// channel - fits into from now on (`Classification::fitted_into`).
    /// Recorded whether or not it changes a message: telling which would
    /// fold the whole history.
    pub fn fit_into(&mut self, channel: &Classification) {
        self.append(HistoryEntry::Fitted { channel: channel.clone() });
    }

    /// Records `item`, what happened that is no change of a message (`is_item`):
    /// a message said - as `push` records it, with what it was produced
    /// from -, how a turn ended (issue 258), after its messages, where a step
    /// of its plan began (issue 263), before the messages of the step, a
    /// command carried out (issue 269) or an activity ended (issue 284),
    /// before the turn's end, a compaction (issue 16), before what replaced
    /// the older messages. A change is recorded by its own method -
    /// `modify`, `revoke`, `reclassify`, `fit_into` -, which says whether it
    /// applies.
    pub fn record(&mut self, item: HistoryEntry) {
        debug_assert!(item.is_item(), "a change is recorded by its own method");
        self.append(item);
    }

    /// Appends `entry`, whatever it is, with its digest.
    fn append(&mut self, entry: HistoryEntry) {
        self.push_with(entry, digest::history_chain);
    }
}

impl HistoryEntry {
    /// Whether it is an item of its history - a message said, the start of a
    /// plan's step, a command carried out, an activity ended, a compaction, a
    /// turn's end - rather than a change of what was said before: what a
    /// reader of the history is given as it stands (`StandingItems`), a
    /// change being in the message it changes.
    pub(crate) fn is_item(&self) -> bool {
        match self {
            HistoryEntry::Said { .. } | HistoryEntry::PlanStepStarted { .. } | HistoryEntry::TurnFinished { .. } | HistoryEntry::TurnGivenUp { .. }
            | HistoryEntry::CommandExecuted { .. } | HistoryEntry::Compacted { .. } | HistoryEntry::ActivityFinished { .. } => true,
            HistoryEntry::Reclassified { .. } | HistoryEntry::Modified { .. } | HistoryEntry::Revoked { .. } | HistoryEntry::Fitted { .. } => false,
        }
    }

    /// The message, if this is one said.
    pub fn said(&self) -> Option<&Message> {
        match self {
            HistoryEntry::Said { message, .. } => Some(message),
            HistoryEntry::Reclassified { .. } | HistoryEntry::Modified { .. } | HistoryEntry::Revoked { .. } | HistoryEntry::Fitted { .. }
            | HistoryEntry::TurnFinished { .. } | HistoryEntry::TurnGivenUp { .. } | HistoryEntry::PlanStepStarted { .. } | HistoryEntry::CommandExecuted { .. }
            | HistoryEntry::Compacted { .. } | HistoryEntry::ActivityFinished { .. } => None,
        }
    }

    /// Applies what it changes to `standing`, a message said before it as it
    /// stands, and says whether that changed it: a modification, a
    /// revocation or a reclassification of that message
    /// (`Message::modify`, `revoke`, `reclassify`), and a fitting of the
    /// channel's, which applies to every message before it
    /// (`Classification::fitted_into`). Every other entry changes nothing.
    /// The one rule by which what stands is read from a history.
    pub fn fold_into(&self, standing: &mut Message) -> bool {
        match self {
            HistoryEntry::Modified { message } if message.id == standing.id => standing.modify(message),
            HistoryEntry::Revoked { message, at } if *message == standing.id => standing.revoke(*at),
            HistoryEntry::Reclassified { message, classification } if *message == standing.id => standing.reclassify(classification),
            HistoryEntry::Fitted { channel } => {
                let fitted = standing.classification.fitted_into(channel);
                let changed = fitted != standing.classification;
                standing.classification = fitted;
                changed
            }
            HistoryEntry::Said { .. } | HistoryEntry::Modified { .. } | HistoryEntry::Revoked { .. } | HistoryEntry::Reclassified { .. }
            | HistoryEntry::TurnFinished { .. } | HistoryEntry::TurnGivenUp { .. } | HistoryEntry::PlanStepStarted { .. } | HistoryEntry::CommandExecuted { .. }
            | HistoryEntry::Compacted { .. } | HistoryEntry::ActivityFinished { .. } => false,
        }
    }

    /// The message it modifies, takes back or reclassifies, if it does.
    pub(crate) fn changed(&self) -> Option<MessageId> {
        match self {
            HistoryEntry::Modified { message } => Some(message.id),
            HistoryEntry::Revoked { message, .. } | HistoryEntry::Reclassified { message, .. } => Some(*message),
            HistoryEntry::Said { .. } | HistoryEntry::Fitted { .. } | HistoryEntry::TurnFinished { .. } | HistoryEntry::TurnGivenUp { .. }
            | HistoryEntry::PlanStepStarted { .. } | HistoryEntry::CommandExecuted { .. } | HistoryEntry::Compacted { .. } | HistoryEntry::ActivityFinished { .. } => None,
        }
    }

    /// What the message it says was produced from, and in which turn, if
    /// this is one said and both are known: whether what its author saw can
    /// be shown (issue 207).
    pub fn context(&self) -> Option<(&ContextRef, TurnId)> {
        match self {
            HistoryEntry::Said { message, context } => Some((context.as_ref()?, message.turn_id?)),
            HistoryEntry::Reclassified { .. } | HistoryEntry::Modified { .. } | HistoryEntry::Revoked { .. } | HistoryEntry::Fitted { .. }
            | HistoryEntry::TurnFinished { .. } | HistoryEntry::TurnGivenUp { .. } | HistoryEntry::PlanStepStarted { .. } | HistoryEntry::CommandExecuted { .. }
            | HistoryEntry::Compacted { .. } | HistoryEntry::ActivityFinished { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidentiality::Confidentiality;
    use crate::digest::message_chain;
    use crate::message::tests::{id, make_message};
    use crate::agent::AgentId;
    use super::testing::{items_of, messages_of, recorded_context};

    #[test]
    fn a_turn_given_up_is_found_while_its_end_is_the_last_thing() {
        let (agent, other) = (ActorId::Agent(AgentId::new()), ActorId::Agent(AgentId::new()));
        let turn = TurnId::new();
        let mut history = History::default();
        history.push(make_message(id(1)), None);
        assert_eq!(history.turn_given_up_last(agent), None, "no turn ended");
        history.record(HistoryEntry::TurnGivenUp { turn_id: turn, actor: agent, reason: GivenUpReason::Left, at: Utc::now() });
        assert_eq!(history.turn_given_up_last(agent), Some((turn, GivenUpReason::Left)));
        assert_eq!(history.turn_given_up_last(other), None, "another agent's");
        history.push(make_message(id(2)), None);
        assert_eq!(history.turn_given_up_last(agent), None, "something followed it");
    }

    #[test]
    fn its_items_are_the_messages_as_they_stand_and_how_turns_ended() {
        let (finished, given_up) = (TurnId::new(), TurnId::new());
        let at = Utc::now();
        let mut history = History::default();
        history.push(make_message(id(1)), None);
        history.record(HistoryEntry::PlanStepStarted { turn_id: finished, actor: ActorId::default(), step_index: 0, total_steps: 1, description: "first".to_string(), at });
        history.push(Message { turn_id: Some(finished), ..make_message(id(2)) }, None);
        history.record(HistoryEntry::TurnFinished { turn_id: finished, actor: ActorId::default(), at });
        history.modify(Message { content: Some("changed".to_string()), ..make_message(id(1)) });
        history.push(Message { turn_id: Some(given_up), ..make_message(id(3)) }, None);
        history.record(HistoryEntry::TurnGivenUp { turn_id: given_up, actor: ActorId::default(), reason: GivenUpReason::Left, at });
        history.fit_into(&Classification::CHANNEL_DEFAULT);

        let items = items_of(&history);
        let kinds: Vec<String> = items.iter().map(|item| match item {
            HistoryEntry::Said { message, .. } => format!("said {}", message.content.clone().unwrap_or_default()),
            HistoryEntry::TurnFinished { turn_id, .. } => format!("finished {}", *turn_id == finished),
            HistoryEntry::TurnGivenUp { turn_id, .. } => format!("given up {}", *turn_id == given_up),
            HistoryEntry::PlanStepStarted { turn_id, description, .. } => format!("step {description} {}", *turn_id == finished),
            other => format!("{other:?}"),
        }).collect();
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
        history.keep_from(2);
        assert!(history.revoke(id(1), Utc::now()), "before what it holds: recorded, the store applies it");
        assert!(history.modify(Message { content: Some("changed".to_string()), ..make_message(id(3)) }), "one it holds as before");
        assert!(history.revoke(id(3), Utc::now()) && !history.revoke(id(3), Utc::now()), "and taken back once");
        assert_eq!(history.end(), 6);
    }

    #[test]
    fn what_a_turn_said_is_what_it_produced_as_it_stands() {
        let turn = TurnId::new();
        let of_turn = |n: u128| Message { turn_id: Some(turn), ..make_message(id(n)) };
        let mut history = History::default();
        history.push(make_message(id(1)), None);
        history.push(of_turn(2), Some(ContextRef::default()));
        // Given to it, or replacing older messages in a compaction: no context.
        history.push(of_turn(3), None);
        history.push(of_turn(4), Some(ContextRef::default()));
        history.push(Message { turn_id: Some(TurnId::new()), ..make_message(id(5)) }, Some(ContextRef::default()));
        history.push(of_turn(6), Some(ContextRef::default()));
        history.modify(Message { content: Some("changed".to_string()), ..of_turn(2) });
        assert!(history.revoke(id(4), Utc::now()));

        let said: Vec<(MessageId, Option<String>)> = history.said_in_turn(ActorId::default(), turn).into_iter().map(|m| (m.id, m.content)).collect();
        assert_eq!(said, [(id(2), Some("changed".to_string())), (id(6), make_message(id(6)).content)], "its own, as they stand, none taken back");
    }

    #[test]
    fn what_a_turn_said_is_read_from_the_end_of_its_actors_turn_before() {
        let (agent, other) = (ActorId::Agent(AgentId::new()), ActorId::Agent(AgentId::new()));
        let (before, turn, theirs) = (TurnId::new(), TurnId::new(), TurnId::new());
        let at = Utc::now();
        let said = |turn: TurnId, n: u128| Message { turn_id: Some(turn), ..make_message(id(n)) };
        let ended = |turn_id, actor| HistoryEntry::TurnFinished { turn_id, actor, at };
        let mut history = History::default();
        history.push(said(before, 1), Some(ContextRef::default()));
        history.record(ended(before, agent));
        let began = history.end();
        assert!(history.said_in_turn(agent, turn).is_empty(), "nothing said yet");

        history.push(said(turn, 2), Some(ContextRef::default()));
        history.push(said(theirs, 3), Some(ContextRef::default()));
        // Another actor's turn ends in between: no end of this actor's.
        history.record(ended(theirs, other));
        history.push(said(turn, 4), Some(ContextRef::default()));
        assert_eq!(history.turn_began(agent, turn), Some(began), "after the end of its turn before");
        assert_eq!(history.said_in_turn(agent, turn).iter().map(|m| m.id).collect::<Vec<_>>(), [id(2), id(4)], "past the other actor's turn end");

        history.record(ended(turn, agent));
        history.push(said(TurnId::new(), 5), Some(ContextRef::default()));
        history.record(ended(TurnId::new(), agent));
        assert!(history.revoke(id(2), at), "changed after later turns");
        assert_eq!(history.said_in_turn(agent, turn).iter().map(|m| m.id).collect::<Vec<_>>(), [id(4)], "an older turn, as it stands");
        assert_eq!(history.said_in_turn(agent, before).iter().map(|m| m.id).collect::<Vec<_>>(), [id(1)], "the first, from the start");
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
        let range: Vec<(u64, Message)> = fold(history.from(from)).into_iter().map(|(node, m)| (node.seq, m)).collect();
        assert_eq!(range.iter().map(|(seq, _)| *seq).collect::<Vec<_>>(), [2, 3], "the messages of the range, where they were said");
        assert_eq!(serde_json::to_value(range.into_iter().map(|(_, m)| m).collect::<Vec<_>>()).unwrap(), serde_json::to_value(&whole[2..]).unwrap(), "as they stand");
        assert_eq!(history.message(id(3)).and_then(|m| m.content).as_deref(), Some("changed"));
        assert_eq!(history.message(id(3)).map(|m| m.classification), Some(public), "fitted after it");
        assert!(history.message(id(1)).is_some_and(|m| m.revoked.is_some()));
    }

    #[test]
    fn an_entry_folds_into_the_message_it_changes_and_says_whether_it_did() {
        let at = Utc::now();
        let mut message = make_message(id(1));
        assert!(!HistoryEntry::Revoked { message: id(2), at }.fold_into(&mut message), "another message");
        assert!(!HistoryEntry::Said { message: make_message(id(1)), context: None }.fold_into(&mut message), "says, changes nothing");
        assert!(HistoryEntry::Modified { message: Message { content: Some("changed".to_string()), ..make_message(id(1)) } }.fold_into(&mut message));
        assert_eq!(message.content.as_deref(), Some("changed"));
        let fitted = HistoryEntry::Fitted { channel: Classification::general(Confidentiality::Public) };
        assert!(fitted.fold_into(&mut message), "lowered to the channel's");
        assert!(!fitted.fold_into(&mut message), "fits already");
        assert!(HistoryEntry::Revoked { message: id(1), at }.fold_into(&mut message));
        assert!(!HistoryEntry::Revoked { message: id(1), at }.fold_into(&mut message), "taken back already");
    }

    #[test]
    fn a_history_keeps_what_its_messages_were_produced_from() {
        let reference = ContextRef { transcript: Some(message_chain(None, &make_message(id(9)))), prompts: Vec::new(), tools: None, knowledge: Vec::new(), profile: Default::default() };
        let turn = TurnId::new();
        let mut history = History::default();
        history.extend(vec![make_message(id(1))]);
        history.push(Message { turn_id: Some(turn), ..make_message(id(2)) }, Some(reference.clone()));

        let loaded: History = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        let entries: Vec<_> = loaded.all_entries().collect();
        assert_eq!(entries[0].entry.context(), None, "said by someone else");
        assert_eq!(entries[1].entry.context(), Some((&reference, turn)), "with the turn it was produced in");
        assert_eq!(recorded_context(&loaded, id(2)), Some((reference.clone(), turn)), "found by its message");
        assert_eq!(recorded_context(&loaded, id(1)), None);
        assert_eq!(loaded.digest(), history.digest());
    }

    #[test]
    fn a_reclassification_is_recorded_and_read_into_the_messages_as_they_stand() {
        let public = Classification::general(Confidentiality::Public);
        let internal = Classification::general(Confidentiality::Internal);
        let said = |n, classification: &Classification| Message { classification: classification.clone(), ..make_message(id(n)) };
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
        assert_eq!(history.message(id(2)).map(|m| m.classification), Some(public.clone()), "read as it stands, on whichever branch");
        assert_eq!(messages_of(&history).iter().map(|m| m.id).collect::<Vec<_>>(), [id(1), id(3)], "the active path says what was said");

        let entries = history.all_entries().len();
        history.fit_into(&public);
        history.fit_into(&public);
        assert_eq!(history.all_entries().len(), entries + 2, "recorded, also when nothing is left to fit");
        assert!(messages_of(&history).iter().chain(history.message(id(2)).iter()).all(|m| m.classification == public));
        assert_eq!(history.last_message_id(), id(3), "the last message said, not the last entry");

        let loaded: History = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        assert_eq!(messages_of(&loaded).iter().map(|m| &m.classification).collect::<Vec<_>>(), [&public, &public]);
    }

    #[test]
    fn what_was_recorded_since_the_history_was_stored_replays_onto_it() {
        let public = Classification::general(Confidentiality::Public);
        let mut history = History::default();
        history.extend(vec![make_message(id(1)), make_message(id(2))]);
        let mut stored: History = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
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
        let modified = |n, content: &str| Message { content: Some(content.to_string()), timestamp: at, ..make_message(id(n)) };
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
        assert!(!history.modify(Message { sender_id: ActorId::System, ..make_message(id(3)) }), "by another sender");
        assert_ne!(history.digest(), before, "recorded: the digest covers it");
        assert_eq!(history.all_entries().len(), 5, "one entry each, nothing rewritten");

        let messages = messages_of(&history);
        assert_eq!(messages.iter().map(|m| m.id).collect::<Vec<_>>(), [id(1), id(3)], "in their places");
        assert_eq!((messages[0].content.as_deref(), messages[0].modified), (Some("corrected"), Some(at)));
        let revoked = history.message(id(2)).expect("a message taken back is still found");
        assert!(revoked.content.is_none() && revoked.revoked == Some(at));
        assert_eq!(history.last_message_id(), id(3), "a change is no message said");

        let loaded: History = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        assert_eq!(messages_of(&loaded)[0].content.as_deref(), Some("corrected"));
        assert_eq!(loaded.digest(), history.digest());
    }

    #[test]
    fn a_change_since_the_history_was_stored_replays_onto_it() {
        let mut history = History::default();
        history.extend(vec![make_message(id(1)), make_message(id(2))]);
        let mut stored: History = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        history.mark_stored();

        history.revoke(id(2), Utc::now());
        stored.apply(history.changes().expect("changed")).unwrap();
        assert_eq!(stored.digest(), history.digest());
        assert!(stored.message(id(2)).is_some_and(|m| m.revoked.is_some()));
    }
}
