//! A chain of entries that is only appended to, each node carrying a digest
//! over its entry and the digest before it (issue 48) - a tree, since
//! branching (issue 08): an entry follows the one that was the chain's head
//! when it was added, and the head can be moved back to any entry to go on
//! from there. The entries after it stay, as a branch that is no longer
//! active. Nothing is ever removed, and nothing is changed once added
//! (issue 197): what stands is read from the entries.
//!
//! One digest therefore names the path from the root up to its entry by
//! content, and every digest on that path names a prefix of it. Where an
//! entry is, its position (`Node::seq`, issue 376), is counted apart from
//! what it says: the order entries were added, across every branch - what a
//! range of the chain is read by. A chain may hold only the entries from a
//! position on (`Chain::suffix`): what lies before is older, and a store's.
//! `History` and `Transcript` are chains of messages (issue 06), an
//! agent's notes (issue 17) and memory (issue 62) chains of changes.
//!
//! What goes into a digest is up to the entry's type, which computes it when
//! the entry is pushed (see `crate::digest`); the chain keeps it in the
//! entry's [`Node`] and never recomputes it: were a scheme to change,
//! recomputing would silently change every digest that refers to an entry.
//!
//! A chain changes in two ways only - an entry is appended, the head moves -
//! and its stored form, [`Stored`], is also what it tells a store since it
//! was last stored (issue 193): the nodes that are new or whose place
//! changed, and the head. `changes` tells it, `mark_stored` forgets it, and
//! `apply` merges one into a chain stored before. A store thus writes what
//! was added, not the chain, in the form it writes the chain.

use crate::digest::Digest;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeSet, HashMap};
use serde::de::Error;

/// An entry at its place in the chain: with its position, the digest of the
/// chain up to and including it, and the digest of the entry it follows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node<E> {
    /// Where the entry is in the order entries were added, on whichever
    /// branch (issue 376): the first entry `0`, every one after it one more.
    /// Given when it is added and never changed, since nothing is removed;
    /// covered by no digest, since it says nothing of what was added.
    pub seq: u64,
    pub digest: Digest,
    /// The digest of the entry it follows, `None` for a root.
    pub parent: Option<Digest>,
    /// Of the entries following it, the one last on the active path - where
    /// a switch to this branch goes on (`Chain::leaf_from`) - if that is not
    /// the newest of them, which it is unless the head went back to an older
    /// one. So an entry's place changes only on such a move, and never when
    /// an entry is appended after it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) active_child: Option<Digest>,
    pub entry: E,
}

impl<E> Node<E> {
    /// `entry` at the position `seq`, after the entry `parent` names, with
    /// `digest`.
    fn new(seq: u64, digest: Digest, parent: Option<Digest>, entry: E) -> Self {
        Self { seq, digest, parent, active_child: None, entry }
    }
}

/// A chain as it is stored: its nodes, in the order they were added - the
/// order of their positions - and its head. Also what a chain tells a store since it was last stored
/// (issue 193) - the nodes new or changed in their place, and the head -
/// and what [`Chain::apply`] merges into it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stored<E> {
    pub entries: Vec<Node<E>>,
    pub head: Option<Digest>,
}

/// How a chain is written: `Stored`, without a copy of the nodes.
#[derive(Serialize)]
struct StoredRef<'a, E> {
    entries: &'a [Node<E>],
    head: &'a Option<Digest>,
}

/// Entries that are only appended to, each chained to the one before by its
/// digest, as a tree with a head. What the chain says - its digest, its
/// entries, its last entry - is the active path, from the root up to the
/// head; `all_entries` are those of every branch.
///
/// A chain holds its entries from the position `base` on: all of them, from
/// `0`, unless it was built as a suffix (`suffix`, issue 376). What a suffix
/// says of what lies before `base` is nothing: an entry that follows one of
/// them follows an older entry, its paths begin at `base`, and a position or
/// digest before it is not found.
#[derive(Debug, Clone)]
pub struct Chain<E> {
    /// The position of the first entry held.
    base: u64,
    /// Every entry held, in the order it was added: an entry after the one it
    /// follows, each at the position `base` plus its index.
    nodes: Vec<Node<E>>,
    head: Option<Digest>,
    /// Where each entry is in `nodes`, by its digest.
    index: HashMap<Digest, usize>,
    /// The active path: positions in `nodes`, from the root up to the head.
    path: Vec<usize>,
    /// The newest entry following each entry that any does - where a switch
    /// to its branch goes on unless it remembers another (`active_child`).
    newest_child: HashMap<Digest, Digest>,
    /// The nodes added, or changed in their place, since the chain was last
    /// stored: positions in `nodes`, in order.
    pending: BTreeSet<usize>,
    /// The head as it was last stored.
    stored_head: Option<Digest>,
}

impl<E> Default for Chain<E> {
    fn default() -> Self {
        Self { base: 0, nodes: Vec::new(), head: None, index: HashMap::new(), path: Vec::new(), newest_child: HashMap::new(), pending: BTreeSet::new(), stored_head: None }
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

    /// The entries of the active path, from the root up to the head - in a
    /// suffix, from the first of them it holds.
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &Node<E>> + ExactSizeIterator + Clone {
        self.path.iter().map(|&i| &self.nodes[i])
    }

    /// Every entry it holds, of every branch, in the order they were added.
    pub fn all_entries(&self) -> impl DoubleEndedIterator<Item = &Node<E>> + ExactSizeIterator {
        self.nodes.iter()
    }

    /// The position of the first entry it holds: `0` unless it is a suffix.
    pub fn base(&self) -> u64 {
        self.base
    }

    /// The position after the last entry: the one the next entry is given.
    pub fn end(&self) -> u64 {
        self.base + self.nodes.len() as u64
    }

    /// The entry at the position `seq`, on whichever branch; `None` before
    /// `base` as after the last.
    pub fn at(&self, seq: u64) -> Option<&Node<E>> {
        self.nodes.get(self.index_of(seq)?)
    }

    /// The entries from the position `seq` on, of every branch, in the order
    /// they were added: what was added since an entry before `seq` was. Empty
    /// from `end` on, and before `base`, where it does not hold all of them.
    pub fn from(&self, seq: u64) -> impl DoubleEndedIterator<Item = &Node<E>> + ExactSizeIterator {
        let start = self.index_of(seq).unwrap_or(self.nodes.len());
        self.nodes[start..].iter()
    }

    /// The position of the entry with the digest `digest`, on whichever
    /// branch, or `None` if it holds no such entry.
    pub fn seq_of(&self, digest: &Digest) -> Option<u64> {
        self.index.get(digest).map(|&i| self.nodes[i].seq)
    }

    /// Where the position `seq` is in `nodes`: past the last for one after
    /// it; `None` for one before `base`.
    fn index_of(&self, seq: u64) -> Option<usize> {
        let offset = seq.checked_sub(self.base)?;
        Some(usize::try_from(offset).map_or(self.nodes.len(), |i| i.min(self.nodes.len())))
    }

    /// The entry at the head.
    pub fn last(&self) -> Option<&Node<E>> {
        self.path.last().map(|&i| &self.nodes[i])
    }

    /// The length of the active path - in a suffix, of the part it holds.
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
            self.add(Node::new(self.end(), digest, previous, entry));
        }
        previous
    }

    /// Adds `node`, whose digest is not there yet, at the position after
    /// the last, and makes it the head.
    fn add(&mut self, node: Node<E>) {
        let digest = node.digest;
        self.pending.insert(self.nodes.len());
        self.insert(node);
        self.move_head(Some(&digest));
    }

    /// Puts `node`, whose digest is not there yet and whose position is
    /// `end`, after the ones there.
    fn insert(&mut self, node: Node<E>) {
        self.index.insert(node.digest, self.nodes.len());
        if let Some(parent) = node.parent {
            self.newest_child.insert(parent, node.digest);
        }
        self.nodes.push(node);
    }

    /// What changed since the chain was last stored (`mark_stored`): the
    /// nodes added, or changed in their place, and the head - `None` if
    /// nothing did.
    pub fn changes(&self) -> Option<Stored<E>>
    where
        E: Clone,
    {
        if self.pending.is_empty() && self.head == self.stored_head {
            return None;
        }
        Some(Stored { entries: self.pending.iter().map(|&i| self.nodes[i].clone()).collect(), head: self.head })
    }

    /// Forgets the changes: they are stored.
    pub fn mark_stored(&mut self) {
        self.pending.clear();
        self.stored_head = self.head;
    }

    /// Merges `stored` into the chain, as a store does when loading what it
    /// appended onto the chain it stored before: a node that is there
    /// already takes its place from `stored` - the branch last taken from
    /// it - a new one is added at its position, and the head is set. Nothing
    /// it does is a change to store again, and nothing changes if `stored`
    /// does not fit. Idempotent: a chain merged into itself is the same
    /// chain.
    ///
    /// In a suffix, an entry may follow one that is not there: an older one,
    /// before `base`, which the store holds.
    ///
    /// # Errors
    /// If `stored` does not fit the chain: a digest twice, an entry before
    /// the one it follows or following another than it did, an entry at
    /// another position than it is or than the one after the last, a place
    /// remembered that does not follow its entry, a head that is not there.
    pub fn apply(&mut self, stored: Stored<E>) -> Result<(), String> {
        // Checked whole before anything changes. A new entry must come
        // after the one it follows, at the position after the last: the
        // order added is the order things happened, which what is read from
        // a chain goes by, and a position names one entry for good.
        let mut incoming: HashMap<Digest, Option<Digest>> = HashMap::with_capacity(stored.entries.len());
        let mut next = self.end();
        for node in &stored.entries {
            if incoming.contains_key(&node.digest) {
                return Err(format!("entry {} is there twice", node.digest));
            }
            match self.index.get(&node.digest) {
                Some(&at) if self.nodes[at].parent != node.parent => {
                    return Err(format!("entry {} follows another entry than it did", node.digest));
                }
                Some(&at) if self.nodes[at].seq != node.seq => {
                    return Err(format!("entry {} is at position {}, not {}", node.digest, self.nodes[at].seq, node.seq));
                }
                Some(_) => {}
                None => {
                    if node.seq != next {
                        return Err(format!("entry {} is at position {}, not after the last at {next}", node.digest, node.seq));
                    }
                    next += 1;
                    if self.base == 0 && node.parent.is_some_and(|parent| !self.index.contains_key(&parent) && !incoming.contains_key(&parent)) {
                        return Err(format!("entry {} is not after the entry it follows", node.digest));
                    }
                }
            }
            incoming.insert(node.digest, node.parent);
        }
        let parent_of = |digest: &Digest| -> Option<Option<Digest>> {
            incoming.get(digest).copied().or_else(|| self.index.get(digest).map(|&i| self.nodes[i].parent))
        };
        for node in &stored.entries {
            if let Some(child) = &node.active_child {
                if parent_of(child) != Some(Some(node.digest)) {
                    return Err(format!("entry {} remembers {child}, which does not follow it", node.digest));
                }
            }
        }
        if stored.head.is_some_and(|head| parent_of(&head).is_none()) {
            return Err("the head is not an entry".to_string());
        }

        for node in stored.entries {
            match self.index.get(&node.digest) {
                Some(&at) => self.nodes[at].active_child = node.active_child,
                None => self.insert(node),
            }
        }
        self.set_head(stored.head.as_ref());
        self.stored_head = self.head;
        Ok(())
    }

    /// The entries of the path `digest` names: from the root up to and
    /// including the entry with that digest, on whichever branch it is, or
    /// `None` if it holds no such entry. In a suffix, the part of the path
    /// from `base` on.
    pub fn prefix(&self, digest: &Digest) -> Option<Vec<&Node<E>>> {
        let end = *self.index.get(digest)?;
        Some(self.path_to(end).into_iter().map(|i| &self.nodes[i]).collect())
    }

    /// Moves the head to the entry `to` - `None`: the root, before every
    /// entry - which continues the chain from there; the entries after it
    /// stay as a branch. Every entry on the new path remembers it as the
    /// branch last taken from it. `false`, and nothing changes, if it holds
    /// no such entry - also one before `base`, which has to be loaded first.
    pub fn move_head(&mut self, to: Option<&Digest>) -> bool {
        if to.is_some_and(|digest| !self.index.contains_key(digest)) {
            return false;
        }
        let path = self.path_from(to);
        for pair in path.windows(2) {
            let (parent, child) = (self.nodes[pair[0]].digest, self.nodes[pair[1]].digest);
            // Remembered only where it is not the newest: the common case,
            // an entry appended after the head, changes nothing before it.
            let remembered = (self.newest_child.get(&parent) != Some(&child)).then_some(child);
            if self.nodes[pair[0]].active_child != remembered {
                self.nodes[pair[0]].active_child = remembered;
                self.pending.insert(pair[0]);
            }
        }
        self.head = to.copied();
        self.path = path;
        true
    }

    /// Sets the head to `to`, which is an entry, as it is placed - without
    /// changing what any entry remembers (`apply`).
    fn set_head(&mut self, to: Option<&Digest>) {
        self.path = self.path_from(to);
        self.head = to.copied();
    }

    /// The positions from the root up to and including the entry `to`
    /// names; empty for `None`.
    fn path_from(&self, to: Option<&Digest>) -> Vec<usize> {
        match to.and_then(|digest| self.index.get(digest)) {
            Some(&end) => self.path_to(end),
            None => Vec::new(),
        }
    }

    /// The entries following `parent` - `None`: the roots - in the order they
    /// were added: the siblings among which a branch is chosen.
    pub fn children(&self, parent: Option<&Digest>) -> Vec<&Node<E>> {
        self.nodes.iter().filter(|n| n.parent.as_ref() == parent).collect()
    }

    /// The digest of the end of the branch through `digest`, following from
    /// every entry the branch last taken from it - the newest unless it
    /// remembers another: where a switch to that branch goes on. `None` if
    /// there is no such entry.
    pub fn leaf_from(&self, digest: &Digest) -> Option<Digest> {
        let mut at = *self.index.get(digest)?;
        while let Some(child) = self.nodes[at].active_child.or_else(|| self.newest_child.get(&self.nodes[at].digest).copied()) {
            at = self.index[&child];
        }
        Some(self.nodes[at].digest)
    }

    /// The positions in `nodes` from the root up to and including `end` -
    /// in a suffix, from the first entry of the path it holds.
    fn path_to(&self, end: usize) -> Vec<usize> {
        let mut path = vec![end];
        let mut at = end;
        while let Some(&parent) = self.nodes[at].parent.as_ref().and_then(|parent| self.index.get(parent)) {
            at = parent;
            path.push(at);
        }
        path.reverse();
        path
    }

    /// The part of a chain from the position `base` on, made of `stored` -
    /// its nodes from that position on and its head - with nothing to store
    /// (issue 376): what a store gives of a chain it holds whole. The first
    /// node is at `base`; a node may follow one before it, which is older.
    ///
    /// # Errors
    /// If `stored` does not fit, as for `apply`.
    pub fn suffix(base: u64, stored: Stored<E>) -> Result<Self, String> {
        let mut chain = Self { base, ..Self::default() };
        chain.apply(stored)?;
        Ok(chain)
    }

    /// The chain `stored` is, whole, with nothing to store.
    fn from_stored(stored: Stored<E>) -> Result<Self, String> {
        Self::suffix(0, stored)
    }
}

/// Written as `Stored`, whole: a suffix is not written, since what it would
/// write is not the chain (issue 376).
impl<E: Serialize> Serialize for Chain<E> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.base != 0 {
            return Err(serde::ser::Error::custom(format!("only the part of a chain from position {} on is held", self.base)));
        }
        StoredRef { entries: &self.nodes, head: &self.head }.serialize(serializer)
    }
}

impl<'de, E: Deserialize<'de>> Deserialize<'de> for Chain<E> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Chain::from_stored(Stored::deserialize(deserializer)?).map_err(|e| Error::custom(format!("not a chain: {e}")))
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
        assert_eq!(json["entries"][2]["seq"], 2, "and where it is");
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
    fn what_changed_since_the_chain_was_stored_is_merged_into_what_was_stored() {
        let mut chain = Chain::new();
        push(&mut chain, "a");
        let after_a = chain.digest().unwrap();
        push(&mut chain, "b");
        let mut copy = stored(&chain);
        assert!(copy.changes().is_none(), "a loaded chain has nothing pending");
        chain.mark_stored();
        assert!(chain.changes().is_none());

        chain.move_head(Some(&after_a));
        let moved = chain.changes().expect("the head moved");
        assert_eq!((labels(&moved.entries), moved.head), (Vec::new(), Some(after_a)), "the head alone: b is still where a goes on: {moved:?}");
        chain.move_head(Some(&after_a));
        push(&mut chain, "c");
        assert_eq!(labels(&chain.changes().unwrap().entries), ["c"], "an entry appended changes nothing before it");
        chain.move_head(Some(&after_a));
        push(&mut chain, "b");
        let changes = chain.changes().unwrap();
        assert_eq!(labels(&changes.entries), ["a", "c"], "a remembers b, which is not its newest; c is new: {changes:?}");
        assert_eq!(changes.head, chain.digest());

        copy.apply(changes.clone()).unwrap();
        assert!(same(&copy, &chain), "{copy:?}\n{chain:?}");
        copy.apply(changes).unwrap();
        assert!(same(&copy, &chain), "merged twice, nothing changes");
        assert!(copy.changes().is_none(), "nothing merged is a change to store again");

        chain.mark_stored();
        assert!(chain.changes().is_none());
    }

    #[test]
    fn what_does_not_fit_is_not_merged() {
        let mut chain = Chain::new();
        push(&mut chain, "a");
        let after_a = chain.digest().unwrap();
        let elsewhere = crate::digest::prompt("elsewhere");
        let fits = |entries: Vec<Node<String>>, head: Option<Digest>| Stored { entries, head };

        let orphan = Node::new(1, crate::digest::prompt("x"), Some(elsewhere), "x".to_string());
        assert!(chain.apply(fits(vec![orphan.clone()], None)).is_err(), "follows what is not there");
        let late = Node::new(2, elsewhere, Some(after_a), "e".to_string());
        assert!(chain.apply(fits(vec![orphan, late], None)).is_err(), "follows what comes after it");
        assert!(chain.apply(fits(Vec::new(), Some(elsewhere))).is_err(), "a head that is not there");
        let moved = Node::new(0, after_a, Some(elsewhere), "a".to_string());
        assert!(chain.apply(fits(vec![moved], Some(after_a))).is_err(), "an entry that followed another");
        let remembers = Node { active_child: Some(elsewhere), ..Node::new(0, after_a, None, "a".to_string()) };
        assert!(chain.apply(fits(vec![remembers], Some(after_a))).is_err(), "remembers what does not follow it");
        let twice = Node::new(1, crate::digest::prompt("y"), Some(after_a), "y".to_string());
        assert!(chain.apply(fits(vec![twice.clone(), twice], Some(after_a))).is_err(), "an entry twice");
        let gap = Node::new(2, crate::digest::prompt("z"), Some(after_a), "z".to_string());
        assert!(chain.apply(fits(vec![gap], Some(after_a))).is_err(), "a position left out");
        let shifted = Node::new(1, after_a, None, "a".to_string());
        assert!(chain.apply(fits(vec![shifted], Some(after_a))).is_err(), "an entry at another position than it is");

        assert_eq!(labels(chain.all_entries()), ["a"], "and nothing changed");
        assert_eq!(chain.digest(), Some(after_a));
    }

    #[test]
    fn an_entry_has_its_position_in_the_order_added_and_a_range_is_read_by_it() {
        let mut chain = Chain::new();
        push(&mut chain, "a");
        let after_a = chain.digest().unwrap();
        push(&mut chain, "b");
        chain.move_head(Some(&after_a));
        push(&mut chain, "c");
        let after_c = chain.digest().unwrap();
        push(&mut chain, "d");
        chain.move_head(Some(&after_a));
        // c after a is there already: the head moves, no position is taken.
        push(&mut chain, "c");

        assert_eq!(chain.all_entries().map(|n| n.seq).collect::<Vec<_>>(), [0, 1, 2, 3], "across branches, one after the other");
        assert_eq!(chain.end(), 4);
        assert_eq!(chain.at(1).map(|n| n.entry.as_str()), Some("b"));
        assert!(chain.at(4).is_none() && chain.at(u64::MAX).is_none(), "nothing past the last");
        assert_eq!(chain.seq_of(&after_c), Some(2));
        assert_eq!(chain.seq_of(&crate::digest::prompt("elsewhere")), None);
        assert_eq!(labels(chain.from(1)), ["b", "c", "d"]);
        assert_eq!(labels(chain.from(4)), Vec::<String>::new(), "nothing from the end on");
        assert_eq!(labels(chain.from(u64::MAX)), Vec::<String>::new());

        let loaded = stored(&chain);
        assert_eq!(loaded.all_entries().map(|n| n.seq).collect::<Vec<_>>(), [0, 1, 2, 3], "kept as they were");
        let mut copy = stored(&chain);
        push(&mut chain, "e");
        copy.apply(chain.changes().unwrap()).unwrap();
        assert_eq!(copy.at(4).map(|n| (n.seq, n.entry.as_str())), Some((4, "e")), "an entry merged is where it was added");
    }

    /// The part of `chain` from `base` on, as a store would give it.
    fn suffix_of(chain: &Chain<String>, base: u64) -> Chain<String> {
        Chain::suffix(base, Stored { entries: chain.from(base).cloned().collect(), head: chain.digest() }).unwrap()
    }

    #[test]
    fn a_suffix_says_what_the_whole_chain_says_from_its_base_on() {
        let mut chain = Chain::new();
        push(&mut chain, "a");
        let after_a = chain.digest().unwrap();
        push(&mut chain, "b");
        push(&mut chain, "c");
        let after_c = chain.digest().unwrap();
        push(&mut chain, "d");
        chain.move_head(Some(&after_c));
        push(&mut chain, "e");

        let part = suffix_of(&chain, 2);
        assert_eq!((part.base(), part.end(), part.digest()), (2, chain.end(), chain.digest()));
        assert_eq!(labels(part.all_entries()), ["c", "d", "e"]);
        assert_eq!(labels(part.entries()), ["c", "e"], "the active path from the first entry it holds");
        assert_eq!(part.len(), 2);
        assert_eq!(labels(part.from(3)), labels(chain.from(3)));
        assert!(part.from(1).next().is_none() && part.at(1).is_none(), "nothing before its base");
        assert_eq!(part.at(2).map(|n| n.entry.as_str()), Some("c"));
        assert_eq!(part.seq_of(&after_c), Some(2));
        assert!(part.prefix(&after_a).is_none() && part.seq_of(&after_a).is_none(), "an older entry is not found");
        assert_eq!(labels(part.prefix(&after_c).unwrap()), ["c"]);
        assert_eq!(part.leaf_from(&after_c), chain.digest());
        assert!(serde_json::to_value(&part).is_err(), "a part is not written as the chain");

        let mut moved = part.clone();
        assert!(!moved.move_head(Some(&after_a)), "not to an older entry");
        assert!(moved.move_head(Some(&after_c)));
        assert_eq!(push(&mut moved, "f"), Some(after_c));
        assert_eq!(moved.at(5).map(|n| (n.seq, n.entry.as_str())), Some((5, "f")), "appended where the whole chain would");
    }

    #[test]
    fn what_the_whole_chain_changed_is_merged_into_a_suffix() {
        let mut chain = Chain::new();
        push(&mut chain, "a");
        push(&mut chain, "b");
        let after_b = chain.digest().unwrap();
        chain.mark_stored();
        let mut part = suffix_of(&chain, 1);

        push(&mut chain, "c");
        chain.move_head(Some(&after_b));
        push(&mut chain, "d");
        part.apply(chain.changes().unwrap()).unwrap();
        assert_eq!(labels(part.all_entries()), ["b", "c", "d"]);
        assert_eq!((part.digest(), part.end()), (chain.digest(), chain.end()));
        assert_eq!(labels(part.entries()), ["b", "d"]);
        assert_eq!(part.leaf_from(&after_b), chain.digest());
        assert!(part.changes().is_none(), "nothing merged is a change to store again");

        let gap = Stored { entries: vec![Node::new(9, crate::digest::prompt("x"), Some(after_b), "x".to_string())], head: None };
        assert!(part.apply(gap).is_err(), "positions are checked in a suffix as well");
        let orphan = Node::new(4, crate::digest::prompt("y"), Some(crate::digest::prompt("older")), "y".to_string());
        assert!(part.apply(Stored { entries: vec![orphan.clone()], head: part.digest() }).is_ok(), "an entry after an older one, which the store holds");
        assert!(chain.apply(Stored { entries: vec![orphan], head: chain.digest() }).is_err(), "but not in the whole chain");
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
