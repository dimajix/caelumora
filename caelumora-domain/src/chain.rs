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
//! `History` is a chain of messages (issue 06), an agent's notes (issue 17)
//! and memory (issue 62) chains of changes; a `Transcript` is a tree of the
//! same nodes with a head per thread (issue 602).
//!
//! What goes into a digest is up to the entry's type, which computes it when
//! the entry is pushed (see `crate::digest`); the chain keeps it in the
//! entry's [`Node`] and never recomputes it: were a scheme to change,
//! recomputing would silently change every digest that refers to an entry.
//!
//! A chain changes in two ways only - an entry is appended, the head moves -
//! and its stored form, [`Stored`], is also what it tells a store since it
//! was last stored (issue 193): the nodes that are new, and the head. A node
//! is stored once and never changes (issue 394). `changes` tells it,
//! `mark_stored` forgets it, and `apply` merges one into a chain stored
//! before. A store thus writes what was added, not the chain, in the form it
//! writes the chain.

use crate::digest::Digest;
use serde::de::Error;
use serde::ser;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{HashMap, HashSet};
use std::fmt;

/// Where an entry is in its chain (issues 376, 417): its place in the order
/// entries were added, across every branch - the first `0`, every one after
/// it one more. Given when the entry is added and never changed; one names
/// one entry of a chain for good. What a range of a chain is read by, and
/// what every reader of a history or a transcript addresses entries by - a
/// page, a delivery, a mark of what was read, a participant's range. Written as
/// its number.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Position(u64);

impl Position {
    /// The position of a chain's first entry.
    pub const START: Position = Position(0);
    /// After every position: the end of a range that is still open.
    pub const MAX: Position = Position(u64::MAX);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Its number.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The position `count` entries after it.
    pub fn after(self, count: u64) -> Self {
        Self(self.0.saturating_add(count))
    }

    /// The position `count` entries before it, the first if there are fewer.
    pub fn before(self, count: u64) -> Self {
        Self(self.0.saturating_sub(count))
    }

    /// The position after it.
    pub fn next(self) -> Self {
        self.after(1)
    }

    /// The position before it, `None` for the first.
    pub fn previous(self) -> Option<Self> {
        self.0.checked_sub(1).map(Self)
    }

    /// How many entries it lies after `earlier`, `None` if it lies before.
    pub fn since(self, earlier: Position) -> Option<u64> {
        self.0.checked_sub(earlier.0)
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// An entry at its place in the chain: with its position, the digest of the
/// chain up to and including it, and the digest of the entry it follows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node<E> {
    /// Where the entry is in the order entries were added, on whichever
    /// branch (issue 376): the first entry `0`, every one after it one more.
    /// Given when it is added and never changed, since nothing is removed;
    /// covered by no digest, since it says nothing of what was added.
    pub seq: Position,
    pub digest: Digest,
    /// The digest of the entry it follows, `None` for a root.
    pub parent: Option<Digest>,
    pub entry: E,
}

impl<E> Node<E> {
    /// `entry` at the position `seq`, after the entry `parent` names, with
    /// `digest`.
    pub(crate) fn new(seq: Position, digest: Digest, parent: Option<Digest>, entry: E) -> Self {
        Self { seq, digest, parent, entry }
    }

    /// The node with `entry` in place of its own - what a reader is given of
    /// it (issue 417) -, at its position, with its digests.
    pub fn with_entry<F>(&self, entry: F) -> Node<F> {
        Node { seq: self.seq, digest: self.digest, parent: self.parent, entry }
    }
}

/// A chain as it is stored: its nodes, in the order they were added - the
/// order of their positions - and its head. Also what a chain tells a store
/// since it was last stored (issue 193) - the nodes added since, and the
/// head - and what [`Chain::apply`] merges into it.
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
    base: Position,
    /// Every entry held, in the order it was added: an entry after the one it
    /// follows, each at the position `base` plus its index.
    nodes: Vec<Node<E>>,
    head: Option<Digest>,
    /// Where each entry is in `nodes`, by its digest.
    index: HashMap<Digest, usize>,
    /// The active path: positions in `nodes`, from the root up to the head.
    path: Vec<usize>,
    /// The nodes added since the chain was last stored: positions in
    /// `nodes`, in the order they were added.
    pending: Vec<usize>,
    /// The head as it was last stored.
    stored_head: Option<Digest>,
}

impl<E> Default for Chain<E> {
    fn default() -> Self {
        Self {
            base: Position::START,
            nodes: Vec::new(),
            head: None,
            index: HashMap::new(),
            path: Vec::new(),
            pending: Vec::new(),
            stored_head: None,
        }
    }
}

impl<E> Chain<E> {
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
    pub fn base(&self) -> Position {
        self.base
    }

    /// The position after the last entry: the one the next entry is given.
    pub fn end(&self) -> Position {
        self.base.after(self.nodes.len() as u64)
    }

    /// The entries from the position `seq` on, of every branch, in the order
    /// they were added: what was added since an entry before `seq` was. Empty
    /// from `end` on, and before `base`, where it does not hold all of them.
    pub fn from(
        &self,
        seq: Position,
    ) -> impl DoubleEndedIterator<Item = &Node<E>> + ExactSizeIterator {
        let start = self.index_of(seq).unwrap_or(self.nodes.len());
        self.nodes[start..].iter()
    }

    /// Where the position `seq` is in `nodes`: past the last for one after
    /// it; `None` for one before `base`.
    fn index_of(&self, seq: Position) -> Option<usize> {
        let offset = seq.since(self.base)?;
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
    pub(crate) fn push_with(
        &mut self,
        entry: E,
        digest: impl FnOnce(Option<&Digest>, &E) -> Digest,
    ) -> Option<Digest> {
        let previous = self.head;
        let digest = digest(previous.as_ref(), &entry);
        if self.index.contains_key(&digest) {
            self.set_head(Some(&digest));
        } else {
            self.add(Node::new(self.end(), digest, previous, entry));
        }
        previous
    }

    /// Adds `node`, whose digest is not there yet, at the position after
    /// the last, and makes it the head.
    fn add(&mut self, node: Node<E>) {
        let digest = node.digest;
        self.pending.push(self.nodes.len());
        self.insert(node);
        self.set_head(Some(&digest));
    }

    /// Puts `node`, whose digest is not there yet and whose position is
    /// `end`, after the ones there.
    fn insert(&mut self, node: Node<E>) {
        self.index.insert(node.digest, self.nodes.len());
        self.nodes.push(node);
    }

    /// What changed since the chain was last stored (`mark_stored`): the
    /// nodes added, and the head - `None` if nothing did.
    pub fn changes(&self) -> Option<Stored<E>>
    where
        E: Clone,
    {
        if self.pending.is_empty() && self.head == self.stored_head {
            return None;
        }
        Some(Stored {
            entries: self.pending.iter().map(|&i| self.nodes[i].clone()).collect(),
            head: self.head,
        })
    }

    /// Forgets the changes: they are stored.
    pub fn mark_stored(&mut self) {
        self.pending.clear();
        self.stored_head = self.head;
    }

    /// Merges `stored` into the chain, as a store does when loading what it
    /// appended onto the chain it stored before: a node that is there
    /// already stays as it is, a new one is added at its position, and the
    /// head is set. Nothing it does is a change to store again, and nothing
    /// changes if `stored` does not fit. Idempotent: a chain merged into
    /// itself is the same chain.
    ///
    /// In a suffix, an entry may follow one that is not there: an older one,
    /// before `base`, which the store holds.
    ///
    /// # Errors
    /// If `stored` does not fit the chain: a digest twice, an entry before
    /// the one it follows or following another than it did, an entry at
    /// another position than it is or than the one after the last, a head
    /// that is not there.
    pub fn apply(&mut self, stored: Stored<E>) -> Result<(), String> {
        // Checked whole before anything changes. A new entry must come
        // after the one it follows, at the position after the last: the
        // order added is the order things happened, which what is read from
        // a chain goes by, and a position names one entry for good.
        let mut incoming: HashSet<Digest> = HashSet::with_capacity(stored.entries.len());
        let mut next = self.end();
        for node in &stored.entries {
            if incoming.contains(&node.digest) {
                return Err(format!("entry {} is there twice", node.digest));
            }
            match self.index.get(&node.digest) {
                Some(&at) if self.nodes[at].parent != node.parent => {
                    return Err(format!("entry {} follows another entry than it did", node.digest));
                }
                Some(&at) if self.nodes[at].seq != node.seq => {
                    return Err(format!(
                        "entry {} is at position {}, not {}",
                        node.digest, self.nodes[at].seq, node.seq
                    ));
                }
                Some(_) => {}
                None => {
                    if node.seq != next {
                        return Err(format!(
                            "entry {} is at position {}, not after the last at {next}",
                            node.digest, node.seq
                        ));
                    }
                    next = next.next();
                    if self.base == Position::START
                        && node.parent.is_some_and(|parent| {
                            !self.index.contains_key(&parent) && !incoming.contains(&parent)
                        })
                    {
                        return Err(format!(
                            "entry {} is not after the entry it follows",
                            node.digest
                        ));
                    }
                }
            }
            incoming.insert(node.digest);
        }
        if stored
            .head
            .is_some_and(|head| !incoming.contains(&head) && !self.index.contains_key(&head))
        {
            return Err("the head is not an entry".to_string());
        }

        for node in stored.entries {
            if !self.index.contains_key(&node.digest) {
                self.insert(node);
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
    /// stay as a branch. `false`, and nothing changes, if it holds no such
    /// entry - also one before `base`, which has to be loaded first.
    pub fn move_head(&mut self, to: Option<&Digest>) -> bool {
        if to.is_some_and(|digest| !self.index.contains_key(digest)) {
            return false;
        }
        self.set_head(to);
        true
    }

    /// Sets the head to `to`, which is an entry.
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

    /// The positions in `nodes` from the root up to and including `end` -
    /// in a suffix, from the first entry of the path it holds.
    fn path_to(&self, end: usize) -> Vec<usize> {
        let mut path = vec![end];
        let mut at = end;
        while let Some(&parent) =
            self.nodes[at].parent.as_ref().and_then(|parent| self.index.get(parent))
        {
            at = parent;
            path.push(at);
        }
        path.reverse();
        path
    }

    /// Lets go of the entries before the position `from`, so that it holds
    /// only the part from there on, as a suffix (issue 379): what a holder
    /// keeps of a chain a store holds whole. Only entries that are stored go,
    /// and only while the head is from `from` on; otherwise, and before
    /// `base`, nothing changes.
    pub fn keep_from(&mut self, from: Position) {
        let Some(cut) = self.index_of(from) else { return };
        let head_kept =
            self.head.is_none_or(|head| self.index.get(&head).is_some_and(|&at| at >= cut));
        if cut == 0 || !head_kept || self.pending.iter().any(|&at| at < cut) {
            return;
        }
        let kept: Vec<Node<E>> = self.nodes.drain(cut..).collect();
        let mut suffix = Self { base: self.base.after(cut as u64), ..Self::default() };
        for node in kept {
            suffix.insert(node);
        }
        suffix.head = self.head;
        suffix.path = suffix.path_from(self.head.as_ref());
        suffix.pending = self.pending.iter().map(|&at| at - cut).collect();
        suffix.stored_head = self.stored_head;
        *self = suffix;
    }

    /// The part of a chain from the position `base` on, made of `stored` -
    /// its nodes from that position on and its head - with nothing to store
    /// (issue 376): what a store gives of a chain it holds whole. The first
    /// node is at `base`; a node may follow one before it, which is older.
    ///
    /// # Errors
    /// If `stored` does not fit, as for `apply`.
    pub fn suffix(base: Position, stored: Stored<E>) -> Result<Self, String> {
        let mut chain = Self { base, ..Self::default() };
        chain.apply(stored)?;
        Ok(chain)
    }

    /// The chain `stored` is, whole, with nothing to store.
    fn from_stored(stored: Stored<E>) -> Result<Self, String> {
        Self::suffix(Position::START, stored)
    }
}

/// Written as `Stored`, whole: a suffix is not written, since what it would
/// write is not the chain (issue 376).
impl<E: Serialize> Serialize for Chain<E> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.base != Position::START {
            return Err(ser::Error::custom(format!(
                "only the part of a chain from position {} on is held",
                self.base
            )));
        }
        StoredRef { entries: &self.nodes, head: &self.head }.serialize(serializer)
    }
}

impl<'de, E: Deserialize<'de>> Deserialize<'de> for Chain<E> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Chain::from_stored(Stored::deserialize(deserializer)?)
            .map_err(|e| Error::custom(format!("not a chain: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest;

    /// A chain of labels, each digested over the one before.
    fn push(chain: &mut Chain<String>, label: &str) -> Option<Digest> {
        chain.push_with(label.to_string(), |previous, label| {
            digest::prompt(&format!(
                "{}/{}",
                previous.map(|d| d.to_string()).unwrap_or_default(),
                label
            ))
        })
    }

    #[test]
    fn every_entry_is_chained_to_the_one_before() {
        let mut chain = Chain::default();
        assert_eq!(push(&mut chain, "a"), None, "nothing before the first");
        let after_a = chain.digest();
        assert_eq!(
            push(&mut chain, "b"),
            after_a,
            "the digest before is what an entry was made from"
        );

        let mut other = Chain::default();
        push(&mut other, "x");
        push(&mut other, "b");
        assert_ne!(chain.digest(), other.digest(), "the same entry after another one");
    }

    fn labels<'a>(entries: impl IntoIterator<Item = &'a Node<String>>) -> Vec<String> {
        entries.into_iter().map(|e| e.entry.clone()).collect()
    }

    #[test]
    fn an_earlier_digest_names_a_prefix() {
        let mut chain = Chain::default();
        push(&mut chain, "a");
        let after_a = chain.digest().unwrap();
        push(&mut chain, "b");

        assert_eq!(labels(chain.prefix(&after_a).unwrap()), ["a"]);
        assert_eq!(labels(chain.prefix(&chain.digest().unwrap()).unwrap()), ["a", "b"]);
        assert!(chain.prefix(&digest::prompt("elsewhere")).is_none());
    }

    #[test]
    fn going_on_from_an_earlier_entry_keeps_the_later_ones_as_a_branch() {
        let mut chain = Chain::default();
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

        assert!(!chain.move_head(Some(&digest::prompt("elsewhere"))), "no such entry");
        assert_eq!(labels(chain.entries()), ["a", "c"], "and nothing changed");
    }

    #[test]
    fn going_on_from_the_root_makes_another_root() {
        let mut chain = Chain::default();
        push(&mut chain, "a");
        assert!(chain.move_head(None));
        assert!(chain.is_empty() && chain.digest().is_none(), "before every entry");
        assert_eq!(push(&mut chain, "x"), None);
        assert_eq!(labels(chain.entries()), ["x"]);
        assert_eq!(labels(chain.all_entries()), ["a", "x"], "a stays");
    }

    #[test]
    fn the_same_entry_after_the_same_one_is_not_added_twice() {
        let mut chain = Chain::default();
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
        let mut chain = Chain::default();
        push(&mut chain, "a");
        let after_a = chain.digest().unwrap();
        push(&mut chain, "b");
        chain.move_head(Some(&after_a));
        push(&mut chain, "c");

        let json = serde_json::to_value(&chain).unwrap();
        assert_eq!(
            json["entries"][1]["parent"],
            serde_json::json!(after_a.to_string()),
            "the entry names what it follows: {json}"
        );
        assert_eq!(json["entries"][1]["entry"], "b", "beside its digest");
        assert_eq!(json["entries"][2]["seq"], 2, "and where it is");
        let loaded: Chain<String> = serde_json::from_value(json).unwrap();
        assert_eq!(
            loaded.all_entries().collect::<Vec<_>>(),
            chain.all_entries().collect::<Vec<_>>()
        );
        assert_eq!(labels(loaded.entries()), ["a", "c"]);
    }

    /// `chain` as it was stored: with nothing pending.
    fn stored(chain: &Chain<String>) -> Chain<String> {
        serde_json::from_value(serde_json::to_value(chain).unwrap()).unwrap()
    }

    fn same(a: &Chain<String>, b: &Chain<String>) -> bool {
        a.digest() == b.digest()
            && a.all_entries().collect::<Vec<_>>() == b.all_entries().collect::<Vec<_>>()
            && labels(a.entries()) == labels(b.entries())
    }

    #[test]
    fn what_changed_since_the_chain_was_stored_is_merged_into_what_was_stored() {
        let mut chain = Chain::default();
        push(&mut chain, "a");
        let after_a = chain.digest().unwrap();
        push(&mut chain, "b");
        let mut copy = stored(&chain);
        assert!(copy.changes().is_none(), "a loaded chain has nothing pending");
        chain.mark_stored();
        assert!(chain.changes().is_none());

        chain.move_head(Some(&after_a));
        let moved = chain.changes().expect("the head moved");
        assert_eq!(
            (labels(&moved.entries), moved.head),
            (Vec::new(), Some(after_a)),
            "the head alone: no entry changes when it moves: {moved:?}"
        );
        push(&mut chain, "c");
        chain.move_head(Some(&after_a));
        push(&mut chain, "b");
        let changes = chain.changes().unwrap();
        assert_eq!(labels(&changes.entries), ["c"], "the entry added, and no other: {changes:?}");
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
        let mut chain = Chain::default();
        push(&mut chain, "a");
        let after_a = chain.digest().unwrap();
        let elsewhere = digest::prompt("elsewhere");
        let fits = |entries: Vec<Node<String>>, head: Option<Digest>| Stored { entries, head };

        let orphan =
            Node::new(Position::new(1), digest::prompt("x"), Some(elsewhere), "x".to_string());
        assert!(
            chain.apply(fits(vec![orphan.clone()], None)).is_err(),
            "follows what is not there"
        );
        let late = Node::new(Position::new(2), elsewhere, Some(after_a), "e".to_string());
        assert!(
            chain.apply(fits(vec![orphan, late], None)).is_err(),
            "follows what comes after it"
        );
        assert!(
            chain.apply(fits(Vec::new(), Some(elsewhere))).is_err(),
            "a head that is not there"
        );
        let moved = Node::new(Position::new(0), after_a, Some(elsewhere), "a".to_string());
        assert!(
            chain.apply(fits(vec![moved], Some(after_a))).is_err(),
            "an entry that followed another"
        );
        let twice =
            Node::new(Position::new(1), digest::prompt("y"), Some(after_a), "y".to_string());
        assert!(
            chain.apply(fits(vec![twice.clone(), twice], Some(after_a))).is_err(),
            "an entry twice"
        );
        let gap = Node::new(Position::new(2), digest::prompt("z"), Some(after_a), "z".to_string());
        assert!(chain.apply(fits(vec![gap], Some(after_a))).is_err(), "a position left out");
        let shifted = Node::new(Position::new(1), after_a, None, "a".to_string());
        assert!(
            chain.apply(fits(vec![shifted], Some(after_a))).is_err(),
            "an entry at another position than it is"
        );

        assert_eq!(labels(chain.all_entries()), ["a"], "and nothing changed");
        assert_eq!(chain.digest(), Some(after_a));
    }

    #[test]
    fn an_entry_has_its_position_in_the_order_added_and_a_range_is_read_by_it() {
        let mut chain = Chain::default();
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

        assert_eq!(
            chain.all_entries().map(|n| n.seq.get()).collect::<Vec<_>>(),
            [0, 1, 2, 3],
            "across branches, one after the other"
        );
        assert_eq!(chain.end().get(), 4);
        assert_eq!(chain.prefix(&after_c).unwrap().last().map(|n| n.seq.get()), Some(2));
        assert_eq!(labels(chain.from(Position::new(1))), ["b", "c", "d"]);
        assert_eq!(
            labels(chain.from(Position::new(4))),
            Vec::<String>::new(),
            "nothing from the end on"
        );
        assert_eq!(labels(chain.from(Position::MAX)), Vec::<String>::new());

        let loaded = stored(&chain);
        assert_eq!(
            loaded.all_entries().map(|n| n.seq.get()).collect::<Vec<_>>(),
            [0, 1, 2, 3],
            "kept as they were"
        );
        let mut copy = stored(&chain);
        push(&mut chain, "e");
        copy.apply(chain.changes().unwrap()).unwrap();
        assert_eq!(
            copy.from(Position::new(4)).next().map(|n| (n.seq.get(), n.entry.as_str())),
            Some((4, "e")),
            "an entry merged is where it was added"
        );
    }

    /// The part of `chain` from `base` on, as a store would give it.
    fn suffix_of(chain: &Chain<String>, base: Position) -> Chain<String> {
        Chain::suffix(
            base,
            Stored { entries: chain.from(base).cloned().collect(), head: chain.digest() },
        )
        .unwrap()
    }

    #[test]
    fn a_chain_lets_go_of_what_is_stored_before_a_position() {
        let mut chain = Chain::default();
        for label in ["a", "b", "c", "d"] {
            push(&mut chain, label);
        }
        let mut whole = chain.clone();
        chain.keep_from(Position::new(2));
        assert_eq!(chain.base().get(), 0, "nothing goes while it is not stored");
        chain.mark_stored();
        push(&mut chain, "e");
        chain.keep_from(Position::new(2));
        assert_eq!(
            (chain.base().get(), chain.end().get(), labels(chain.entries())),
            (2, 5, vec!["c".to_string(), "d".to_string(), "e".to_string()]),
            "the stored part before it goes, what is pending stays"
        );
        let changes = chain.changes().expect("still pending");
        assert_eq!(
            changes.entries.iter().map(|n| n.seq.get()).collect::<Vec<_>>(),
            [4],
            "and is still what changed"
        );
        whole.mark_stored();
        whole.apply(changes).unwrap();
        assert_eq!(whole.digest(), chain.digest(), "which fits the whole chain");
        chain.keep_from(Position::new(1));
        assert_eq!(chain.base().get(), 2, "nothing before what it holds");
        push(&mut chain, "f");
        assert_eq!(
            chain.from(Position::new(5)).next().map(|n| n.entry.as_str()),
            Some("f"),
            "and it goes on after it"
        );
    }

    #[test]
    fn a_suffix_says_what_the_whole_chain_says_from_its_base_on() {
        let mut chain = Chain::default();
        push(&mut chain, "a");
        let after_a = chain.digest().unwrap();
        push(&mut chain, "b");
        push(&mut chain, "c");
        let after_c = chain.digest().unwrap();
        push(&mut chain, "d");
        chain.move_head(Some(&after_c));
        push(&mut chain, "e");

        let part = suffix_of(&chain, Position::new(2));
        assert_eq!(
            (part.base().get(), part.end().get(), part.digest()),
            (2, chain.end().get(), chain.digest())
        );
        assert_eq!(labels(part.all_entries()), ["c", "d", "e"]);
        assert_eq!(
            labels(part.entries()),
            ["c", "e"],
            "the active path from the first entry it holds"
        );
        assert_eq!(part.len(), 2);
        assert_eq!(labels(part.from(Position::new(3))), labels(chain.from(Position::new(3))));
        assert!(part.from(Position::new(1)).next().is_none(), "nothing before its base");
        assert_eq!(part.from(Position::new(2)).next().map(|n| n.entry.as_str()), Some("c"));
        assert!(part.prefix(&after_a).is_none(), "an older entry is not found");
        assert_eq!(labels(part.prefix(&after_c).unwrap()), ["c"]);
        assert!(serde_json::to_value(&part).is_err(), "a part is not written as the chain");

        let mut moved = part.clone();
        assert!(!moved.move_head(Some(&after_a)), "not to an older entry");
        assert!(moved.move_head(Some(&after_c)));
        assert_eq!(push(&mut moved, "f"), Some(after_c));
        assert_eq!(
            moved.from(Position::new(5)).next().map(|n| (n.seq.get(), n.entry.as_str())),
            Some((5, "f")),
            "appended where the whole chain would"
        );
    }

    #[test]
    fn what_the_whole_chain_changed_is_merged_into_a_suffix() {
        let mut chain = Chain::default();
        push(&mut chain, "a");
        push(&mut chain, "b");
        let after_b = chain.digest().unwrap();
        chain.mark_stored();
        let mut part = suffix_of(&chain, Position::new(1));

        push(&mut chain, "c");
        chain.move_head(Some(&after_b));
        push(&mut chain, "d");
        part.apply(chain.changes().unwrap()).unwrap();
        assert_eq!(labels(part.all_entries()), ["b", "c", "d"]);
        assert_eq!((part.digest(), part.end()), (chain.digest(), chain.end()));
        assert_eq!(labels(part.entries()), ["b", "d"]);
        assert!(part.changes().is_none(), "nothing merged is a change to store again");

        let gap = Stored {
            entries: vec![Node::new(
                Position::new(9),
                digest::prompt("x"),
                Some(after_b),
                "x".to_string(),
            )],
            head: None,
        };
        assert!(part.apply(gap).is_err(), "positions are checked in a suffix as well");
        let orphan = Node::new(
            Position::new(4),
            digest::prompt("y"),
            Some(digest::prompt("older")),
            "y".to_string(),
        );
        assert!(
            part.apply(Stored { entries: vec![orphan.clone()], head: part.digest() }).is_ok(),
            "an entry after an older one, which the store holds"
        );
        assert!(
            chain.apply(Stored { entries: vec![orphan], head: chain.digest() }).is_err(),
            "but not in the whole chain"
        );
    }

    #[test]
    fn what_is_not_a_chain_is_not_loaded() {
        let mut chain = Chain::default();
        push(&mut chain, "a");
        push(&mut chain, "b");
        let json = serde_json::to_value(&chain).unwrap();

        let mut unknown_head = json.clone();
        unknown_head["head"] = serde_json::json!(digest::prompt("elsewhere").to_string());
        assert!(
            serde_json::from_value::<Chain<String>>(unknown_head).is_err(),
            "a head that is not an entry"
        );

        let mut reversed = json.clone();
        reversed["entries"].as_array_mut().unwrap().reverse();
        assert!(
            serde_json::from_value::<Chain<String>>(reversed).is_err(),
            "an entry before the one it follows"
        );

        let mut twice = json;
        let first = twice["entries"][0].clone();
        twice["entries"].as_array_mut().unwrap().push(first);
        assert!(serde_json::from_value::<Chain<String>>(twice).is_err(), "an entry twice");
    }
}
