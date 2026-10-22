//! A chain a state holds, as the `JsonFileStore` keeps it (issues 379,
//! 380): per chain a directory beside its state's snapshot, with the nodes
//! in segments of `SEGMENT` entries by position (`chain-<from>.json`) and
//! `head.json`, which says where the chain ends and what its head is. A put
//! writes the segments, then the head: nodes a crash left past the head's
//! end are not there. What is laid over a chain - a history's items - keeps
//! files of its own beside these.

use super::{read_json_opt, to_backend_error, write_json_atomic};
use caelumora_model::chain::{Position, Chain, Node, Stored};
use caelumora_model::digest::Digest;
use caelumora_model::store::kept_chain::{fits, follow_back, followed, place};
use caelumora_model::store::{StoreError, StoreResult};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use tracing::debug;

/// How many entries of a chain one file holds.
pub(super) const SEGMENT: u64 = 256;

/// Where a chain stands: its head, and the position after its last entry.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct ChainHead {
    pub(super) head: Option<Digest>,
    pub(super) end: Position,
}

/// The position of the first entry of the segment that holds `seq`.
pub(super) fn segment_of(seq: Position) -> Position {
    Position::new(seq.get() - seq.get() % SEGMENT)
}

/// The segment before the one from `segment`, `None` before the first.
pub(super) fn segment_before(segment: Position) -> Option<Position> {
    segment.get().checked_sub(SEGMENT).map(Position::new)
}

fn segment_path(dir: &Path, from: Position) -> PathBuf {
    dir.join(format!("chain-{from}.json"))
}

fn head_path(dir: &Path) -> PathBuf {
    dir.join("head.json")
}

/// Where the chain in `dir` stands; at its start if none is kept there.
pub(super) async fn read_head(dir: &Path) -> StoreResult<ChainHead> {
    Ok(read_json_opt(&head_path(dir)).await?.unwrap_or_default())
}

/// The entries of the segment from `from`, as written - past the head's end
/// too, if a crash left some.
async fn read_segment<E: DeserializeOwned>(dir: &Path, from: Position) -> StoreResult<Vec<Node<E>>> {
    Ok(read_json_opt(&segment_path(dir, from)).await?.unwrap_or_default())
}

/// The entries from the position `from` up to `end`, in order.
pub(super) async fn read_chain<E: DeserializeOwned>(dir: &Path, from: Position, end: Position) -> StoreResult<Vec<Node<E>>> {
    let mut nodes = Vec::new();
    let mut segment = segment_of(from);
    while segment < end {
        nodes.extend(read_segment(dir, segment).await?.into_iter().filter(|node| node.seq >= from && node.seq < end));
        segment = segment.after(SEGMENT);
    }
    Ok(nodes)
}

/// Writes `changes` - what the holder's chain added, or changed in its
/// place, since it was stored - into the chain `key` names, kept in `dir`:
/// the segments, then the head. Returns where the chain stands now, and the
/// nodes added.
///
/// # Errors
/// If they do not fit (`fits`), or cannot be written.
pub(super) async fn append<E>(dir: &Path, key: impl Display, changes: Stored<E>) -> StoreResult<(ChainHead, Vec<Node<E>>)>
where
    E: Serialize + DeserializeOwned + Clone,
{
    tokio::fs::create_dir_all(dir).await.map_err(to_backend_error)?;
    let head = read_head(dir).await?;
    let end = fits(head.end, &changes.entries).map_err(|e| StoreError::Backend(format!("what {key} added does not fit: {e}")))?;
    let mut added = Vec::new();
    let mut by_segment: BTreeMap<Position, Vec<Node<E>>> = BTreeMap::new();
    for node in changes.entries {
        if node.seq >= head.end {
            added.push(node.clone());
        }
        by_segment.entry(segment_of(node.seq)).or_default().push(node);
    }
    for (from, nodes) in by_segment {
        let mut stored = read_segment(dir, from).await?;
        stored.retain(|node| node.seq < head.end);
        for node in nodes {
            place(&mut stored, from, node);
        }
        debug!("Writing the entries of {} from position {}", key, from);
        write_json_atomic(&segment_path(dir, from), &stored).await?;
    }
    let head = ChainHead { head: changes.head, end };
    write_json_atomic(&head_path(dir), &head).await?;
    Ok((head, added))
}

/// The chain `key` names, kept in `dir`, from the position `from` on, as a
/// holder keeps it; empty if none is kept.
///
/// # Errors
/// If it cannot be read, or what is read is no chain.
pub(super) async fn load<E: DeserializeOwned>(dir: &Path, key: impl Display, from: Position) -> StoreResult<Chain<E>> {
    let head = read_head(dir).await?;
    let entries = read_chain(dir, from, head.end).await?;
    Chain::suffix(from, Stored { entries, head: head.head })
        .map_err(|e| StoreError::Backend(format!("{key} cannot be read from position {from}: {e}")))
}

/// The path from the root up to the entry with the digest `digest` of the
/// chain `key` names, kept in `dir`, on whichever branch - the newest such
/// entry if one is there twice -, read backwards segment by segment; `None`
/// if there is none.
///
/// # Errors
/// If it cannot be read, or the path does not lead back to a root.
pub(super) async fn path<E: DeserializeOwned + Clone>(dir: &Path, key: impl Display, digest: Digest) -> StoreResult<Option<Vec<Node<E>>>> {
    let end = read_head(dir).await?.end;
    let mut want = Some(digest);
    let mut path = Vec::new();
    let mut segment = end.previous().map(segment_of);
    while let Some(from) = segment.filter(|_| want.is_some()) {
        let nodes: Vec<Node<E>> = read_segment(dir, from).await?;
        follow_back(&mut want, &mut path, nodes.iter().rev().filter(|node| node.seq < end));
        segment = segment_before(from);
    }
    followed(want, path).map_err(|e| StoreError::Backend(format!("{key} does not lead back to its root: {e}")))
}
