//! How a store keeps a chain a state holds (issues 379, 380): its nodes by
//! position, as the puts of its holder appended them, and its head - not as
//! a `Chain`, which makes a digest unique. A holder that keeps only a window
//! of its chain dedupes only within it, so that a transcript may add an
//! entry a second time whose first lies before its window; the same digest
//! names the same content after the same predecessors, and the store keeps
//! both.

use crate::chain::{Chain, Node, Stored};
use crate::digest::Digest;

/// Whether the nodes of `entries` - what a chain added, or changed in its
/// place, since it was stored - fit what a store keeps of it up to the
/// position `end`: each is there already, or comes right after the last.
/// Returns the position after the last when they are taken in.
///
/// # Errors
/// If a node lies past the one after the last.
pub(crate) fn fits<E>(end: u64, entries: &[Node<E>]) -> Result<u64, String> {
    let mut next = end;
    for node in entries.iter().filter(|node| node.seq >= end) {
        if node.seq != next {
            return Err(format!("an entry at position {} after the last at {}", node.seq, next.saturating_sub(1)));
        }
        next += 1;
    }
    Ok(next)
}

/// Puts `node`, which fits (`fits`), into `nodes` - the nodes a store keeps
/// from the position `from` on -: in its place, the branch last taken from
/// it, if it is there; after the last otherwise.
pub(crate) fn place<E>(nodes: &mut Vec<Node<E>>, from: u64, node: Node<E>) {
    match usize::try_from(node.seq - from).ok().filter(|&at| at < nodes.len()) {
        Some(at) => nodes[at] = node,
        None => nodes.push(node),
    }
}

/// A chain as the `MemoryStore` keeps it: every node, by position, and the
/// head.
#[derive(Debug)]
pub(crate) struct KeptChain<E> {
    nodes: Vec<Node<E>>,
    head: Option<Digest>,
}

impl<E> Default for KeptChain<E> {
    fn default() -> Self {
        Self { nodes: Vec::new(), head: None }
    }
}

impl<E: Clone> KeptChain<E> {
    /// Takes in `changes`, what the holder's chain added or changed in its
    /// place since it was stored, whole or not at all.
    ///
    /// # Errors
    /// If they do not fit (`fits`).
    pub(crate) fn append(&mut self, changes: Stored<E>) -> Result<(), String> {
        fits(self.end(), &changes.entries)?;
        for node in changes.entries {
            place(&mut self.nodes, 0, node);
        }
        self.head = changes.head;
        Ok(())
    }

    /// The position after the last node.
    pub(crate) fn end(&self) -> u64 {
        self.nodes.len() as u64
    }

    /// The digest of the chain's active path.
    pub(crate) fn head(&self) -> Option<Digest> {
        self.head
    }

    /// The nodes from the position `seq` on, in the order they were added.
    pub(crate) fn from(&self, seq: u64) -> &[Node<E>] {
        let start = usize::try_from(seq).map_or(self.nodes.len(), |at| at.min(self.nodes.len()));
        &self.nodes[start..]
    }

    /// The chain from the position `from` on, as its holder keeps it.
    ///
    /// # Errors
    /// If what is kept from there is no chain.
    pub(crate) fn suffix(&self, from: u64) -> Result<Chain<E>, String> {
        Chain::suffix(from, Stored { entries: self.from(from).to_vec(), head: self.head })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest;

    fn node(seq: u64, label: &str) -> Node<String> {
        serde_json::from_value(serde_json::json!({ "seq": seq, "digest": digest::prompt(label), "parent": null, "entry": label })).unwrap()
    }

    #[test]
    fn a_chain_is_kept_by_position_and_a_digest_may_be_there_twice() {
        let mut kept = KeptChain::default();
        kept.append(Stored { entries: vec![node(0, "a"), node(1, "b")], head: Some(digest::prompt("b")) }).unwrap();
        kept.append(Stored { entries: vec![node(1, "b"), node(2, "a")], head: Some(digest::prompt("a")) }).expect("in its place, and a digest again");
        assert_eq!(kept.end(), 3);
        assert_eq!(kept.from(1).iter().map(|n| n.entry.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        assert!(kept.append(Stored { entries: vec![node(4, "c")], head: None }).is_err(), "past the one after the last");
        assert_eq!((kept.end(), kept.head()), (3, Some(digest::prompt("a"))), "and nothing taken in");
    }
}
