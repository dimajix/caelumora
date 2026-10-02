//! A chain of entries that is only appended to, each node carrying a digest
//! over its entry and the digest before it (issue 48) - a tree, since
//! branching (issue 08): an entry follows the one that was the chain's head
//! when it was added, and the head can be moved back to any entry to go on
//! from there. The entries after it stay, as a branch that is no longer
//! active. Nothing is ever removed.
//!
//! One digest therefore names the path from the root up to its entry by
//! content, and every digest on that path names a prefix of it.
//! `MessageHistory` and `Transcript` are chains of messages (issue 06), an
//! agent's notes (issue 17) and memory (issue 62) chains of changes.
//!
//! What goes into a digest is up to the entry's type, which computes it when
//! the entry is pushed (see `crate::digest`); the chain keeps it in the
//! entry's [`Node`] and never recomputes it: were a scheme to change,
//! recomputing would silently change every digest that refers to an entry.
//!
//! A chain changes in two ways only - an entry is appended, the head
//! moves - and keeps a journal of them since it was last stored (issue
//! 193): `changes` tells
//! them as [`ChainChange`]s, `mark_stored` forgets them, and `apply` replays
//! one onto a chain stored before. A store thus writes what was added, not
//! the chain.

use crate::digest::Digest;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;

/// An entry at its place in the chain: with the digest of the chain up to
/// and including it, and the digest of the entry it follows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node<E> {
    pub digest: Digest,
    /// The digest of the entry it follows, `None` for a root.
    pub parent: Option<Digest>,
    /// Of the entries following it, the one last on the active path - where
    /// a switch to this branch goes on (`Chain::leaf_from`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) active_child: Option<Digest>,
    pub entry: E,
}

impl<E> Node<E> {
    /// `entry` after the entry `parent` names, with `digest` - as a chain
    /// would place it, before it is in one (a turn's messages, issue 136).
    pub fn new(digest: Digest, parent: Option<Digest>, entry: E) -> Self {
        Self { digest, parent, active_child: None, entry }
    }
}

/// A change to a [`Chain`] since it was last stored (issue 193): what a
/// store appends, and replays through [`Chain::apply`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainChange<E> {
    /// An entry appended, which became the head.
    Added(Node<E>),
    /// The head moved to the entry `to` names - `None`: the root, before
    /// every entry.
    Head { to: Option<Digest> },
}

/// A change not yet stored, by the position of its entry in `nodes`.
#[derive(Debug, Clone)]
enum Pending {
    Added(usize),
    Head(Option<Digest>),
}

/// Entries that are only appended to, each chained to the one before by its
/// digest, as a tree with a head. What the chain says - its digest, its
/// entries, its last entry - is the active path, from the root up to the
/// head; `all_entries` are those of every branch. Stored as every node, in
/// the order added, and the head.
#[derive(Debug, Clone)]
pub struct Chain<E> {
    /// Every entry, in the order it was added: an entry after the one it
    /// follows.
    nodes: Vec<Node<E>>,
    head: Option<Digest>,
    /// Where each entry is in `nodes`, by its digest.
    index: HashMap<Digest, usize>,
    /// The active path: positions in `nodes`, from the root up to the head.
    path: Vec<usize>,
    /// What changed since the chain was last stored, in order.
    pending: Vec<Pending>,
}

impl<E> Default for Chain<E> {
    fn default() -> Self {
        Self { nodes: Vec::new(), head: None, index: HashMap::new(), path: Vec::new(), pending: Vec::new() }
    }
}

impl<E> Chain<E> {
    pub fn new() -> Self {
        Self::default()
    }

    /// The digest of the active path, or `None` while it is empty.
    pub fn digest(&self) -> Option<Digest> {
        self.head
    }

    /// The entries of the active path, from the root up to the head.
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &Node<E>> + ExactSizeIterator {
        self.path.iter().map(|&i| &self.nodes[i])
    }

    /// Every entry of every branch, in the order they were added.
    pub fn all_entries(&self) -> impl DoubleEndedIterator<Item = &Node<E>> + ExactSizeIterator {
        self.nodes.iter()
    }

    /// The entry at the head.
    pub fn last(&self) -> Option<&Node<E>> {
        self.path.last().map(|&i| &self.nodes[i])
    }

    /// The length of the active path.
    pub fn len(&self) -> usize {
        self.path.len()
    }

    pub fn is_empty(&self) -> bool {
        self.path.is_empty()
    }

    /// Appends `entry` with the digest `digest` computes for it after the
    /// head, makes it the head, and returns the digest before it. An entry
    /// with the digest of one already there is that entry - the same
    /// predecessor and the same content - and is not added twice: the head
    /// moves to it.
    pub(crate) fn push_with(&mut self, entry: E, digest: impl FnOnce(Option<&Digest>, &E) -> Digest) -> Option<Digest> {
        let previous = self.head;
        let digest = digest(previous.as_ref(), &entry);
        if self.index.contains_key(&digest) {
            self.move_head(Some(&digest));
        } else {
            self.add(Node::new(digest, previous, entry));
            self.pending.push(Pending::Added(self.nodes.len() - 1));
        }
        previous
    }

    /// Adds `node`, whose digest is not there yet, and makes it the head.
    fn add(&mut self, node: Node<E>) {
        let digest = node.digest;
        self.index.insert(digest, self.nodes.len());
        self.nodes.push(node);
        self.set_head(Some(&digest));
    }

    /// The changes since the chain was last stored (`mark_stored`), in the
    /// order they were made.
    pub fn changes(&self) -> Vec<ChainChange<E>>
    where
        E: Clone,
    {
        self.pending.iter().map(|pending| match pending {
            Pending::Added(at) => ChainChange::Added(Node::new(self.nodes[*at].digest, self.nodes[*at].parent, self.nodes[*at].entry.clone())),
            Pending::Head(to) => ChainChange::Head { to: *to },
        }).collect()
    }

    /// Forgets the changes: they are stored.
    pub fn mark_stored(&mut self) {
        self.pending.clear();
    }

    /// Replays `change`, as a store does when loading what it appended onto
    /// the chain it stored before. Nothing it does is a change to store
    /// again. Idempotent: an entry added that is there already only becomes
    /// the head, a head move restates where it is.
    ///
    /// # Errors
    /// If the change does not fit the chain: an entry following one that is
    /// not there, a head that is not there.
    pub fn apply(&mut self, change: ChainChange<E>) -> Result<(), String> {
        match change {
            ChainChange::Added(node) => {
                if self.index.contains_key(&node.digest) {
                    self.set_head(Some(&node.digest));
                } else if node.parent.is_some_and(|parent| !self.index.contains_key(&parent)) {
                    return Err(format!("entry {} follows an entry that is not there", node.digest));
                } else {
                    self.add(Node::new(node.digest, node.parent, node.entry));
                }
            }
            ChainChange::Head { to } => {
                if !self.set_head(to.as_ref()) {
                    return Err("the head moved to an entry that is not there".to_string());
                }
            }
        }
        Ok(())
    }

    /// The entries of the path `digest` names: from the root up to and
    /// including the entry with that digest, on whichever branch it is, or
    /// `None` if there is no such entry.
    pub fn prefix(&self, digest: &Digest) -> Option<Vec<&Node<E>>> {
        let end = *self.index.get(digest)?;
        Some(self.path_to(end).into_iter().map(|i| &self.nodes[i]).collect())
    }

    /// Moves the head to the entry `to` - `None`: the root, before every
    /// entry - which continues the chain from there; the entries after it
    /// stay as a branch. Every entry on the new path remembers it as the
    /// branch last taken from it. `false`, and nothing changes, if there is
    /// no such entry.
    pub fn move_head(&mut self, to: Option<&Digest>) -> bool {
        let moved = self.head != to.copied();
        if !self.set_head(to) {
            return false;
        }
        if moved {
            self.pending.push(Pending::Head(to.copied()));
        }
        true
    }

    /// `move_head` without the journal: for what is no change to store.
    fn set_head(&mut self, to: Option<&Digest>) -> bool {
        let path = match to {
            Some(digest) => match self.index.get(digest) {
                Some(&end) => self.path_to(end),
                None => return false,
            },
            None => Vec::new(),
        };
        for pair in path.windows(2) {
            let child = self.nodes[pair[1]].digest;
            self.nodes[pair[0]].active_child = Some(child);
        }
        self.head = to.copied();
        self.path = path;
        true
    }

    /// The entries following `parent` - `None`: the roots - in the order they
    /// were added: the siblings among which a branch is chosen.
    pub fn children(&self, parent: Option<&Digest>) -> Vec<&Node<E>> {
        self.nodes.iter().filter(|n| n.parent.as_ref() == parent).collect()
    }

    /// The digest of the end of the branch through `digest`, following from
    /// every entry the branch last taken from it: where a switch to that
    /// branch goes on. `None` if there is no such entry.
    pub fn leaf_from(&self, digest: &Digest) -> Option<Digest> {
        let mut at = *self.index.get(digest)?;
        while let Some(child) = self.nodes[at].active_child {
            at = self.index[&child];
        }
        Some(self.nodes[at].digest)
    }

    /// The positions from the root up to and including `end`.
    fn path_to(&self, end: usize) -> Vec<usize> {
        let mut path = vec![end];
        let mut at = end;
        while let Some(parent) = self.nodes[at].parent {
            at = self.index[&parent];
            path.push(at);
        }
        path.reverse();
        path
    }

    /// The chain of the stored `nodes` with head `head`, if they are one:
    /// every digest once, every entry after the one it follows, every
    /// remembered branch one that follows its entry, and the head one of
    /// them.
    fn from_stored(nodes: Vec<Node<E>>, head: Option<Digest>) -> Result<Self, String> {
        let mut index = HashMap::with_capacity(nodes.len());
        for (i, node) in nodes.iter().enumerate() {
            if let Some(parent) = &node.parent {
                if !index.contains_key(parent) {
                    return Err(format!("entry {} follows {parent}, which is not before it", node.digest));
                }
            }
            if index.insert(node.digest, i).is_some() {
                return Err(format!("entry {} is there twice", node.digest));
            }
        }
        for node in &nodes {
            if let Some(child) = &node.active_child {
                if index.get(child).map(|&c| nodes[c].parent.as_ref()) != Some(Some(&node.digest)) {
                    return Err(format!("entry {} remembers {child}, which does not follow it", node.digest));
                }
            }
        }
        let mut chain = Self { nodes, head: None, index, path: Vec::new(), pending: Vec::new() };
        if let Some(head) = &head {
            if !chain.index.contains_key(head) {
                return Err(format!("the head {head} is not an entry"));
            }
            chain.path = chain.path_to(chain.index[head]);
        }
        chain.head = head;
        Ok(chain)
    }
}

/// How a chain is stored.
#[derive(Serialize)]
struct StoredRef<'a, E> {
    entries: &'a [Node<E>],
    head: &'a Option<Digest>,
}

#[derive(Deserialize)]
struct Stored<E> {
    entries: Vec<Node<E>>,
    head: Option<Digest>,
}

impl<E: Serialize> Serialize for Chain<E> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        StoredRef { entries: &self.nodes, head: &self.head }.serialize(serializer)
    }
}

impl<'de, E: Deserialize<'de>> Deserialize<'de> for Chain<E> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let Stored { entries, head } = Stored::deserialize(deserializer)?;
        Chain::from_stored(entries, head).map_err(|e| serde::de::Error::custom(format!("not a chain: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chain of labels, each digested over the one before.
    fn push(chain: &mut Chain<String>, label: &str) -> Option<Digest> {
        chain.push_with(label.to_string(), |previous, label| {
            crate::digest::prompt(&format!("{}/{}", previous.map(|d| d.to_string()).unwrap_or_default(), label))
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

    fn labels<'a>(entries: impl IntoIterator<Item = &'a Node<String>>) -> Vec<String> {
        entries.into_iter().map(|e| e.entry.clone()).collect()
    }

    #[test]
    fn an_earlier_digest_names_a_prefix() {
        let mut chain = Chain::new();
        push(&mut chain, "a");
        let after_a = chain.digest().unwrap();
        push(&mut chain, "b");

        assert_eq!(labels(chain.prefix(&after_a).unwrap()), ["a"]);
        assert_eq!(labels(chain.prefix(&chain.digest().unwrap()).unwrap()), ["a", "b"]);
        assert!(chain.prefix(&crate::digest::prompt("elsewhere")).is_none());
    }

    #[test]
    fn going_on_from_an_earlier_entry_keeps_the_later_ones_as_a_branch() {
        let mut chain = Chain::new();
        push(&mut chain, "a");
        let after_a = chain.digest().unwrap();
        push(&mut chain, "b");
        let after_b = chain.digest().unwrap();

        assert!(chain.move_head(Some(&after_a)));
        assert_eq!(chain.digest(), Some(after_a));
        assert_eq!(push(&mut chain, "c"), Some(after_a), "continued from a");

        assert_eq!(labels(chain.entries()), ["a", "c"], "the active path");
        assert_eq!((chain.len(), chain.last().map(|e| e.entry.as_str())), (2, Some("c")));
        assert_eq!(labels(chain.all_entries()), ["a", "b", "c"], "b stays");
        assert_eq!(labels(chain.prefix(&after_b).unwrap()), ["a", "b"], "and names its own path");
        assert_eq!(labels(chain.children(Some(&after_a))), ["b", "c"], "siblings, as they were added");

        assert!(!chain.move_head(Some(&crate::digest::prompt("elsewhere"))), "no such entry");
        assert_eq!(labels(chain.entries()), ["a", "c"], "and nothing changed");
    }

    #[test]
    fn going_on_from_the_root_makes_another_root() {
        let mut chain = Chain::new();
        push(&mut chain, "a");
        assert!(chain.move_head(None));
        assert!(chain.is_empty() && chain.digest().is_none(), "before every entry");
        assert_eq!(push(&mut chain, "x"), None);
        assert_eq!(labels(chain.entries()), ["x"]);
        assert_eq!(labels(chain.children(None)), ["a", "x"]);
    }

    #[test]
    fn a_switch_to_a_branch_goes_on_where_it_was_left() {
        let mut chain = Chain::new();
        push(&mut chain, "a");
        let after_a = chain.digest().unwrap();
        push(&mut chain, "b");
        let after_b = chain.digest().unwrap();
        push(&mut chain, "b1");
        let after_b1 = chain.digest().unwrap();
        chain.move_head(Some(&after_b));
        push(&mut chain, "b2");
        let after_b2 = chain.digest().unwrap();
        chain.move_head(Some(&after_b1));
        chain.move_head(Some(&after_a));
        push(&mut chain, "c");

        assert_eq!(chain.leaf_from(&after_b), Some(after_b1), "b1 was last on the active path, though b2 is newer");
        assert_eq!(chain.leaf_from(&after_b2), Some(after_b2), "a leaf is its own end");
        assert_eq!(chain.leaf_from(&after_a), chain.digest(), "the active branch ends at the head");
        assert_eq!(chain.leaf_from(&crate::digest::prompt("elsewhere")), None);
    }

    #[test]
    fn the_same_entry_after_the_same_one_is_not_added_twice() {
        let mut chain = Chain::new();
        push(&mut chain, "a");
        let after_a = chain.digest().unwrap();
        push(&mut chain, "b");
        let after_b = chain.digest().unwrap();
        chain.move_head(Some(&after_a));

        assert_eq!(push(&mut chain, "b"), Some(after_a));
        assert_eq!(chain.digest(), Some(after_b), "the head is at the entry already there");
        assert_eq!(chain.all_entries().len(), 2);
    }

    #[test]
    fn a_chain_is_stored_with_its_branches_and_its_head() {
        let mut chain = Chain::new();
        push(&mut chain, "a");
        let after_a = chain.digest().unwrap();
        push(&mut chain, "b");
        chain.move_head(Some(&after_a));
        push(&mut chain, "c");

        let json = serde_json::to_value(&chain).unwrap();
        assert_eq!(json["entries"][1]["parent"], serde_json::json!(after_a.to_string()), "the entry names what it follows: {json}");
        assert_eq!(json["entries"][1]["entry"], "b", "beside its digest");
        let loaded: Chain<String> = serde_json::from_value(json).unwrap();
        assert_eq!(loaded.all_entries().collect::<Vec<_>>(), chain.all_entries().collect::<Vec<_>>());
        assert_eq!(labels(loaded.entries()), ["a", "c"]);
        assert_eq!(loaded.leaf_from(&after_a), chain.digest(), "the branch last taken is kept");
    }

    /// `chain` as it was stored: with nothing pending.
    fn stored(chain: &Chain<String>) -> Chain<String> {
        serde_json::from_value(serde_json::to_value(chain).unwrap()).unwrap()
    }

    fn same(a: &Chain<String>, b: &Chain<String>) -> bool {
        a.digest() == b.digest()
            && a.all_entries().collect::<Vec<_>>() == b.all_entries().collect::<Vec<_>>()
            && labels(a.entries()) == labels(b.entries())
            && a.all_entries().all(|e| a.leaf_from(&e.digest) == b.leaf_from(&e.digest))
    }

    #[test]
    fn the_changes_since_the_chain_was_stored_replay_onto_what_was_stored() {
        let mut chain = Chain::new();
        push(&mut chain, "a");
        let after_a = chain.digest().unwrap();
        let mut copy = stored(&chain);
        assert!(copy.changes().is_empty(), "a loaded chain has nothing pending");
        chain.mark_stored();

        push(&mut chain, "b");
        chain.move_head(Some(&after_a));
        chain.move_head(Some(&after_a));
        push(&mut chain, "c");
        chain.move_head(Some(&after_a));
        push(&mut chain, "b");

        let changes = chain.changes();
        let ops: Vec<&str> = changes.iter().map(|c| match c { ChainChange::Added(_) => "added", ChainChange::Head { .. } => "head" }).collect();
        assert_eq!(ops, ["added", "head", "added", "head", "head"], "once each, in order; a move to where the head is is no change: {changes:?}");

        for change in changes.clone() {
            copy.apply(change).unwrap();
        }
        assert!(same(&copy, &chain), "{copy:?}\n{chain:?}");
        assert_eq!(labels(copy.all_entries()), ["a", "b", "c"]);

        for change in changes {
            copy.apply(change).unwrap();
        }
        assert!(same(&copy, &chain), "applied twice, nothing changes");
        assert!(copy.changes().is_empty(), "nothing replayed is a change to store again");

        chain.mark_stored();
        assert!(chain.changes().is_empty());
    }

    #[test]
    fn a_change_that_does_not_fit_is_not_applied() {
        let mut chain = Chain::new();
        push(&mut chain, "a");
        let elsewhere = crate::digest::prompt("elsewhere");
        let orphan = Node::new(crate::digest::prompt("x"), Some(elsewhere), "x".to_string());
        assert!(chain.apply(ChainChange::Added(orphan.clone())).is_err(), "follows what is not there");
        assert!(chain.apply(ChainChange::Head { to: Some(elsewhere) }).is_err());
        assert_eq!(labels(chain.all_entries()), ["a"], "and nothing changed");
    }

    #[test]
    fn a_change_is_stored_as_the_node_added() {
        let mut chain = Chain::new();
        push(&mut chain, "a");
        push(&mut chain, "b");
        let json = serde_json::to_value(chain.changes()).unwrap();
        assert_eq!(json[1]["added"]["entry"], "b", "{json}");
        assert_eq!(json[1]["added"]["parent"], serde_json::json!(chain.prefix(&chain.digest().unwrap()).unwrap()[0].digest.to_string()));
        let loaded: Vec<ChainChange<String>> = serde_json::from_value(json).unwrap();
        assert_eq!(loaded, chain.changes());
    }

    #[test]
    fn what_is_not_a_chain_is_not_loaded() {
        let mut chain = Chain::new();
        push(&mut chain, "a");
        push(&mut chain, "b");
        let json = serde_json::to_value(&chain).unwrap();

        let mut unknown_head = json.clone();
        unknown_head["head"] = serde_json::json!(crate::digest::prompt("elsewhere").to_string());
        assert!(serde_json::from_value::<Chain<String>>(unknown_head).is_err(), "a head that is not an entry");

        let mut reversed = json.clone();
        reversed["entries"].as_array_mut().unwrap().reverse();
        assert!(serde_json::from_value::<Chain<String>>(reversed).is_err(), "an entry before the one it follows");

        let mut twice = json;
        let first = twice["entries"][0].clone();
        twice["entries"].as_array_mut().unwrap().push(first);
        assert!(serde_json::from_value::<Chain<String>>(twice).is_err(), "an entry twice");
    }
}
