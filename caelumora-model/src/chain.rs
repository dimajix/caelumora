//! A chain of entries that is only appended to, each entry carrying a digest
//! over its own content and the digest before it (issue 48).
//!
//! One digest therefore names the whole chain up to its entry by content, and
//! every earlier digest names a prefix of it. `MessageHistory` and
//! `Transcript` are chains of messages (issue 06); a channel's notes will
//! be one too (issue 17), and branching (issue 08) will make chains trees.
//!
//! What goes into an entry's digest is up to its type, which computes it when
//! the entry is made (see `crate::digest`). The chain stores the digest and
//! never recomputes it: were a scheme to change, recomputing would silently
//! change every digest that refers to an entry.

use crate::digest::Digest;
use serde::{Deserialize, Serialize};

/// An entry of a [`Chain`]: something with the digest of the chain up to and
/// including it.
pub trait ChainEntry {
    fn digest(&self) -> &Digest;
}

/// Entries that are only appended to, each chained to the one before by its
/// digest. Serialised as the list of its entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Chain<E> {
    entries: Vec<E>,
}

impl<E> Default for Chain<E> {
    fn default() -> Self {
        Self { entries: Vec::new() }
    }
}

impl<E: ChainEntry> Chain<E> {
    pub fn new() -> Self {
        Self::default()
    }

    /// The digest of the whole chain, or `None` while it is empty.
    pub fn digest(&self) -> Option<Digest> {
        self.entries.last().map(|e| *e.digest())
    }

    pub fn entries(&self) -> &[E] {
        &self.entries
    }

    pub fn last(&self) -> Option<&E> {
        self.entries.last()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entries, to change what their digests do not cover - and nothing
    /// else: what a digest covers is the record and is never rewritten.
    pub(crate) fn entries_mut(&mut self) -> &mut [E] {
        &mut self.entries
    }

    /// Appends the entry `make` builds from the digest of the chain before
    /// it, and returns that digest.
    pub fn push_with(&mut self, make: impl FnOnce(Option<&Digest>) -> E) -> Option<Digest> {
        let previous = self.digest();
        self.entries.push(make(previous.as_ref()));
        previous
    }

    /// The entries of the chain `digest` names: this chain up to and
    /// including the entry with that digest, or `None` if it is not a prefix
    /// of this chain.
    pub fn prefix(&self, digest: &Digest) -> Option<&[E]> {
        let end = self.entries.iter().position(|e| e.digest() == digest)?;
        Some(&self.entries[..=end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An entry that is just its digest, chained over a label.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct Label {
        label: String,
        digest: Digest,
    }

    impl ChainEntry for Label {
        fn digest(&self) -> &Digest {
            &self.digest
        }
    }

    fn push(chain: &mut Chain<Label>, label: &str) -> Option<Digest> {
        chain.push_with(|previous| {
            let chained = format!("{}/{}", previous.map(|d| d.to_string()).unwrap_or_default(), label);
            Label { label: label.to_string(), digest: crate::digest::prompt(&chained) }
        })
    }

    #[test]
    fn every_entry_is_chained_to_the_one_before() {
        let mut chain = Chain::new();
        assert_eq!(push(&mut chain, "a"), None, "nothing before the first");
        let after_a = chain.digest();
        assert_eq!(push(&mut chain, "b"), after_a, "the digest before is what an entry was made from");

        let mut other = Chain::new();
        push(&mut other, "x");
        push(&mut other, "b");
        assert_ne!(chain.digest(), other.digest(), "the same entry after another one");
    }

    #[test]
    fn an_earlier_digest_names_a_prefix() {
        let mut chain = Chain::new();
        push(&mut chain, "a");
        let after_a = chain.digest().unwrap();
        push(&mut chain, "b");

        let labels = |entries: &[Label]| entries.iter().map(|e| e.label.clone()).collect::<Vec<_>>();
        assert_eq!(labels(chain.prefix(&after_a).unwrap()), ["a"]);
        assert_eq!(labels(chain.prefix(&chain.digest().unwrap()).unwrap()), ["a", "b"]);
        assert!(chain.prefix(&crate::digest::prompt("elsewhere")).is_none());
    }

    #[test]
    fn a_chain_is_stored_as_its_entries() {
        let mut chain = Chain::new();
        push(&mut chain, "a");
        let json = serde_json::to_value(&chain).unwrap();
        assert!(json.is_array(), "{json}");
        let loaded: Chain<Label> = serde_json::from_value(json).unwrap();
        assert_eq!(loaded.entries(), chain.entries());
    }
}
