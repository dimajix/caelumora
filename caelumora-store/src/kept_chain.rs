//! How a store keeps a chain a state holds (issues 379, 380): its nodes by
//! position, as the puts of its holder appended them, and its head - not as
//! a `Chain`, which makes a digest unique. A holder that keeps only a window
//! of its chain dedupes only within it, so that a transcript may add an
//! entry a second time whose first lies before its window; the same digest
//! names the same content after the same predecessors, and the store keeps
//! both.

use caelumora_domain::chain::{Chain, Node, Position, Stored};
use caelumora_domain::digest::Digest;

/// The nodes of `entries` - what a chain added since it was stored - that a
/// store keeping it up to the position `end` takes in: those from `end` on,
/// each right after the last. A node before `end` is there already - stored
/// by a put its holder did not learn had succeeded - and stays as it is,
/// since a node never changes once stored (issue 394).
///
/// # Errors
/// If a node lies past the one after the last.
pub fn new_nodes<E>(end: Position, entries: Vec<Node<E>>) -> Result<Vec<Node<E>>, String> {
    let added: Vec<Node<E>> = entries.into_iter().filter(|node| node.seq >= end).collect();
    let mut next = end;
    for node in &added {
        if node.seq != next {
            return Err(format!(
                "an entry at position {} after the one after the last, {}",
                node.seq, next
            ));
        }
        next = next.next();
    }
    Ok(added)
}

/// Follows the path to an entry back through `older` - nodes of a chain,
/// newest first, each older than those gone through before -: takes the
/// node `want` names into `path`, and wants the one it follows next, the
/// newest node with that digest before it. Done once `want` is `None`.
pub fn follow_back<'a, E: Clone + 'a>(
    want: &mut Option<Digest>,
    path: &mut Vec<Node<E>>,
    older: impl IntoIterator<Item = &'a Node<E>>,
) {
    for node in older {
        if want.is_none() {
            return;
        }
        if *want == Some(node.digest) {
            path.push(node.clone());
            *want = node.parent;
        }
    }
}

/// The path `follow_back` gathered, from the root on: `None` if the entry
/// it was to end at was not found.
///
/// # Errors
/// If the path was not followed back to its root: an entry it holds follows
/// one that is not there.
pub fn followed<E>(
    want: Option<Digest>,
    mut path: Vec<Node<E>>,
) -> Result<Option<Vec<Node<E>>>, String> {
    if path.is_empty() {
        return Ok(None);
    }
    if let Some(missing) = want {
        return Err(format!("the entry {missing} on the path is not there"));
    }
    path.reverse();
    Ok(Some(path))
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
    /// Takes in `changes`, what the holder's chain added since it was
    /// stored, whole or not at all.
    ///
    /// # Errors
    /// If they do not fit (`new_nodes`).
    pub(crate) fn append(&mut self, changes: Stored<E>) -> Result<(), String> {
        let added = new_nodes(self.end(), changes.entries)?;
        self.nodes.extend(added);
        self.head = changes.head;
        Ok(())
    }

    /// The position after the last node.
    pub(crate) fn end(&self) -> Position {
        Position::START.after(self.nodes.len() as u64)
    }

    /// The digest of the chain's active path.
    pub(crate) fn head(&self) -> Option<Digest> {
        self.head
    }

    /// The nodes from the position `seq` on, in the order they were added.
    pub(crate) fn from(&self, seq: Position) -> &[Node<E>] {
        let start =
            usize::try_from(seq.get()).map_or(self.nodes.len(), |at| at.min(self.nodes.len()));
        &self.nodes[start..]
    }

    /// The path from the root up to the entry with the digest `digest`, on
    /// whichever branch, the newest such entry if one is there twice;
    /// `None` if none is.
    ///
    /// # Errors
    /// If the path does not lead back to a root.
    pub(crate) fn path(&self, digest: Digest) -> Result<Option<Vec<Node<E>>>, String> {
        let mut want = Some(digest);
        let mut path = Vec::new();
        follow_back(&mut want, &mut path, self.nodes.iter().rev());
        followed(want, path)
    }

    /// The chain from the position `from` on, as its holder keeps it.
    ///
    /// # Errors
    /// If what is kept from there is no chain.
    pub(crate) fn suffix(&self, from: Position) -> Result<Chain<E>, String> {
        Chain::suffix(from, Stored { entries: self.from(from).to_vec(), head: self.head })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::digest;

    fn node(seq: u64, label: &str) -> Node<String> {
        serde_json::from_value(serde_json::json!({ "seq": seq, "digest": digest::prompt(label), "parent": null, "entry": label })).unwrap()
    }

    #[test]
    fn a_chain_is_kept_by_position_and_a_digest_may_be_there_twice() {
        let mut kept = KeptChain::default();
        kept.append(Stored {
            entries: vec![node(0, "a"), node(1, "b")],
            head: Some(digest::prompt("b")),
        })
        .unwrap();
        kept.append(Stored {
            entries: vec![node(1, "b"), node(2, "a")],
            head: Some(digest::prompt("a")),
        })
        .expect("one there already, and a digest again");
        kept.append(Stored { entries: vec![node(2, "x")], head: Some(digest::prompt("a")) })
            .expect("there already");
        assert_eq!(
            kept.from(Position::new(2)).iter().map(|n| n.entry.as_str()).collect::<Vec<_>>(),
            ["a"],
            "a node stored stays as it is"
        );
        assert_eq!(kept.end().get(), 3);
        assert_eq!(
            kept.from(Position::new(1)).iter().map(|n| n.entry.as_str()).collect::<Vec<_>>(),
            ["b", "a"]
        );
        assert!(
            kept.append(Stored { entries: vec![node(4, "c")], head: None }).is_err(),
            "past the one after the last"
        );
        assert_eq!(
            (kept.end().get(), kept.head()),
            (3, Some(digest::prompt("a"))),
            "and nothing taken in"
        );
    }

    #[test]
    fn a_path_is_followed_back_from_the_newest_entry_with_its_digest() {
        let child = |seq: u64, label: &str, parent: &str| -> Node<String> {
            serde_json::from_value(serde_json::json!({ "seq": seq, "digest": digest::prompt(label), "parent": digest::prompt(parent), "entry": label })).unwrap()
        };
        let mut kept = KeptChain::default();
        // a - b, then anew: a again, and c after it.
        let entries = vec![node(0, "a"), child(1, "b", "a"), node(2, "a"), child(3, "c", "a")];
        kept.append(Stored { entries, head: Some(digest::prompt("c")) }).unwrap();
        let seqs = |path: Option<Vec<Node<String>>>| {
            path.map(|path| path.iter().map(|n| n.seq.get()).collect::<Vec<_>>())
        };
        assert_eq!(
            seqs(kept.path(digest::prompt("c")).unwrap()),
            Some(vec![2, 3]),
            "through the newest a before it"
        );
        assert_eq!(
            seqs(kept.path(digest::prompt("b")).unwrap()),
            Some(vec![0, 1]),
            "through the a before it"
        );
        assert_eq!(seqs(kept.path(digest::prompt("x")).unwrap()), None);
        let mut broken = KeptChain::default();
        broken.append(Stored { entries: vec![child(0, "b", "a")], head: None }).unwrap();
        assert!(broken.path(digest::prompt("b")).is_err(), "what it follows is not there");
    }
}
