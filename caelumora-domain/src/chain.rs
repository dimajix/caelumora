//! A chain of entries that is only appended to, each node carrying a digest
//! over its entry and the digest before it (issue 48): every entry follows
//! the one added before it. Nothing is ever removed, and nothing is changed
//! once added (issue 197): what stands is read from the entries.
//!
//! One digest therefore names the chain up to its entry by content. Where an
//! entry is, its position (`Node::seq`, issue 376), is counted apart from
//! what it says: the order entries were added - what a range of the chain is
//! read by. A chain may hold only the entries from a position on
//! (`Chain::suffix`): what lies before is older, and a store's. `History` is
//! a chain of messages (issue 06), an agent's notes (issue 17) and memory
//! (issue 62) chains of changes; a `Transcript` is a tree of the same nodes
//! with a head per thread (issue 602), and the one that branches.
//!
//! What goes into a digest is up to the entry's type, which computes it when
//! the entry is pushed (see `crate::digest`); the chain keeps it in the
//! entry's [`Node`] and never recomputes it: were a scheme to change,
//! recomputing would silently change every digest that refers to an entry.
//!
//! A chain changes in one way only - an entry is appended - and its stored
//! form, [`Stored`], is also what it tells a store since it was last stored
//! (issue 193): the nodes that are new, and the head. A node is stored once
//! and never changes (issue 394). `changes` tells it, `mark_stored` forgets
//! it, and `apply` merges one into a chain stored before. A store thus
//! writes what was added, not the chain, in the form it writes the chain.

use crate::digest::Digest;
use serde::de::Error;
use serde::ser;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Where an entry is in its chain (issues 376, 417): its place in the order
/// entries were added, across every branch of a transcript - the first `0`,
/// every one after it one more. Given when the entry is added and never changed; one names
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
    /// branch of a transcript (issue 376): the first entry `0`, every one
    /// after it one more.
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
/// order of their positions - and its head, the digest of the last. Also
/// what a chain tells a store since it was last stored (issue 193) - the
/// nodes added since, and the head - and what [`Chain::apply`] merges into
/// it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stored<E> {
    pub entries: Vec<Node<E>>,
    pub head: Option<Digest>,
}

/// How a chain is written: `Stored`, without a copy of the nodes.
#[derive(Serialize)]
struct StoredRef<'a, E> {
    entries: &'a [Node<E>],
    head: Option<Digest>,
}

/// Entries that are only appended to, each chained to the one before by its
/// digest. What the chain says - its digest, its entries, its last entry -
/// is read from them in the order they were added.
///
/// A chain holds its entries from the position `base` on: all of them, from
/// `0`, unless it was built as a suffix (`suffix`, issue 376). What a suffix
/// says of what lies before `base` is nothing: its first entry follows an
/// older one, and a position before `base` is not found.
#[derive(Debug, Clone)]
pub struct Chain<E> {
    /// The position of the first entry held.
    base: Position,
    /// Every entry held, in the order it was added, each following the one
    /// before it, at the position `base` plus its index.
    nodes: Vec<Node<E>>,
    /// The nodes added since the chain was last stored: positions in
    /// `nodes`, in the order they were added.
    pending: Vec<usize>,
}

impl<E> Default for Chain<E> {
    fn default() -> Self {
        Self { base: Position::START, nodes: Vec::new(), pending: Vec::new() }
    }
}

impl<E> Chain<E> {
    /// The digest of the chain, the one of its last entry, or `None` while
    /// it holds none.
    pub fn digest(&self) -> Option<Digest> {
        self.last().map(|node| node.digest)
    }

    /// Every entry it holds, in the order they were added - in a suffix,
    /// from the first of them it holds.
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &Node<E>> + ExactSizeIterator + Clone {
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

    /// The entries from the position `seq` on, in the order they were added:
    /// what was added since an entry before `seq` was. Empty from `end` on,
    /// and before `base`, where it does not hold all of them.
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

    /// The last entry.
    pub fn last(&self) -> Option<&Node<E>> {
        self.nodes.last()
    }

    /// How many entries it holds - in a suffix, from `base` on.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Appends `entry` with the digest `digest` computes for it after the
    /// last entry, and returns the digest before it.
    pub(crate) fn push_with(
        &mut self,
        entry: E,
        digest: impl FnOnce(Option<&Digest>, &E) -> Digest,
    ) -> Option<Digest> {
        let previous = self.digest();
        let digest = digest(previous.as_ref(), &entry);
        self.pending.push(self.nodes.len());
        self.nodes.push(Node::new(self.end(), digest, previous, entry));
        previous
    }

    /// What changed since the chain was last stored (`mark_stored`): the
    /// nodes added, and the head - `None` if nothing did.
    pub fn changes(&self) -> Option<Stored<E>>
    where
        E: Clone,
    {
        if self.pending.is_empty() {
            return None;
        }
        Some(Stored {
            entries: self.pending.iter().map(|&i| self.nodes[i].clone()).collect(),
            head: self.digest(),
        })
    }

    /// Forgets the changes: they are stored.
    pub fn mark_stored(&mut self) {
        self.pending.clear();
    }

    /// Merges `stored` into the chain, as a store does when loading what it
    /// appended onto the chain it stored before: a node that is there
    /// already stays as it is, a new one is added at the position after the
    /// last. Nothing it does is a change to store again, and nothing changes
    /// if `stored` does not fit. Idempotent: a chain merged into itself is
    /// the same chain, and a change merged again changes nothing, also one
    /// older than the last merged - what a journal replays onto a snapshot
    /// written after it.
    ///
    /// In a suffix that holds no entry yet, the first entry follows one that
    /// is not there: an older one, before `base`, which the store holds.
    ///
    /// # Errors
    /// If `stored` does not fit the chain: an entry at a position it holds
    /// that is not the one there, an entry at a position taken by one before
    /// it in `stored`, an entry that does not follow the one before it, a
    /// position left out or before `base`, a head that is not the last entry
    /// of `stored`.
    pub fn apply(&mut self, stored: Stored<E>) -> Result<(), String> {
        // Checked whole before anything changes. A new entry comes at the
        // position after the last, following it: the order added is the
        // order things happened, which what is read from a chain goes by,
        // and a position names one entry for good.
        let mut last = self.digest();
        let mut next = self.end();
        for node in &stored.entries {
            let Some(at) = self.index_of(node.seq) else {
                return Err(format!(
                    "entry {} is at position {}, before the first held at {}",
                    node.digest, node.seq, self.base
                ));
            };
            if let Some(there) = self.nodes.get(at) {
                if (there.digest, there.parent) != (node.digest, node.parent) {
                    return Err(format!(
                        "entry {} is not the one at position {}",
                        node.digest, node.seq
                    ));
                }
                last = Some(there.digest);
                continue;
            }
            if node.seq < next {
                return Err(format!(
                    "entry {} is at position {}, taken by an entry before it",
                    node.digest, node.seq
                ));
            }
            if node.seq != next {
                return Err(format!(
                    "entry {} is at position {}, not after the last at {next}",
                    node.digest, node.seq
                ));
            }
            let follows_older = last.is_none() && self.base > Position::START;
            if node.parent != last && !follows_older {
                return Err(format!("entry {} does not follow the one before it", node.digest));
            }
            last = Some(node.digest);
            next = next.next();
        }
        if stored.head != last {
            return Err("the head is not the last entry merged".to_string());
        }

        for node in stored.entries {
            if node.seq >= self.end() {
                self.nodes.push(node);
            }
        }
        Ok(())
    }

    /// Lets go of the entries before the position `from`, so that it holds
    /// only the part from there on, as a suffix (issue 379): what a holder
    /// keeps of a chain a store holds whole. Only entries that are stored go,
    /// and the last entry stays; otherwise, and before `base`, nothing
    /// changes.
    pub fn keep_from(&mut self, from: Position) {
        let Some(cut) = self.index_of(from) else { return };
        if cut == 0 || cut >= self.nodes.len() || self.pending.iter().any(|&at| at < cut) {
            return;
        }
        self.nodes.drain(..cut);
        self.base = self.base.after(cut as u64);
        for at in &mut self.pending {
            *at -= cut;
        }
    }

    /// The part of a chain from the position `base` on, made of `stored` -
    /// its nodes from that position on and its head - with nothing to store
    /// (issue 376): what a store gives of a chain it holds whole. The first
    /// node is at `base` and follows one before it, which is older.
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
        StoredRef { entries: &self.nodes, head: self.digest() }.serialize(serializer)
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
        assert_eq!(chain.last().map(|n| n.parent), Some(after_a), "and what it follows");
        assert_eq!(chain.digest(), chain.last().map(|n| n.digest), "the digest of the last");

        let mut other = Chain::default();
        push(&mut other, "x");
        push(&mut other, "b");
        assert_ne!(chain.digest(), other.digest(), "the same entry after another one");
    }

    fn labels<'a>(entries: impl IntoIterator<Item = &'a Node<String>>) -> Vec<String> {
        entries.into_iter().map(|e| e.entry.clone()).collect()
    }

    #[test]
    fn a_chain_is_stored_with_its_entries_and_its_head() {
        let mut chain = Chain::default();
        push(&mut chain, "a");
        let after_a = chain.digest().unwrap();
        push(&mut chain, "b");

        let json = serde_json::to_value(&chain).unwrap();
        assert_eq!(
            json["entries"][1]["parent"],
            serde_json::json!(after_a.to_string()),
            "the entry names what it follows: {json}"
        );
        assert_eq!(json["entries"][1]["entry"], "b", "beside its digest");
        assert_eq!(json["entries"][1]["seq"], 1, "and where it is");
        assert_eq!(json["head"], serde_json::json!(chain.digest().unwrap().to_string()));
        let loaded: Chain<String> = serde_json::from_value(json).unwrap();
        assert_eq!(loaded.entries().collect::<Vec<_>>(), chain.entries().collect::<Vec<_>>());
    }

    /// `chain` as it was stored: with nothing pending.
    fn stored(chain: &Chain<String>) -> Chain<String> {
        serde_json::from_value(serde_json::to_value(chain).unwrap()).unwrap()
    }

    fn same(a: &Chain<String>, b: &Chain<String>) -> bool {
        a.digest() == b.digest()
            && a.entries().collect::<Vec<_>>() == b.entries().collect::<Vec<_>>()
    }

    #[test]
    fn what_changed_since_the_chain_was_stored_is_merged_into_what_was_stored() {
        let mut chain = Chain::default();
        push(&mut chain, "a");
        push(&mut chain, "b");
        let mut copy = stored(&chain);
        assert!(copy.changes().is_none(), "a loaded chain has nothing pending");
        chain.mark_stored();
        assert!(chain.changes().is_none());

        push(&mut chain, "c");
        push(&mut chain, "d");
        let changes = chain.changes().unwrap();
        assert_eq!(labels(&changes.entries), ["c", "d"], "the entries added: {changes:?}");
        assert_eq!(changes.head, chain.digest());

        copy.apply(changes.clone()).unwrap();
        assert!(same(&copy, &chain), "{copy:?}\n{chain:?}");
        copy.apply(changes.clone()).unwrap();
        assert!(same(&copy, &chain), "merged twice, nothing changes");
        assert!(copy.changes().is_none(), "nothing merged is a change to store again");

        chain.mark_stored();
        assert!(chain.changes().is_none());
        push(&mut chain, "e");
        let newer = chain.changes().unwrap();
        copy.apply(newer.clone()).unwrap();
        copy.apply(changes.clone()).expect("an older change, as a journal replays it");
        copy.apply(newer).unwrap();
        assert!(same(&copy, &chain), "merged again, nothing changes: {copy:?}\n{chain:?}");

        let after_c = changes.entries[0].clone();
        let skipping = Node::new(chain.end(), digest::prompt("f"), chain.digest(), "f".to_string());
        assert!(
            copy.apply(Stored {
                entries: vec![after_c, skipping.clone()],
                head: Some(skipping.digest)
            })
            .is_err(),
            "a new entry follows the one before it in what is merged"
        );
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
        let x = orphan.digest;
        assert!(
            chain.apply(fits(vec![orphan.clone()], Some(x))).is_err(),
            "follows what is not the last"
        );
        let root = Node::new(Position::new(1), x, None, "x".to_string());
        assert!(chain.apply(fits(vec![root], Some(x))).is_err(), "a second root");
        assert!(
            chain.apply(fits(Vec::new(), Some(elsewhere))).is_err(),
            "a head that is not the last entry"
        );
        let next = Node::new(Position::new(1), x, Some(after_a), "x".to_string());
        assert!(
            chain.apply(fits(vec![next.clone()], Some(after_a))).is_err(),
            "a head that is not the last entry added"
        );
        let moved = Node::new(Position::new(0), after_a, Some(elsewhere), "a".to_string());
        assert!(
            chain.apply(fits(vec![moved], Some(after_a))).is_err(),
            "an entry that followed another"
        );
        assert!(chain.apply(fits(vec![next.clone(), next], Some(x))).is_err(), "an entry twice");
        let gap = Node::new(Position::new(2), digest::prompt("z"), Some(after_a), "z".to_string());
        let z = gap.digest;
        assert!(chain.apply(fits(vec![gap], Some(z))).is_err(), "a position left out");

        assert_eq!(labels(chain.entries()), ["a"], "and nothing changed");
        assert_eq!(chain.digest(), Some(after_a));
    }

    #[test]
    fn an_entry_has_its_position_in_the_order_added_and_a_range_is_read_by_it() {
        let mut chain = Chain::default();
        for label in ["a", "b", "c", "d"] {
            push(&mut chain, label);
        }

        assert_eq!(chain.entries().map(|n| n.seq.get()).collect::<Vec<_>>(), [0, 1, 2, 3]);
        assert_eq!(chain.end().get(), 4);
        assert_eq!(labels(chain.from(Position::new(1))), ["b", "c", "d"]);
        assert_eq!(
            labels(chain.from(Position::new(4))),
            Vec::<String>::new(),
            "nothing from the end on"
        );
        assert_eq!(labels(chain.from(Position::MAX)), Vec::<String>::new());

        let loaded = stored(&chain);
        assert_eq!(
            loaded.entries().map(|n| n.seq.get()).collect::<Vec<_>>(),
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
        chain.mark_stored();
        chain.keep_from(Position::new(5));
        assert_eq!(chain.base().get(), 2, "the last entry stays");
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
        for label in ["a", "b", "c", "d"] {
            push(&mut chain, label);
        }
        let after_d = chain.digest();

        let part = suffix_of(&chain, Position::new(2));
        assert_eq!(
            (part.base().get(), part.end().get(), part.digest()),
            (2, chain.end().get(), chain.digest())
        );
        assert_eq!(labels(part.entries()), ["c", "d"], "from the first entry it holds");
        assert_eq!(part.len(), 2);
        assert_eq!(labels(part.from(Position::new(3))), labels(chain.from(Position::new(3))));
        assert!(part.from(Position::new(1)).next().is_none(), "nothing before its base");
        assert_eq!(part.from(Position::new(2)).next().map(|n| n.entry.as_str()), Some("c"));
        assert!(serde_json::to_value(&part).is_err(), "a part is not written as the chain");

        let mut longer = part.clone();
        assert_eq!(push(&mut longer, "e"), after_d);
        assert_eq!(
            longer.from(Position::new(4)).next().map(|n| (n.seq.get(), n.entry.as_str())),
            Some((4, "e")),
            "appended where the whole chain would"
        );
    }

    #[test]
    fn what_the_whole_chain_changed_is_merged_into_a_suffix() {
        let mut chain = Chain::default();
        push(&mut chain, "a");
        push(&mut chain, "b");
        chain.mark_stored();
        let mut part = suffix_of(&chain, Position::new(1));

        push(&mut chain, "c");
        push(&mut chain, "d");
        part.apply(chain.changes().unwrap()).unwrap();
        assert_eq!(labels(part.entries()), ["b", "c", "d"]);
        assert_eq!((part.digest(), part.end()), (chain.digest(), chain.end()));
        assert!(part.changes().is_none(), "nothing merged is a change to store again");

        let x = Node::new(Position::new(9), digest::prompt("x"), part.digest(), "x".to_string());
        let gap = Stored { head: Some(x.digest), entries: vec![x] };
        assert!(part.apply(gap).is_err(), "positions are checked in a suffix as well");
        let before = Node::new(Position::new(0), digest::prompt("a"), None, "a".to_string());
        assert!(
            part.apply(Stored { head: part.digest(), entries: vec![before] }).is_err(),
            "and none before its base"
        );
        let older = Node::new(
            Position::new(4),
            digest::prompt("y"),
            Some(digest::prompt("older")),
            "y".to_string(),
        );
        assert!(
            Chain::suffix(
                Position::new(4),
                Stored { entries: vec![older.clone()], head: Some(older.digest) }
            )
            .is_ok(),
            "the first entry of a suffix follows an older one, which the store holds"
        );
        assert!(
            part.apply(Stored { head: Some(older.digest), entries: vec![older] }).is_err(),
            "but no entry after the first"
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
            "a head that is not the last entry"
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
