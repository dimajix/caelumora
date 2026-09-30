//! Agent memory (issue 62, step 3): what an agent remembers across its
//! channels and sessions, kept in its `AgentState`.
//!
//! Like notes (`crate::note`), memory is a chain of changes that is only
//! appended to - something remembered, or an earlier entry forgotten - and
//! what is remembered now is the chain read from start to end. Unlike notes,
//! it belongs to the agent itself, not to one of its channels: it does not
//! branch with a channel's history (principle 7 in issue 19), and every entry
//! carries the classification of the channel it was written in, so that it
//! is recalled only where that channel's contents may be read (the flow
//! rule of issue 133, `Classification::flows_to`).

use crate::chain::{Chain, ChainEntry};
use crate::confidentiality::ChannelClassification;
use crate::digest::Digest;
use crate::message::MessageId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The number of an entry of an agent's memory: 1 for the first, counting
/// on, never given twice. Small, because a model has to repeat it to forget
/// the entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryId(pub u32);

impl fmt::Display for MemoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A change to an agent's memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryChange {
    Remember { id: MemoryId, text: String },
    Forget { id: MemoryId },
}

/// An entry of a [`MemoryChain`]: a change, the channel it was made in,
/// where in that channel's conversation, and the digest of the chain up to
/// and including it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub change: MemoryChange,
    /// The channel the change was made in, with its level at the time.
    pub origin: ChannelClassification,
    /// The message the change follows: the last one of the turn that made
    /// it in `origin` (`MessageId::default()` if there was none).
    pub after: MessageId,
    pub written_at: DateTime<Utc>,
    pub digest: Digest,
}

impl ChainEntry for MemoryEntry {
    fn digest(&self) -> &Digest {
        &self.digest
    }
}

/// Something an agent remembers now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    pub id: MemoryId,
    pub text: String,
    pub origin: ChannelClassification,
    pub after: MessageId,
    pub written_at: DateTime<Utc>,
}

/// An agent's memory, as a chain of changes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryChain {
    changes: Chain<MemoryEntry>,
}

impl MemoryChain {
    pub fn new() -> Self {
        Self::default()
    }

    /// The digest of the whole chain - what a reader records as the state
    /// of knowledge it saw - or `None` while it is empty.
    pub fn digest(&self) -> Option<Digest> {
        self.changes.digest()
    }

    pub fn entries(&self) -> &[MemoryEntry] {
        self.changes.entries()
    }

    /// Appends `change`, made in `origin` in the turn that ended with message
    /// `after`.
    pub fn push(&mut self, change: MemoryChange, origin: ChannelClassification, after: MessageId) {
        self.changes.push_with(|previous| {
            let digest = crate::digest::memory_chain(previous, &change, &origin, after);
            MemoryEntry { change, origin, after, written_at: Utc::now(), digest }
        });
    }

    /// What is remembered now: every entry not forgotten, oldest first.
    pub fn current(&self) -> Vec<Memory> {
        let mut memories: Vec<Memory> = Vec::new();
        for entry in self.changes.entries() {
            match &entry.change {
                MemoryChange::Remember { id, text } => memories.push(Memory {
                    id: *id,
                    text: text.clone(),
                    origin: entry.origin.clone(),
                    after: entry.after,
                    written_at: entry.written_at,
                }),
                MemoryChange::Forget { id } => memories.retain(|m| m.id != *id),
            }
        }
        memories
    }

    /// The number the next entry gets: one more than any ever remembered.
    pub fn next_id(&self) -> MemoryId {
        let highest = self.changes.entries().iter()
            .filter_map(|e| match &e.change {
                MemoryChange::Remember { id, .. } => Some(id.0),
                MemoryChange::Forget { .. } => None,
            })
            .max()
            .unwrap_or(0);
        MemoryId(highest + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> ChannelClassification {
        ChannelClassification::default()
    }

    #[test]
    fn what_is_remembered_is_the_chain_read_from_start_to_end() {
        let mut chain = MemoryChain::new();
        assert_eq!(chain.next_id(), MemoryId(1));
        let remember = |id, text: &str| MemoryChange::Remember { id: MemoryId(id), text: text.to_string() };
        chain.push(remember(1, "Kaya likes tea."), origin(), MessageId::new());
        chain.push(remember(2, "Kaya lives in Berlin."), origin(), MessageId::new());
        chain.push(MemoryChange::Forget { id: MemoryId(1) }, origin(), MessageId::new());

        let texts: Vec<_> = chain.current().into_iter().map(|m| m.text).collect();
        assert_eq!(texts, ["Kaya lives in Berlin."]);
        assert_eq!(chain.next_id(), MemoryId(3), "a forgotten number is not given again");
        assert_eq!(chain.current()[0].origin, origin(), "every entry knows where it was written");
    }
}
