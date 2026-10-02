//! The history of a channel as an agent keeps it (issue 06): the chain of
//! messages said in it, each of the agent's own with what it was produced
//! from.

use crate::chain::{Chain, ChainChange, ChainEntry};
use crate::confidentiality::Classification;
use crate::context::ContextRef;
use crate::digest::Digest;
use crate::message::{last_message_id_of, messages_of, push_message, Message, MessageEntry, MessageId};
use serde::{Deserialize, Serialize};

/// A message of a `MessageHistory`: with the digest of the history up to and
/// including it and, for a message this agent produced, what it was produced
/// from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub message: Message,
    pub context: Option<ContextRef>,
    pub digest: Digest,
}

impl ChainEntry for HistoryEntry {
    fn digest(&self) -> &Digest {
        &self.digest
    }
}

impl MessageEntry for HistoryEntry {
    fn message(&self) -> &Message {
        &self.message
    }
}

/// The conversation as it happened - a chain like a `Transcript`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageHistory {
    messages: Chain<HistoryEntry>,
}

impl MessageHistory {
    /// The digest of the whole history, or `None` while it is empty.
    pub fn digest(&self) -> Option<Digest> {
        self.messages.digest()
    }

    pub fn messages(&self) -> Vec<Message> {
        messages_of(&self.messages)
    }

    /// The entries of every branch, in the order they were added.
    pub fn all_entries(&self) -> impl DoubleEndedIterator<Item = &HistoryEntry> + ExactSizeIterator {
        self.messages.all_entries()
    }

    /// The messages of `visible` after the last of them the history has,
    /// and not in it - on any branch: what an actor that joins missed (issue
    /// 190), for one that joins again what was said while it was away. A
    /// history is only ever appended to, so older messages that a wider
    /// history grants on rejoining would land after the actor's own later
    /// answers; they are left out, and the history stays in the order things
    /// were said.
    pub fn missing_from(&self, visible: Vec<Message>) -> Vec<Message> {
        let known: std::collections::HashSet<_> = self.all_entries().map(|e| e.message.id).collect();
        let after = visible.iter().rposition(|m| known.contains(&m.id)).map_or(0, |last| last + 1);
        visible.into_iter().skip(after).filter(|m| !known.contains(&m.id)).collect()
    }

    pub fn last_message_id(&self) -> MessageId {
        last_message_id_of(&self.messages)
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
        push_message(&mut self.messages, msg, |message, digest| HistoryEntry { message, context, digest });
    }

    /// Moves the head to the entry with `digest` - `None`: before every
    /// entry - to go on from there (issue 08). `false` if there is none.
    pub fn move_head(&mut self, digest: Option<&Digest>) -> bool {
        self.messages.move_head(digest)
    }

    /// Reclassifies message `id` - on whichever branch - as
    /// `classification` (issue 133), in place:
    /// a message's digest does not cover its classification, so the record
    /// stays what it was. `false` if there is no such message.
    pub fn reclassify(&mut self, id: MessageId, classification: Classification) -> bool {
        self.messages.amend_each(|entry| {
            let it = entry.message.id == id;
            if it {
                entry.message.classification = classification.clone();
            }
            it
        }) > 0
    }

    /// Fits every message into a channel now classified `channel`
    /// (`Classification::fitted_into`) - what lowering a channel's
    /// classification does to what is in it, on every branch: all of them
    /// were said in the channel. Returns how many changed.
    pub fn fit_into(&mut self, channel: &Classification) -> usize {
        self.messages.amend_each(|entry| {
            let fitted = entry.message.classification.fitted_into(channel);
            let changed = fitted != entry.message.classification;
            if changed {
                entry.message.classification = fitted;
            }
            changed
        })
    }

    /// The changes since the history was last stored (issue 193).
    pub fn changes(&self) -> Vec<ChainChange<HistoryEntry>> {
        self.messages.changes()
    }

    pub fn mark_stored(&mut self) {
        self.messages.mark_stored()
    }

    /// Replays `change` onto the history as stored (`Chain::apply`).
    pub fn apply(&mut self, change: ChainChange<HistoryEntry>) -> Result<(), String> {
        self.messages.apply(change)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(entries[0].context, None, "said by someone else");
        assert_eq!(entries[1].context, Some(reference));
        assert_eq!(loaded.digest(), history.digest());
    }

    #[test]
    fn reclassifying_reaches_the_stored_history_as_changes_and_leaves_the_digests() {
        use crate::confidentiality::{Classification, Confidentiality};
        let mut history = MessageHistory::default();
        history.extend(vec![make_message(id(1)), make_message(id(2))]);
        let mut stored: MessageHistory = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        history.mark_stored();

        let public = Classification::general(Confidentiality::Public);
        assert!(history.reclassify(id(2), public.clone()));
        assert!(!history.reclassify(id(9), public.clone()), "no such message");
        assert_eq!(history.fit_into(&public), 1, "the first still had to be fitted; the second fits");
        let changes = history.changes();
        assert_eq!(changes.len(), 2, "one amendment per message changed: {changes:?}");

        for change in changes {
            stored.apply(change).unwrap();
        }
        assert_eq!(stored.digest(), history.digest(), "a classification is not what a digest covers");
        assert!(stored.messages().iter().all(|m| m.classification == public));
    }
}
