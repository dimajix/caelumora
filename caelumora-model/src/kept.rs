//! Values kept once by digest (issues 47, 137): the texts of an agent's
//! prompt parts and the sets of tools its model was offered, which the
//! `ContextRef`s in its history point to. Only ever added to, and - like a
//! chain - keeping what was added since it was last stored (issue 193), so
//! that a store writes that and not the whole.

use crate::digest::Digest;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Values by digest, each kept once. Stored as the map alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Kept<V> {
    values: BTreeMap<Digest, V>,
    /// The digests added since the values were last stored, in order.
    #[serde(skip)]
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
        if let std::collections::btree_map::Entry::Vacant(vacant) = self.values.entry(digest) {
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

    /// Replays a change, as a store does when loading what it appended:
    /// `value` is kept under `digest` unless something is already. No change
    /// to store again.
    pub fn apply(&mut self, digest: Digest, value: V) {
        self.values.entry(digest).or_insert(value);
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
        let mut stored: Kept<String> = serde_json::from_value(serde_json::to_value(&kept).unwrap()).unwrap();
        assert!(stored.changes().is_empty(), "loaded: nothing pending");
        kept.mark_stored();

        kept.keep(prompt("a"), || unreachable!("kept already"));
        kept.keep(prompt("b"), || "b".to_string());
        assert_eq!(kept.len(), 2);
        let changes = kept.changes();
        assert_eq!(changes, [(prompt("b"), "b".to_string())], "once, and only what is new");

        for (digest, value) in changes.clone() {
            stored.apply(digest, value);
        }
        for (digest, value) in changes {
            stored.apply(digest, value);
        }
        assert_eq!(stored.get(&prompt("b")).map(String::as_str), Some("b"));
        assert_eq!(stored.len(), 2, "applied twice, kept once");
        assert!(stored.changes().is_empty(), "nothing replayed is a change to store again");
    }

    #[test]
    fn stored_as_the_map_alone() {
        let mut kept = Kept::default();
        kept.keep(prompt("a"), || "a".to_string());
        let json = serde_json::to_value(&kept).unwrap();
        assert_eq!(json, serde_json::json!({ prompt("a").to_string(): "a" }));
    }
}
