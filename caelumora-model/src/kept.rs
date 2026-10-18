//! Values kept once by digest (issues 47, 137, 426): the texts of an agent's
//! prompt parts, the sets of tools its model was offered and the frames of
//! its contexts, which the messages in its history point to. Added to,
//! and - like a chain - keeping what was added since it was last stored
//! (issue 193), so that a store writes that, apart from the state's
//! document (issue 383).

use crate::digest::Digest;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

/// Values by digest, each kept once.
#[derive(Debug, Clone)]
pub struct Kept<V> {
    values: BTreeMap<Digest, V>,
    /// The digests added since the values were last stored, in order.
    pending: Vec<Digest>,
}

impl<V> Default for Kept<V> {
    fn default() -> Self {
        Self { values: BTreeMap::new(), pending: Vec::new() }
    }
}

impl<V> Kept<V> {
    /// Keeps what `value` makes under `digest`, unless something is kept
    /// there already: a digest names its value, so what is there is it.
    pub fn keep(&mut self, digest: Digest, value: impl FnOnce() -> V) {
        if let Entry::Vacant(vacant) = self.values.entry(digest) {
            vacant.insert(value());
            self.pending.push(digest);
        }
    }

    pub fn get(&self, digest: &Digest) -> Option<&V> {
        self.values.get(digest)
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// What was added since the values were last stored (`mark_stored`), in
    /// the order it was added.
    pub fn changes(&self) -> Vec<(Digest, V)>
    where
        V: Clone,
    {
        self.pending.iter().filter_map(|d| self.values.get(d).map(|v| (*d, v.clone()))).collect()
    }

    /// Forgets the changes: they are stored.
    pub fn mark_stored(&mut self) {
        self.pending.clear();
    }

    /// Lets go of the values that are stored and not `wanted` (issue 383):
    /// what is still to be stored stays, whatever is wanted.
    pub fn keep_only(&mut self, wanted: impl Fn(&Digest) -> bool) {
        let pending = &self.pending;
        self.values.retain(|digest, _| wanted(digest) || pending.contains(digest));
    }

    /// The values a store read, each under its digest, with nothing
    /// pending: they are stored.
    pub fn stored(values: impl IntoIterator<Item = (Digest, V)>) -> Self {
        Self { values: values.into_iter().collect(), pending: Vec::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::prompt;

    #[test]
    fn a_value_is_kept_once_and_what_was_added_since_stored_is_told() {
        let mut kept = Kept::default();
        kept.keep(prompt("a"), || "a".to_string());
        kept.mark_stored();

        kept.keep(prompt("a"), || unreachable!("kept already"));
        kept.keep(prompt("b"), || "b".to_string());
        assert_eq!(kept.len(), 2);
        assert_eq!(kept.changes(), [(prompt("b"), "b".to_string())], "once, and only what is new");

        kept.keep_only(|_| false);
        assert_eq!(kept.changes(), [(prompt("b"), "b".to_string())], "what is to be stored stays");
        assert!(kept.get(&prompt("a")).is_none(), "what is stored and not wanted goes");
        kept.mark_stored();
        kept.keep_only(|digest| *digest == prompt("b"));
        assert_eq!(kept.len(), 1, "what is wanted stays");

        let stored = Kept::stored([(prompt("b"), "b".to_string())]);
        assert_eq!(stored.get(&prompt("b")).map(String::as_str), Some("b"));
        assert!(stored.changes().is_empty(), "what a store read is no change to store again");
    }
}
