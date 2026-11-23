//! A chain a state holds, as `JsonStates` keeps it (issues 379, 380): per chain a directory beside
//! its state's snapshot, with the nodes in segments of `SEGMENT` entries by position
//! (`chain-<from>.json`) and `head.json`, which says where the chain ends and what its head is. A
//! put writes the segments, then the head: nodes a crash left past the head's end are not there.
//! What is laid over a chain - a history's items - keeps files of its own beside these.

use super::{read_json_opt, to_backend_error, write_json_atomic};
use caelumora_domain::chain::{Chain, Node, Position, Stored};
use caelumora_domain::digest::Digest;
use caelumora_domain::store::{StoreError, StoreResult};
use caelumora_store::kept_chain::{follow_back, followed, new_nodes};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::debug;

/// How many entries of a chain one file holds.
pub(super) const SEGMENT: u64 = 256;

/// Where a chain stands: its head, of the kind `H` its chain has - the one
/// digest of a `Chain`, the heads of the threads of a `Transcript` -, and
/// the position after its last entry.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct ChainHead<H = Option<Digest>> {
    pub(super) head: H,
    pub(super) end: Position,
}

/// Where a chain stands, of whatever kind its head is: the position after its
/// last entry.
#[derive(Debug, Default, Deserialize)]
struct ChainEnd {
    end: Position,
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
pub(super) async fn read_head<H: DeserializeOwned + Default>(
    dir: &Path,
) -> StoreResult<ChainHead<H>> {
    Ok(read_json_opt(&head_path(dir)).await?.unwrap_or_default())
}

/// The position after the last entry of the chain in `dir`, whatever kind
/// its head is; the start if none is kept there.
pub(super) async fn read_end(dir: &Path) -> StoreResult<Position> {
    Ok(read_json_opt::<ChainEnd>(&head_path(dir)).await?.unwrap_or_default().end)
}

/// The entries of the segment from `from`, as written - past the head's end
/// too, if a crash left some.
async fn read_segment<E: DeserializeOwned>(
    dir: &Path,
    from: Position,
) -> StoreResult<Vec<Node<E>>> {
    Ok(read_json_opt(&segment_path(dir, from)).await?.unwrap_or_default())
}

/// The entries from the position `from` up to `end`, in order.
pub(super) async fn read_chain<E: DeserializeOwned>(
    dir: &Path,
    from: Position,
    end: Position,
) -> StoreResult<Vec<Node<E>>> {
    let mut nodes = Vec::new();
    let mut segment = segment_of(from);
    while segment < end {
        nodes.extend(
            read_segment(dir, segment)
                .await?
                .into_iter()
                .filter(|node| node.seq >= from && node.seq < end),
        );
        segment = segment.after(SEGMENT);
    }
    Ok(nodes)
}

/// Writes `entries` - what the holder's chain added since it was stored -
/// and its `head` into the chain `key` names, kept in `dir`: the segments,
/// then the head. Returns where the chain stands now, and the nodes added.
///
/// # Errors
/// If they do not fit (`new_nodes`), or cannot be written.
pub(super) async fn append<E, H>(
    dir: &Path,
    key: impl Display,
    entries: Vec<Node<E>>,
    head: H,
) -> StoreResult<(ChainHead<H>, Vec<Node<E>>)>
where
    E: Serialize + DeserializeOwned + Clone,
    H: Serialize,
{
    fs::create_dir_all(dir).await.map_err(to_backend_error)?;
    let kept_end = read_end(dir).await?;
    let added = new_nodes(kept_end, entries)
        .map_err(|e| StoreError::Backend(format!("what {key} added does not fit: {e}")))?;
    let end = kept_end.after(added.len() as u64);
    let mut by_segment: BTreeMap<Position, Vec<Node<E>>> = BTreeMap::new();
    for node in &added {
        by_segment.entry(segment_of(node.seq)).or_default().push(node.clone());
    }
    for (from, nodes) in by_segment {
        let mut stored = read_segment(dir, from).await?;
        stored.retain(|node| node.seq < kept_end);
        stored.extend(nodes);
        debug!("Writing the entries of {} from position {}", key, from);
        write_json_atomic(&segment_path(dir, from), &stored).await?;
    }
    let head = ChainHead { head, end };
    write_json_atomic(&head_path(dir), &head).await?;
    Ok((head, added))
}

/// The chain `key` names, kept in `dir`, from the position `from` on, as a
/// holder keeps it; empty if none is kept.
///
/// # Errors
/// If it cannot be read, or what is read is no chain.
pub(super) async fn load<E: DeserializeOwned>(
    dir: &Path,
    key: impl Display,
    from: Position,
) -> StoreResult<Chain<E>> {
    let head: ChainHead = read_head(dir).await?;
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
pub(super) async fn path<E: DeserializeOwned + Clone>(
    dir: &Path,
    key: impl Display,
    digest: Digest,
) -> StoreResult<Option<Vec<Node<E>>>> {
    let end = read_end(dir).await?;
    follow_back_before(dir, key, end, Some(digest), Vec::new()).await
}

/// The path from the root up to the entry at the position `at` of the chain
/// `key` names, kept in `dir`, read backwards segment by segment from there:
/// a branch of a transcript, its head kept by its position.
///
/// # Errors
/// If it cannot be read, there is no entry at `at`, or the path does not
/// lead back to a root.
pub(super) async fn path_to<E: DeserializeOwned + Clone>(
    dir: &Path,
    key: impl Display,
    at: Position,
) -> StoreResult<Vec<Node<E>>> {
    let nodes: Vec<Node<E>> = read_segment(dir, segment_of(at)).await?;
    let Some(head) = nodes.iter().find(|node| node.seq == at) else {
        return Err(StoreError::Backend(format!("{key} holds no entry at position {at}")));
    };
    let (want, path) = (head.parent, vec![head.clone()]);
    let path = follow_back_before(dir, &key, at, want, path).await?;
    Ok(path.unwrap_or_default())
}

/// Follows the path that `path` - the entries followed back so far, newest
/// first - goes on with back from the entry `want` names, the newest such
/// before the position `before` of the chain `key` names, kept in `dir`,
/// reading backwards segment by segment (`follow_back`); `None` if there is
/// none.
async fn follow_back_before<E: DeserializeOwned + Clone>(
    dir: &Path,
    key: impl Display,
    before: Position,
    mut want: Option<Digest>,
    mut path: Vec<Node<E>>,
) -> StoreResult<Option<Vec<Node<E>>>> {
    let mut segment = before.previous().map(segment_of);
    while let Some(from) = segment.filter(|_| want.is_some()) {
        let nodes: Vec<Node<E>> = read_segment(dir, from).await?;
        follow_back(&mut want, &mut path, nodes.iter().rev().filter(|node| node.seq < before));
        segment = segment_before(from);
    }
    followed(want, path)
        .map_err(|e| StoreError::Backend(format!("{key} does not lead back to its root: {e}")))
}
