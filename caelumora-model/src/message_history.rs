//! The history of a channel as an agent keeps it (issue 06): the chain of
//! what happened in it - messages said, each of the agent's own with what it
//! was produced from, and reclassifications (issue 133) - read into the
//! messages as they stand (issue 197).

use crate::chain::Chain;
use crate::confidentiality::Classification;
use crate::context::ContextRef;
use crate::message::{Message, MessageId};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// What happened in a `MessageHistory`; its node carries the digest of the
/// history up to and including it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryEntry {
    /// A message said and, for one this agent produced, what it was
    /// produced from. Boxed: a message is many times the size of the other
    /// entries.
    Said { message: Box<Message>, context: Option<ContextRef> },
    /// The message `message` - on whichever branch - is classified
    /// `classification` from here on (issue 133).
    Reclassified { message: MessageId, classification: Classification },
    /// The channel was classified `channel`, and every message said so far
    /// fits into it from here on (`Classification::fitted_into`).
    Fitted { channel: Classification },
}

/// The conversation as it happened - a chain like a `Transcript`. What it
/// records is read from start to end into the messages as they stand: a
/// reclassification applies to a message on any branch, as a change made
/// in the channel does, so it is folded over every branch in the order
/// things happened.
pub type MessageHistory = Chain<HistoryEntry>;

impl Chain<HistoryEntry> {
    /// The messages of the active path as they stand.
    pub fn messages(&self) -> Vec<Message> {
        let active: HashSet<MessageId> = self.entries().filter_map(|n| n.entry.said()).map(|m| m.id).collect();
        self.fold().into_iter().filter(|m| active.contains(&m.id)).collect()
    }

    /// The message `id` as it stands - on whichever branch - or `None` if
    /// it was never said here.
    pub fn message(&self, id: MessageId) -> Option<Message> {
        self.fold().into_iter().find(|m| m.id == id)
    }

    /// The messages of every branch as they stand, in the order they were
    /// said: every message said, with every reclassification since applied
    /// to it.
    fn fold(&self) -> Vec<Message> {
        let mut messages: Vec<Message> = Vec::new();
        let mut at: HashMap<MessageId, usize> = HashMap::new();
        for node in self.all_entries() {
            match &node.entry {
                HistoryEntry::Said { message, .. } => {
                    at.insert(message.id, messages.len());
                    messages.push((**message).clone());
                }
                HistoryEntry::Reclassified { message, classification } => {
                    if let Some(&i) = at.get(message) {
                        messages[i].classification = classification.clone();
                    }
                }
                HistoryEntry::Fitted { channel } => {
                    for message in &mut messages {
                        message.classification = message.classification.fitted_into(channel);
                    }
                }
            }
        }
        messages
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
        self.record(HistoryEntry::Said { message: Box::new(msg), context });
    }

    /// Records that message `id` - on whichever branch - is classified
    /// `classification` from now on (issue 133). `false`, and nothing is
    /// recorded, if there is no such message.
    pub fn reclassify(&mut self, id: MessageId, classification: Classification) -> bool {
        if !self.all_entries().filter_map(|n| n.entry.said()).any(|m| m.id == id) {
            return false;
        }
        self.record(HistoryEntry::Reclassified { message: id, classification });
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

    fn record(&mut self, entry: HistoryEntry) {
        self.push_with(entry, crate::digest::history_chain);
    }
}

impl HistoryEntry {
    /// The message, if this is one said.
    fn said(&self) -> Option<&Message> {
        match self {
            HistoryEntry::Said { message, .. } => Some(message),
            HistoryEntry::Reclassified { .. } | HistoryEntry::Fitted { .. } => None,
        }
    }

    /// What message `id` was produced from, if this is it and that is
    /// known.
    pub fn context_of(&self, id: MessageId) -> Option<&ContextRef> {
        match self {
            HistoryEntry::Said { message, context } if message.id == id => context.as_ref(),
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
    fn a_history_keeps_what_its_messages_were_produced_from() {
        let reference = ContextRef { transcript: Some(message_chain(None, &make_message(id(9)))), prompts: Vec::new(), tools: None, knowledge: Vec::new(), profile: Default::default(), turn_entries: 0 };
        let mut history = MessageHistory::default();
        history.extend(vec![make_message(id(1))]);
        history.push(make_message(id(2)), Some(reference.clone()));

        let loaded: MessageHistory = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        let entries: Vec<_> = loaded.all_entries().collect();
        assert_eq!(entries[0].entry.context_of(id(1)), None, "said by someone else");
        assert_eq!(entries[1].entry.context_of(id(2)), Some(&reference));
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
        let changes = history.changes();
        assert_eq!(changes.len(), 2, "one entry each: {changes:?}");
        for change in changes {
            stored.apply(change).unwrap();
        }
        assert_eq!(stored.digest(), history.digest());
        assert!(stored.messages().iter().all(|m| m.classification == public));
    }
}
