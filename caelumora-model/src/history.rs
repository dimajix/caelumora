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
use std::collections::{HashMap, HashSet};
use crate::digest;

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

/// A page of a history's items (issue 218): the last ones before a
/// position, as `items` gives them, and where the page before it ends.
#[derive(Debug, Clone, Default)]
pub struct HistoryPage {
    /// The items, oldest first.
    pub items: Vec<HistoryEntry>,
    /// The position of the oldest of `items`: what the page before this one
    /// is read before (`History::items_before`); `None` if there is no older
    /// item to be had.
    pub earlier: Option<u64>,
}

impl Chain<HistoryEntry> {
    /// The messages of the active path as they stand.
    pub fn messages(&self) -> Vec<Message> {
        let mut folded = self.folded();
        self.entries().filter_map(|node| folded.remove(&node.seq)).collect()
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
        let mut folded = self.folded();
        self.entries().filter_map(|node| item(node, &mut folded)).collect()
    }

    /// A page of `items` (issue 218): the last `limit` items of the active
    /// path before the position `before` and from the position `from` on -
    /// where a participant's view begins -, of those `keep` keeps, as they
    /// stand. Read backwards from `before`, and folded from the first item of
    /// the page to the end: what is read grows with how far back the page
    /// is, not with the history. At least one item is asked for.
    pub fn items_before(&self, before: u64, from: u64, limit: usize, keep: impl Fn(&HistoryEntry) -> bool) -> HistoryPage {
        let limit = limit.max(1);
        // One more than the page, to tell whether an older one is kept; as
        // many again while too few of them are.
        let mut wanted = limit + 1;
        loop {
            let candidates: Vec<&Node<HistoryEntry>> = self.entries_before(before).rev()
                .take_while(|node| node.seq >= from)
                .filter(|node| node.entry.is_item())
                .take(wanted)
                .collect();
            let Some(oldest) = candidates.last() else { return HistoryPage::default() };
            let mut folded: HashMap<u64, Message> = fold(self.from(oldest.seq)).into_iter().map(|(node, message)| (node.seq, message)).collect();
            let mut kept: Vec<(u64, HistoryEntry)> = candidates.iter().rev()
                .filter_map(|node| Some((node.seq, item(node, &mut folded)?)))
                .filter(|(_, item)| keep(item))
                .collect();
            if kept.len() > limit || candidates.len() < wanted {
                let older = kept.len().saturating_sub(limit);
                let page = kept.split_off(older);
                let earlier = page.first().filter(|_| older > 0).map(|(seq, _)| *seq);
                return HistoryPage { items: page.into_iter().map(|(_, item)| item).collect(), earlier };
            }
            wanted = wanted.saturating_mul(2);
        }
    }

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
    /// recorded lies: after the end of the turn of `actor` before it, `0` if
    /// none came before. An actor is in one turn at a time, and a turn
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
        reached.then_some(0)
    }

    /// What `agent` produced on a branch of its transcript, read from the end
    /// backwards: each of its messages recorded with the context it was
    /// produced from, with the transcript entry it was produced after - up to
    /// the compaction of `agent` that began the branch (issue 16), the
    /// `Compacted` entry whose new head `on_branch` says is on it; from the
    /// start if no compaction did. Every message produced on the branch is
    /// recorded after that entry, so it is all of them, and what was
    /// produced on later branches besides, whose contexts name entries the
    /// branch does not hold (issue 376).
    pub fn produced_on_branch<'a>(&'a self, agent: ActorId, on_branch: impl Fn(&Digest) -> bool + 'a) -> impl Iterator<Item = (MessageId, Option<Digest>)> + 'a {
        self.all_entries().rev()
            .take_while(move |node| !matches!(&node.entry, HistoryEntry::Compacted { actor, to, .. } if *actor == agent && on_branch(to)))
            .filter_map(move |node| match &node.entry {
                HistoryEntry::Said { message, context: Some(context) } if message.sender_id == agent => Some((message.id, context.transcript)),
                _ => None,
            })
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
        fold(self.from(self.position_of(id)?)).into_iter().next().map(|(_, message)| message)
    }

    /// The position of the entry that said the message `id` - on whichever
    /// branch -, or `None` if it was never said here. Found from the end
    /// backwards.
    pub fn position_of(&self, id: MessageId) -> Option<u64> {
        self.all_entries().rev().find(|node| node.entry.said().is_some_and(|m| m.id == id)).map(|node| node.seq)
    }

    /// The messages of every branch as they stand, by the position of the
    /// entry that said each.
    fn folded(&self) -> HashMap<u64, Message> {
        fold(self.all_entries()).into_iter().map(|(node, message)| (node.seq, message)).collect()
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
    /// Recorded whether or not it changes a message: telling which would
    /// fold the whole history.
    pub fn fit_into(&mut self, channel: &Classification) {
        self.record(HistoryEntry::Fitted { channel: channel.clone() });
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
        self.push_with(entry, digest::history_chain);
    }
}

/// `node` as an item of its history (`History::items`): the message it says
/// as `folded` holds it - taken from there -, with what it was produced from;
/// any other item as it is; `None` for a change, which is in the message it
/// changes.
fn item(node: &Node<HistoryEntry>, folded: &mut HashMap<u64, Message>) -> Option<HistoryEntry> {
    match &node.entry {
        HistoryEntry::Said { context, .. } => folded.remove(&node.seq).map(|message| HistoryEntry::Said { message, context: context.clone() }),
        entry => entry.is_item().then(|| entry.clone()),
    }
}

impl HistoryEntry {
    /// Whether it is an item of its history (`History::items`) - a message
    /// said, the start of a plan's step, a command carried out, an activity
    /// ended, a compaction, a turn's end - rather than a change of what was
    /// said before.
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

    /// What message `id` was produced from, and in which turn (issue 241),
    /// if this is it and both are known.
    pub fn context_of(&self, id: MessageId) -> Option<(&ContextRef, TurnId)> {
        self.context().filter(|_| self.said().is_some_and(|message| message.id == id))
    }

    /// What the message it says was produced from, and in which turn, if
    /// this is one said and both are known: whether what its author saw can
    /// be shown (issue 207).
    pub fn context(&self) -> Option<(&ContextRef, TurnId)> {
        match self {
            HistoryEntry::Said { message, context } => Some((context.as_ref()?, message.turn_id?)),
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
    use crate::agent::AgentId;

    #[test]
    fn a_turn_given_up_is_found_while_its_end_is_the_last_thing() {
        let (agent, other) = (ActorId::Agent(AgentId::new()), ActorId::Agent(AgentId::new()));
        let turn = TurnId::new();
        let mut history = History::default();
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
        let mut history = History::default();
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
    fn its_items_are_read_in_pages_from_the_end_as_they_stand() {
        let at = Utc::now();
        let turn = TurnId::new();
        let mut history = History::default();
        for n in 1..=5 {
            history.push(make_message(id(n)), None);
        }
        history.end_turn(HistoryEntry::TurnFinished { turn_id: turn, actor: ActorId::default(), at });
        history.push(make_message(id(6)), None);
        history.modify(Message { content: Some("changed".to_string()), ..make_message(id(1)) });
        assert!(history.revoke(id(5), at));
        let label = |item: &HistoryEntry| match item {
            HistoryEntry::Said { message, .. } if message.revoked.is_some() => format!("{} revoked", message.id.0.as_u128()),
            HistoryEntry::Said { message, .. } => format!("{} {}", message.id.0.as_u128(), message.content.clone().unwrap_or_default()),
            _ => "finished".to_string(),
        };
        let labels = |page: &HistoryPage| page.items.iter().map(label).collect::<Vec<_>>();

        let mut pages = Vec::new();
        let mut before = history.end();
        loop {
            let page = history.items_before(before, 0, 3, |_| true);
            pages.push(labels(&page));
            match page.earlier {
                Some(earlier) => before = earlier,
                None => break,
            }
        }
        assert_eq!(pages.len(), 3, "seven items, three to a page: {pages:?}");
        assert_eq!(pages[0], ["5 revoked".to_string(), "finished".to_string(), label(&history.items()[6])], "the end first, as it stands");
        assert_eq!(pages[2], ["1 changed"], "a change at the end is in a message of the first page");
        assert_eq!(pages.iter().rev().flatten().cloned().collect::<Vec<_>>(), history.items().iter().map(label).collect::<Vec<_>>(), "together, the items");

        let from = history.position_of(id(3)).unwrap();
        let visible = history.items_before(history.end(), from, 10, |_| true);
        assert_eq!(visible.items.len(), 5, "from where a view begins");
        assert_eq!(visible.earlier, None, "and nothing before it");

        let odd = |item: &HistoryEntry| item.said().is_some_and(|m| m.id.0.as_u128() % 2 == 1);
        let kept = history.items_before(history.end(), 0, 2, odd);
        assert_eq!(labels(&kept), ["3 ".to_string() + &make_message(id(3)).content.unwrap_or_default(), "5 revoked".to_string()], "a page of those kept, full");
        let rest = history.items_before(kept.earlier.unwrap(), 0, 2, odd);
        assert_eq!((labels(&rest), rest.earlier), (vec!["1 changed".to_string()], None), "and nothing older that is kept");
        assert!(history.items_before(0, 0, 5, |_| true).items.is_empty(), "nothing before the start");
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
        history.end_turn(ended(before, agent));
        let began = history.end();
        assert!(history.said_in_turn(agent, turn).is_empty(), "nothing said yet");

        history.push(said(turn, 2), Some(ContextRef::default()));
        history.push(said(theirs, 3), Some(ContextRef::default()));
        // Another actor's turn ends in between: no end of this actor's.
        history.end_turn(ended(theirs, other));
        history.push(said(turn, 4), Some(ContextRef::default()));
        assert_eq!(history.turn_began(agent, turn), Some(began), "after the end of its turn before");
        assert_eq!(history.said_in_turn(agent, turn).iter().map(|m| m.id).collect::<Vec<_>>(), [id(2), id(4)], "past the other actor's turn end");

        history.end_turn(ended(turn, agent));
        history.push(said(TurnId::new(), 5), Some(ContextRef::default()));
        history.end_turn(ended(TurnId::new(), agent));
        assert!(history.revoke(id(2), at), "changed after later turns");
        assert_eq!(history.said_in_turn(agent, turn).iter().map(|m| m.id).collect::<Vec<_>>(), [id(4)], "an older turn, as it stands");
        assert_eq!(history.said_in_turn(agent, before).iter().map(|m| m.id).collect::<Vec<_>>(), [id(1)], "the first, from the start");
    }

    #[test]
    fn what_an_agent_produced_on_a_branch_is_read_back_to_the_compaction_that_began_it() {
        let (agent, other) = (ActorId::Agent(AgentId::new()), ActorId::Agent(AgentId::new()));
        let turn = TurnId::new();
        let at = Utc::now();
        let after = |label: &str| Some(digest::prompt(label));
        let produced = |sender, n: u128, label: &str| (Message { sender_id: sender, ..make_message(id(n)) }, Some(ContextRef { transcript: after(label), ..ContextRef::default() }));
        let compacted = |actor, to: &str| HistoryEntry::Compacted { turn_id: turn, actor, from: digest::prompt("before"), to: digest::prompt(to), at };
        let mut history = History::default();
        let (message, context) = produced(agent, 1, "root");
        history.push(message, context);
        history.record_compaction(compacted(agent, "first"));
        let (message, context) = produced(agent, 2, "first");
        history.push(message, context);
        history.push(make_message(id(3)), None);
        // Another agent's compaction, and what it produced: not this agent's.
        history.record_compaction(compacted(other, "theirs"));
        let (message, context) = produced(other, 4, "theirs");
        history.push(message, context);
        history.record_compaction(compacted(agent, "second"));
        let (message, context) = produced(agent, 5, "second");
        history.push(message, context);

        let on = |branch: &'static str| history.produced_on_branch(agent, move |digest| *digest == digest::prompt(branch)).collect::<Vec<_>>();
        assert_eq!(on("second"), [(id(5), after("second"))], "the current branch, from its compaction on");
        assert_eq!(on("first"), [(id(5), after("second")), (id(2), after("first"))], "an earlier one, and what came after it");
        assert_eq!(on("elsewhere").len(), 3, "from the start if no compaction began it");
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
    fn what_was_missed_is_what_came_after_the_last_message_kept_and_nothing_older() {
        let ids = |messages: Vec<Message>| messages.into_iter().map(|m| m.id).collect::<Vec<_>>();
        let (one, two, three, four) = (make_message(id(1)), make_message(id(2)), make_message(id(3)), make_message(id(4)));
        let mut history = History::default();
        assert_eq!(ids(history.missing_from(vec![three.clone()])), [id(3)], "joined seeing the last message only");
        history.extend(vec![three.clone()]);
        // Rejoined with all of it: "four" was missed, "one" and "two" are
        // older than what it has and would come after it.
        assert_eq!(ids(history.missing_from(vec![one, two, three.clone(), four])), [id(4)], "in the order things were said");
        assert!(history.missing_from(vec![three]).is_empty(), "nothing new");
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
        assert_eq!(history.messages().iter().map(|m| m.id).collect::<Vec<_>>(), [id(1), id(3)], "the active path says what was said");

        let entries = history.all_entries().len();
        history.fit_into(&public);
        history.fit_into(&public);
        assert_eq!(history.all_entries().len(), entries + 2, "recorded, also when nothing is left to fit");
        assert!(history.messages().iter().chain(history.message(id(2)).iter()).all(|m| m.classification == public));
        assert_eq!(history.last_message_id(), id(3), "the last message said, not the last entry");

        let loaded: History = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        assert_eq!(loaded.messages().iter().map(|m| &m.classification).collect::<Vec<_>>(), [&public, &public]);
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
        assert!(stored.messages().iter().all(|m| m.classification == public));
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

        let messages = history.messages();
        assert_eq!(messages.iter().map(|m| m.id).collect::<Vec<_>>(), [id(1), id(3)], "in their places");
        assert_eq!((messages[0].content.as_deref(), messages[0].modified), (Some("corrected"), Some(at)));
        let revoked = history.message(id(2)).expect("a message taken back is still found");
        assert!(revoked.content.is_none() && revoked.revoked == Some(at));
        assert_eq!(history.last_message_id(), id(3), "a change is no message said");

        let loaded: History = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        assert_eq!(loaded.messages()[0].content.as_deref(), Some("corrected"));
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
