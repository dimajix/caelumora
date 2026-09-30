//! The history of a channel as an agent keeps it (issue 06): the chain of
//! messages said in it, each of the agent's own with what it was produced
//! from.

use crate::chain::{Chain, ChainEntry};
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
    pub fn new() -> Self {
        Self::default()
    }

    /// The digest of the whole history, or `None` while it is empty.
    pub fn digest(&self) -> Option<Digest> {
        self.messages.digest()
    }

    pub fn messages(&self) -> Vec<Message> {
        messages_of(&self.messages)
    }

    /// The entries of the active branch.
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &HistoryEntry> + ExactSizeIterator {
        self.messages.entries()
    }

    /// The entries of every branch, in the order they were added.
    pub fn all_entries(&self) -> impl DoubleEndedIterator<Item = &HistoryEntry> + ExactSizeIterator {
        self.messages.all_entries()
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
        match self.messages.entries_mut().find(|e| e.message.id == id) {
            Some(entry) => {
                entry.message.classification = classification;
                true
            }
            None => false,
        }
    }

    /// Fits every message into a channel now classified `channel`
    /// (`Classification::fitted_into`) - what lowering a channel's
    /// classification does to what is in it, on every branch: all of them
    /// were said in the channel. Returns how many changed.
    pub fn fit_into(&mut self, channel: &Classification) -> usize {
        let mut changed = 0;
        for entry in self.messages.entries_mut() {
            let fitted = entry.message.classification.fitted_into(channel);
            if fitted != entry.message.classification {
                entry.message.classification = fitted;
                changed += 1;
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::message_chain;
    use crate::message::tests::{id, make_message};

    #[test]
    fn a_history_keeps_what_its_messages_were_produced_from() {
        let reference = ContextRef { transcript: Some(message_chain(None, &make_message(id(9)))), prompts: Vec::new(), tools: None, knowledge: Vec::new(), profile: Default::default(), turn_entries: 0 };
        let mut history = MessageHistory::new();
        history.extend(vec![make_message(id(1))]);
        history.push(make_message(id(2)), Some(reference.clone()));

        let loaded: MessageHistory = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        let entries: Vec<_> = loaded.entries().collect();
        assert_eq!(entries[0].context, None, "said by someone else");
        assert_eq!(entries[1].context, Some(reference));
        assert_eq!(loaded.digest(), history.digest());
    }
}
