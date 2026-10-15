//! The history of a channel as an agent keeps it (issue 06): the chain of
//! what happened in it - messages said, each of the agent's own with what it
//! was produced from, reclassifications (issue 133), changes to what was
//! said (issue 234), where the steps of a plan began (issue 263) and how
//! turns ended (issue 258) - read into the messages as they stand (issue
//! 197).

use crate::actor::ActorId;
use crate::chain::Chain;
use crate::confidentiality::Classification;
use crate::context::ContextRef;
use crate::digest::Digest;
use crate::work::TurnId;
use crate::message::{Message, MessageId};
use crate::turn::GivenUpReason;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// What happened in a `MessageHistory`; its node carries the digest of the
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
/// every branch in the order things happened. A message taken back stays, without what it
/// said (`Message::revoked`), so that whatever names it by id finds it.
pub type MessageHistory = Chain<HistoryEntry>;

impl Chain<HistoryEntry> {
    /// The messages of the active path as they stand.
    pub fn messages(&self) -> Vec<Message> {
        let active: HashSet<MessageId> = self.entries().filter_map(|n| n.entry.said()).map(|m| m.id).collect();
        self.fold().into_iter().filter(|m| active.contains(&m.id)).collect()
    }

    /// The history of the active path as it stands, in the order it
    /// happened (issue 258): the messages said - `Said`, each as it stands,
    /// with every change since folded into it -, where the steps of a plan
    /// began - `PlanStepStarted` (issue 263) -, the commands carried out -
    /// `CommandExecuted` (issue 269) -, the activities of agents ended -
    /// `ActivityFinished` (issue 284) - and how turns ended - `TurnFinished`
    /// and `TurnGivenUp`; in an agent's history its compactions, `Compacted`.
    /// No other entry occurs: what changes
    /// a message is in the message, a fitting in every message.
    pub fn items(&self) -> Vec<HistoryEntry> {
        let mut folded: HashMap<Digest, Message> = self.fold_nodes().into_iter().collect();
        self.entries()
            .filter_map(|node| match &node.entry {
                HistoryEntry::Said { context, .. } => folded.remove(&node.digest)
                    .map(|message| HistoryEntry::Said { message, context: context.clone() }),
                turn @ (HistoryEntry::PlanStepStarted { .. } | HistoryEntry::TurnFinished { .. } | HistoryEntry::TurnGivenUp { .. } | HistoryEntry::CommandExecuted { .. } | HistoryEntry::Compacted { .. } | HistoryEntry::ActivityFinished { .. }) => Some(turn.clone()),
                HistoryEntry::Reclassified { .. } | HistoryEntry::Modified { .. } | HistoryEntry::Revoked { .. } | HistoryEntry::Fitted { .. } => None,
            })
            .collect()
    }

    /// What the turn `turn_id` said, as it stands, in the order it was said
    /// (issue 284): its messages recorded with the context they were
    /// produced from - not what it was given, nor what replaced older
    /// messages in a compaction -, on every branch, none taken back. Its
    /// answer, whatever a compaction left of it in the transcript.
    pub fn said_in_turn(&self, turn_id: TurnId) -> Vec<Message> {
        let produced: HashSet<Digest> = self.all_entries()
            .filter(|node| matches!(&node.entry, HistoryEntry::Said { message, context: Some(_) } if message.turn_id == Some(turn_id)))
            .map(|node| node.digest)
            .collect();
        self.fold_nodes().into_iter()
            .filter(|(digest, message)| produced.contains(digest) && message.revoked.is_none())
            .map(|(_, message)| message)
            .collect()
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
    /// it was never said here.
    pub fn message(&self, id: MessageId) -> Option<Message> {
        self.fold().into_iter().find(|m| m.id == id)
    }

    /// The messages of every branch as they stand, in the order they were
    /// said: every message said, with every reclassification, modification
    /// and revocation since applied to it.
    fn fold(&self) -> Vec<Message> {
        self.fold_nodes().into_iter().map(|(_, message)| message).collect()
    }

    /// `fold`, each message with the digest of the node that said it.
    fn fold_nodes(&self) -> Vec<(Digest, Message)> {
        let mut messages: Vec<(Digest, Message)> = Vec::new();
        let mut at: HashMap<MessageId, usize> = HashMap::new();
        for node in self.all_entries() {
            match &node.entry {
                HistoryEntry::Said { message, .. } => {
                    at.insert(message.id, messages.len());
                    messages.push((node.digest, message.clone()));
                }
                HistoryEntry::Reclassified { message, classification } => {
                    if let Some(&i) = at.get(message) {
                        messages[i].1.reclassify(classification);
                    }
                }
                HistoryEntry::Modified { message } => {
                    if let Some(&i) = at.get(&message.id) {
                        messages[i].1.modify(message);
                    }
                }
                HistoryEntry::Revoked { message, at: when } => {
                    if let Some(&i) = at.get(message) {
                        messages[i].1.revoke(*when);
                    }
                }
                HistoryEntry::Fitted { channel } => {
                    for (_, message) in &mut messages {
                        message.classification = message.classification.fitted_into(channel);
                    }
                }
                HistoryEntry::PlanStepStarted { .. } | HistoryEntry::TurnFinished { .. } | HistoryEntry::TurnGivenUp { .. } | HistoryEntry::CommandExecuted { .. } | HistoryEntry::Compacted { .. } | HistoryEntry::ActivityFinished { .. } => {}
            }
        }
        messages
    }

    /// The messages of the active path as they stand that came after the
    /// message `last` - all of them if `last` is `None` or not on it.
    pub fn messages_after(&self, last: Option<MessageId>) -> Vec<Message> {
        let messages = self.messages();
        let after = last.and_then(|last| messages.iter().position(|m| m.id == last)).map_or(0, |at| at + 1);
        messages.into_iter().skip(after).collect()
    }

    /// The messages of `visible` after the last of them the history has,
    /// and not in it - on any branch: what an actor that joins missed (issue
    /// 190), for one that joins again what was said while it was away. A
    /// history is only ever appended to, so older messages that a wider
    /// history grants on rejoining would land after the actor's own later
    /// answers; they are left out, and the history stays in the order things
    /// were said.
    pub fn missing_from(&self, visible: Vec<Message>) -> Vec<Message> {
        let known: HashSet<MessageId> = self.all_entries().filter_map(|n| n.entry.said()).map(|m| m.id).collect();
        let after = visible.iter().rposition(|m| known.contains(&m.id)).map_or(0, |last| last + 1);
        visible.into_iter().skip(after).filter(|m| !known.contains(&m.id)).collect()
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
    /// (`Message::reclassify`).
    pub fn reclassify(&mut self, id: MessageId, classification: Classification) -> bool {
        if !self.message(id).is_some_and(|mut m| m.reclassify(&classification)) {
            return false;
        }
        self.record(HistoryEntry::Reclassified { message: id, classification });
        true
    }

    /// Records that the message `modified.id` - on whichever branch -
    /// reads as `modified` from now on (issue 234). `false`, and nothing is
    /// recorded, if there is no such message or it cannot be modified so as
    /// it stands (`Message::modify`): taken back, or by another sender.
    pub fn modify(&mut self, modified: Message) -> bool {
        if !self.message(modified.id).is_some_and(|mut m| m.modify(&modified)) {
            return false;
        }
        self.record(HistoryEntry::Modified { message: modified });
        true
    }

    /// Records that the message `id` - on whichever branch - was taken back
    /// at `at` (issue 234). `false`, and nothing is recorded, if there is no
    /// such message or it was taken back already.
    pub fn revoke(&mut self, id: MessageId, at: DateTime<Utc>) -> bool {
        if !self.message(id).is_some_and(|mut m| m.revoke(at)) {
            return false;
        }
        self.record(HistoryEntry::Revoked { message: id, at });
        true
    }

    /// Records that the channel is classified `channel` now, which every
    /// message said so far - on every branch: all of them were said in the
    /// channel - fits into from now on (`Classification::fitted_into`).
    /// Returns how many messages that changes.
    pub fn fit_into(&mut self, channel: &Classification) -> usize {
        let changed = self.fold().iter().filter(|m| m.classification.fitted_into(channel) != m.classification).count();
        self.record(HistoryEntry::Fitted { channel: channel.clone() });
        changed
    }

    /// Records how a turn ended (issue 258): `TurnFinished` or
    /// `TurnGivenUp`, after its messages.
    pub fn end_turn(&mut self, end: HistoryEntry) {
        debug_assert!(matches!(end, HistoryEntry::TurnFinished { .. } | HistoryEntry::TurnGivenUp { .. }), "only the end of a turn");
        self.record(end);
    }

    /// Records that a step of a turn's plan began (issue 263):
    /// `PlanStepStarted`, before the messages of the step.
    pub fn start_plan_step(&mut self, start: HistoryEntry) {
        debug_assert!(matches!(start, HistoryEntry::PlanStepStarted { .. }), "only the start of a plan step");
        self.record(start);
    }

    /// Records that a command was carried out in a turn (issue 269):
    /// `CommandExecuted`, before the turn's end.
    pub fn record_command(&mut self, executed: HistoryEntry) {
        debug_assert!(matches!(executed, HistoryEntry::CommandExecuted { .. }), "only a command carried out");
        self.record(executed);
    }

    /// Records that an agent ended an activity of its own (issue 284):
    /// `ActivityFinished`, before the turn's end.
    pub fn record_activity(&mut self, finished: HistoryEntry) {
        debug_assert!(matches!(finished, HistoryEntry::ActivityFinished { .. }), "only an activity ended");
        self.record(finished);
    }

    /// Records that the agent replaced its context (issue 16):
    /// `Compacted`, before what replaced its older messages.
    pub fn record_compaction(&mut self, compacted: HistoryEntry) {
        debug_assert!(matches!(compacted, HistoryEntry::Compacted { .. }), "only a compaction");
        self.record(compacted);
    }

    fn record(&mut self, entry: HistoryEntry) {
        self.push_with(entry, crate::digest::history_chain);
    }
}

impl HistoryEntry {
    /// The message, if this is one said.
    pub fn said(&self) -> Option<&Message> {
        match self {
            HistoryEntry::Said { message, .. } => Some(message),
            HistoryEntry::Reclassified { .. } | HistoryEntry::Modified { .. } | HistoryEntry::Revoked { .. } | HistoryEntry::Fitted { .. }
            | HistoryEntry::TurnFinished { .. } | HistoryEntry::TurnGivenUp { .. } | HistoryEntry::PlanStepStarted { .. } | HistoryEntry::CommandExecuted { .. }
            | HistoryEntry::Compacted { .. } | HistoryEntry::ActivityFinished { .. } => None,
        }
    }

    /// What message `id` was produced from, and in which turn (issue 241),
    /// if this is it and both are known.
    pub fn context_of(&self, id: MessageId) -> Option<(&ContextRef, TurnId)> {
        match self {
            HistoryEntry::Said { message, context } if message.id == id => Some((context.as_ref()?, message.turn_id?)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidentiality::Confidentiality;
    use crate::digest::message_chain;
    use crate::message::tests::{id, make_message};

    #[test]
    fn a_turn_given_up_is_found_while_its_end_is_the_last_thing() {
        let (agent, other) = (ActorId::Agent(crate::agent::AgentId::new()), ActorId::Agent(crate::agent::AgentId::new()));
        let turn = TurnId::new();
        let mut history = MessageHistory::default();
        history.push(make_message(id(1)), None);
        assert_eq!(history.turn_given_up_last(agent), None, "no turn ended");
        history.end_turn(HistoryEntry::TurnGivenUp { turn_id: turn, actor: agent, reason: GivenUpReason::Stopped, at: Utc::now() });
        assert_eq!(history.turn_given_up_last(agent), Some((turn, GivenUpReason::Stopped)));
        assert_eq!(history.turn_given_up_last(other), None, "another agent's");
        history.push(make_message(id(2)), None);
        assert_eq!(history.turn_given_up_last(agent), None, "something followed it");
    }

    #[test]
    fn its_items_are_the_messages_as_they_stand_and_how_turns_ended() {
        let (finished, given_up) = (TurnId::new(), TurnId::new());
        let at = Utc::now();
        let mut history = MessageHistory::default();
        history.push(make_message(id(1)), None);
        history.start_plan_step(HistoryEntry::PlanStepStarted { turn_id: finished, actor: ActorId::default(), step_index: 0, total_steps: 1, description: "first".to_string(), at });
        history.push(Message { turn_id: Some(finished), ..make_message(id(2)) }, None);
        history.end_turn(HistoryEntry::TurnFinished { turn_id: finished, actor: ActorId::default(), at });
        history.modify(Message { content: Some("changed".to_string()), ..make_message(id(1)) });
        history.push(Message { turn_id: Some(given_up), ..make_message(id(3)) }, None);
        history.end_turn(HistoryEntry::TurnGivenUp { turn_id: given_up, actor: ActorId::default(), reason: GivenUpReason::Stopped, at });
        history.fit_into(&Classification::CHANNEL_DEFAULT);

        let items = history.items();
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
        assert_eq!(history.messages().len(), 3, "the messages alone, as before");
    }

    #[test]
    fn what_a_turn_said_is_what_it_produced_as_it_stands() {
        let turn = TurnId::new();
        let of_turn = |n: u128| Message { turn_id: Some(turn), ..make_message(id(n)) };
        let mut history = MessageHistory::default();
        history.push(make_message(id(1)), None);
        history.push(of_turn(2), Some(ContextRef::default()));
        // Given to it, or replacing older messages in a compaction: no context.
        history.push(of_turn(3), None);
        history.push(of_turn(4), Some(ContextRef::default()));
        history.push(Message { turn_id: Some(TurnId::new()), ..make_message(id(5)) }, Some(ContextRef::default()));
        history.push(of_turn(6), Some(ContextRef::default()));
        history.modify(Message { content: Some("changed".to_string()), ..of_turn(2) });
        assert!(history.revoke(id(4), Utc::now()));

        let said: Vec<(MessageId, Option<String>)> = history.said_in_turn(turn).into_iter().map(|m| (m.id, m.content)).collect();
        assert_eq!(said, [(id(2), Some("changed".to_string())), (id(6), make_message(id(6)).content)], "its own, as they stand, none taken back");
    }

    #[test]
    fn what_was_missed_is_what_came_after_the_last_message_kept_and_nothing_older() {
        let ids = |messages: Vec<Message>| messages.into_iter().map(|m| m.id).collect::<Vec<_>>();
        let (one, two, three, four) = (make_message(id(1)), make_message(id(2)), make_message(id(3)), make_message(id(4)));
        let mut history = MessageHistory::default();
        assert_eq!(ids(history.missing_from(vec![three.clone()])), [id(3)], "joined seeing the last message only");
        history.extend(vec![three.clone()]);
        // Rejoined with all of it: "four" was missed, "one" and "two" are
        // older than what it has and would come after it.
        assert_eq!(ids(history.missing_from(vec![one, two, three.clone(), four])), [id(4)], "in the order things were said");
        assert!(history.missing_from(vec![three]).is_empty(), "nothing new");
    }

    #[test]
    fn what_came_after_a_message_is_the_rest_of_the_active_path() {
        let ids = |messages: Vec<Message>| messages.into_iter().map(|m| m.id).collect::<Vec<_>>();
        let mut history = MessageHistory::default();
        history.extend(vec![make_message(id(1)), make_message(id(2)), make_message(id(3))]);
        assert_eq!(ids(history.messages_after(Some(id(1)))), [id(2), id(3)]);
        assert_eq!(ids(history.messages_after(Some(id(3)))), Vec::<MessageId>::new(), "nothing after the last");
        assert_eq!(ids(history.messages_after(None)), [id(1), id(2), id(3)], "all, if nothing came before");
        assert_eq!(ids(history.messages_after(Some(id(9)))), [id(1), id(2), id(3)], "all, if it is not on the path");
    }

    #[test]
    fn a_history_keeps_what_its_messages_were_produced_from() {
        let reference = ContextRef { transcript: Some(message_chain(None, &make_message(id(9)))), prompts: Vec::new(), tools: None, knowledge: Vec::new(), profile: Default::default() };
        let turn = TurnId::new();
        let mut history = MessageHistory::default();
        history.extend(vec![make_message(id(1))]);
        history.push(Message { turn_id: Some(turn), ..make_message(id(2)) }, Some(reference.clone()));

        let loaded: MessageHistory = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        let entries: Vec<_> = loaded.all_entries().collect();
        assert_eq!(entries[0].entry.context_of(id(1)), None, "said by someone else");
        assert_eq!(entries[1].entry.context_of(id(2)), Some((&reference, turn)), "with the turn it was produced in");
        assert_eq!(entries[1].entry.context_of(id(1)), None, "another message");
        assert_eq!(loaded.digest(), history.digest());
    }

    #[test]
    fn a_reclassification_is_recorded_and_read_into_the_messages_as_they_stand() {
        let public = Classification::general(Confidentiality::Public);
        let internal = Classification::general(Confidentiality::Internal);
        let said = |n, classification: &Classification| Message { classification: classification.clone(), ..make_message(id(n)) };
        let mut history = MessageHistory::default();
        history.extend(vec![said(1, &internal)]);
        let fork = history.digest();
        history.extend(vec![said(2, &internal)]);
        history.move_head(fork.as_ref());
        history.extend(vec![said(3, &internal)]);
        let before = history.digest();

        assert!(history.reclassify(id(2), public.clone()), "on the branch no longer active");
        assert!(!history.reclassify(id(9), public.clone()), "never said here");
        let mut revoked = history.clone();
        revoked.revoke(id(1), chrono::Utc::now());
        assert!(!revoked.reclassify(id(1), public.clone()), "taken back: nothing left to release");
        assert_ne!(history.digest(), before, "recorded: the digest covers it");
        assert_eq!(history.all_entries().len(), 4, "one entry");
        assert_eq!(history.message(id(2)).map(|m| m.classification), Some(public.clone()), "read as it stands, on whichever branch");
        assert_eq!(history.messages().iter().map(|m| m.id).collect::<Vec<_>>(), [id(1), id(3)], "the active path says what was said");

        assert_eq!(history.fit_into(&public), 2, "the two still internal");
        assert_eq!(history.fit_into(&public), 0, "nothing left to fit, recorded all the same");
        assert!(history.messages().iter().chain(history.message(id(2)).iter()).all(|m| m.classification == public));
        assert_eq!(history.last_message_id(), id(3), "the last message said, not the last entry");

        let loaded: MessageHistory = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        assert_eq!(loaded.messages().iter().map(|m| &m.classification).collect::<Vec<_>>(), [&public, &public]);
    }

    #[test]
    fn what_was_recorded_since_the_history_was_stored_replays_onto_it() {
        let public = Classification::general(Confidentiality::Public);
        let mut history = MessageHistory::default();
        history.extend(vec![make_message(id(1)), make_message(id(2))]);
        let mut stored: MessageHistory = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        history.mark_stored();

        history.reclassify(id(2), public.clone());
        history.fit_into(&public);
        let changes = history.changes().expect("changed");
        assert_eq!(changes.entries.len(), 2, "one entry each: {changes:?}");
        stored.apply(changes).unwrap();
        assert_eq!(stored.digest(), history.digest());
        assert!(stored.messages().iter().all(|m| m.classification == public));
    }

    #[test]
    fn a_change_is_recorded_and_read_into_the_messages_as_they_stand() {
        let at = chrono::Utc::now();
        let modified = |n, content: &str| Message { content: Some(content.to_string()), timestamp: at, ..make_message(id(n)) };
        let mut history = MessageHistory::default();
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
        assert!(!history.modify(Message { sender_id: crate::actor::ActorId::System, ..make_message(id(3)) }), "by another sender");
        assert_ne!(history.digest(), before, "recorded: the digest covers it");
        assert_eq!(history.all_entries().len(), 5, "one entry each, nothing rewritten");

        let messages = history.messages();
        assert_eq!(messages.iter().map(|m| m.id).collect::<Vec<_>>(), [id(1), id(3)], "in their places");
        assert_eq!((messages[0].content.as_deref(), messages[0].modified), (Some("corrected"), Some(at)));
        let revoked = history.message(id(2)).expect("a message taken back is still found");
        assert!(revoked.content.is_none() && revoked.revoked == Some(at));
        assert_eq!(history.last_message_id(), id(3), "a change is no message said");

        let loaded: MessageHistory = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        assert_eq!(loaded.messages()[0].content.as_deref(), Some("corrected"));
        assert_eq!(loaded.digest(), history.digest());
    }

    #[test]
    fn a_change_since_the_history_was_stored_replays_onto_it() {
        let mut history = MessageHistory::default();
        history.extend(vec![make_message(id(1)), make_message(id(2))]);
        let mut stored: MessageHistory = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        history.mark_stored();

        history.revoke(id(2), chrono::Utc::now());
        stored.apply(history.changes().expect("changed")).unwrap();
        assert_eq!(stored.digest(), history.digest());
        assert!(stored.message(id(2)).is_some_and(|m| m.revoked.is_some()));
    }
}
