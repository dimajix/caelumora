//! The histories of the states, as the `JsonFileStore` keeps them (issue
//! 379): per history a directory beside its state's snapshot, `<key>.history`,
//! with the chain in segments of `SEGMENT` entries by position
//! (`chain-<from>.json`), the items as they stand in segments of the same
//! positions (`items-<from>.json`), and `head.json`, which says where the
//! chain ends and up to where the items were derived. A put writes the
//! chain's segments, then the head, then the items' segments, then the head
//! again: entries a crash left past the head's end are not there, and items
//! behind the chain are derived anew from it when the history is next
//! loaded or appended to. Slow where it has to be - finding a message scans
//! the items backwards, a fitting rewrites every segment -, which a
//! database's indexes are for.

use super::{delete_if_exists, read_json_opt, to_backend_error, write_json_atomic, JsonFileStore};
use crate::chain::{Chain, Node, Stored as StoredChain};
use crate::digest::Digest;
use crate::history::{History, HistoryEntry, HistoryPage};
use crate::message::MessageId;
use crate::store::history::{page_of, HistoryKey, HistoryStore, Keep, StandingItems};
use crate::store::{StoreError, StoreResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// How many entries of a chain, and how many positions of its items, one
/// file holds.
const SEGMENT: u64 = 256;

/// Where a history stands: the chain's head, the position after its last
/// entry, and the position up to which the items are derived.
#[derive(Debug, Default, Serialize, Deserialize)]
struct HistoryHead {
    head: Option<Digest>,
    end: u64,
    items_end: u64,
}

/// The position of the first entry of the segment that holds `seq`.
fn segment_of(seq: u64) -> u64 {
    seq - seq % SEGMENT
}

fn chain_path(dir: &Path, from: u64) -> PathBuf {
    dir.join(format!("chain-{from}.json"))
}

fn items_path(dir: &Path, from: u64) -> PathBuf {
    dir.join(format!("items-{from}.json"))
}

fn head_path(dir: &Path) -> PathBuf {
    dir.join("head.json")
}

async fn read_head(dir: &Path) -> StoreResult<HistoryHead> {
    Ok(read_json_opt(&head_path(dir)).await?.unwrap_or_default())
}

/// The entries of the segment from `from`, as written - past the head's end
/// too, if a crash left some.
async fn read_segment(dir: &Path, from: u64) -> StoreResult<Vec<Node<HistoryEntry>>> {
    Ok(read_json_opt(&chain_path(dir, from)).await?.unwrap_or_default())
}

async fn read_items(dir: &Path, from: u64) -> StoreResult<StandingItems> {
    Ok(read_json_opt(&items_path(dir, from)).await?.unwrap_or_default())
}

/// The entries from the position `from` up to `end`, in order.
async fn read_chain(dir: &Path, from: u64, end: u64) -> StoreResult<Vec<Node<HistoryEntry>>> {
    let mut nodes = Vec::new();
    let mut segment = segment_of(from);
    while segment < end {
        nodes.extend(read_segment(dir, segment).await?.into_iter().filter(|node| node.seq >= from && node.seq < end));
        segment += SEGMENT;
    }
    Ok(nodes)
}

/// Item segments read while entries are taken in, written once at the end.
struct ItemSegments<'a> {
    dir: &'a Path,
    read: BTreeMap<u64, StandingItems>,
    changed: HashSet<u64>,
}

impl<'a> ItemSegments<'a> {
    fn new(dir: &'a Path) -> Self {
        Self { dir, read: BTreeMap::new(), changed: HashSet::new() }
    }

    async fn segment(&mut self, from: u64) -> StoreResult<&mut StandingItems> {
        if !self.read.contains_key(&from) {
            let items = read_items(self.dir, from).await?;
            self.read.insert(from, items);
        }
        self.read.get_mut(&from).ok_or_else(|| StoreError::Backend(format!("the items from position {from} were not read")))
    }

    /// Takes `node`, the next entry of a chain that ends at `end`, in: an
    /// item into its segment, a change into the segment that holds the item
    /// it changes - found from the end backwards -, a fitting into every
    /// segment.
    async fn take(&mut self, node: &Node<HistoryEntry>, end: u64) -> StoreResult<()> {
        if node.entry.is_item() {
            let from = segment_of(node.seq);
            self.segment(from).await?.push(node);
            self.changed.insert(from);
            return Ok(());
        }
        let fitting = matches!(node.entry, HistoryEntry::Fitted { .. });
        let mut from = segment_of(end.saturating_sub(1));
        loop {
            if self.segment(from).await?.amend(&node.entry) {
                self.changed.insert(from);
                if !fitting {
                    return Ok(());
                }
            }
            if from == 0 {
                return Ok(());
            }
            from -= SEGMENT;
        }
    }

    async fn write(self) -> StoreResult<()> {
        for (from, items) in &self.read {
            if self.changed.contains(from) {
                write_json_atomic(&items_path(self.dir, *from), items).await?;
            }
        }
        Ok(())
    }
}

/// Takes the entries of the chain from the head's `items_end` up to its
/// `end` into the items, and records that it did: after a put, what it
/// added; after a crash, what was not derived yet. Taking an entry in twice
/// changes nothing.
async fn derive_items(dir: &Path, head: &mut HistoryHead, added: &[Node<HistoryEntry>]) -> StoreResult<()> {
    let mut segments = ItemSegments::new(dir);
    for node in added.iter().filter(|node| node.seq >= head.items_end && node.seq < head.end) {
        segments.take(node, head.end).await?;
    }
    segments.write().await?;
    head.items_end = head.end;
    write_json_atomic(&head_path(dir), head).await
}

/// The head of the history in `dir`, its items derived up to its end: a
/// crash between the chain and the items is repaired here.
async fn derived_head(dir: &Path, key: HistoryKey) -> StoreResult<HistoryHead> {
    let mut head = read_head(dir).await?;
    if head.items_end < head.end {
        warn!("The items of {} were derived up to position {} of {}; deriving the rest", key, head.items_end, head.end);
        let missing = read_chain(dir, head.items_end, head.end).await?;
        derive_items(dir, &mut head, &missing).await?;
    }
    Ok(head)
}

impl JsonFileStore {
    /// The directory of the history `key` names, beside its state's
    /// snapshot.
    pub(super) fn history_dir(&self, key: HistoryKey) -> PathBuf {
        let state = match key {
            HistoryKey::Channel(channel) => self.channel_state_path(channel),
            HistoryKey::Agent(agent, channel) => self.agent_channel_state_path(agent, channel),
        };
        state.with_extension("history")
    }

    /// Appends what `history`, the history `key` names as a state holds it,
    /// added since it was last stored, derives the items of it, and marks it
    /// stored.
    pub(super) async fn append_history(&self, key: HistoryKey, history: &mut History) -> StoreResult<()> {
        let Some(changes) = history.changes() else { return Ok(()) };
        let dir = self.history_dir(key);
        tokio::fs::create_dir_all(&dir).await.map_err(to_backend_error)?;
        let mut head = derived_head(&dir, key).await?;
        let mut by_segment: BTreeMap<u64, Vec<Node<HistoryEntry>>> = BTreeMap::new();
        for node in changes.entries {
            by_segment.entry(segment_of(node.seq)).or_default().push(node);
        }
        let mut end = head.end;
        let mut added = Vec::new();
        for (from, nodes) in by_segment {
            let mut stored = read_segment(&dir, from).await?;
            stored.retain(|node| node.seq < head.end);
            for node in nodes {
                let at = usize::try_from(node.seq - from).map_err(to_backend_error)?;
                if at < stored.len() {
                    // Changed in its place: the branch last taken from it.
                    stored[at] = node;
                } else if node.seq == end {
                    end += 1;
                    added.push(node.clone());
                    stored.push(node);
                } else {
                    return Err(StoreError::Backend(format!("what {key} added does not fit: an entry at position {} after the last at {}", node.seq, end.saturating_sub(1))));
                }
            }
            debug!("Writing the entries of {} from position {}", key, from);
            write_json_atomic(&chain_path(&dir, from), &stored).await?;
        }
        head.head = changes.head;
        head.end = end;
        write_json_atomic(&head_path(&dir), &head).await?;
        derive_items(&dir, &mut head, &added).await?;
        history.mark_stored();
        Ok(())
    }

    /// The history `key` names from the position `from` on, as a holder
    /// keeps it; empty if none is kept.
    pub(super) async fn load_history(&self, key: HistoryKey, from: u64) -> StoreResult<History> {
        let dir = self.history_dir(key);
        let head = derived_head(&dir, key).await?;
        let entries = read_chain(&dir, from, head.end).await?;
        Chain::suffix(from, StoredChain { entries, head: head.head })
            .map_err(|e| StoreError::Backend(format!("{key} cannot be read from position {from}: {e}")))
    }
}

#[async_trait]
impl HistoryStore for JsonFileStore {
    async fn page(&self, key: HistoryKey, from: u64, before: Option<u64>, limit: usize, keep: &Keep<'_>) -> StoreResult<HistoryPage> {
        let dir = self.history_dir(key);
        let head = read_head(&dir).await?;
        let before = before.map_or(head.items_end, |before| before.min(head.items_end));
        let mut newest: Vec<(u64, HistoryEntry)> = Vec::new();
        if before == 0 {
            return Ok(HistoryPage::default());
        }
        let mut segment = segment_of(before - 1);
        loop {
            newest.extend(read_items(&dir, segment).await?.newest_before(before).filter(|(seq, item)| *seq >= from && keep(item)).cloned());
            if newest.len() > limit || segment == 0 || segment <= from {
                break;
            }
            segment -= SEGMENT;
        }
        Ok(page_of(newest, from, limit, keep, head.head))
    }

    async fn said(&self, key: HistoryKey, id: MessageId) -> StoreResult<Option<(u64, HistoryEntry)>> {
        let dir = self.history_dir(key);
        let head = read_head(&dir).await?;
        if head.items_end == 0 {
            return Ok(None);
        }
        let mut segment = segment_of(head.items_end - 1);
        loop {
            if let Some(found) = read_items(&dir, segment).await?.said(id) {
                return Ok(Some(found));
            }
            if segment == 0 {
                return Ok(None);
            }
            segment -= SEGMENT;
        }
    }

    async fn entries(&self, key: HistoryKey, from: u64) -> StoreResult<Vec<Node<HistoryEntry>>> {
        let dir = self.history_dir(key);
        let head = read_head(&dir).await?;
        read_chain(&dir, from, head.end).await
    }

    async fn rebuild(&self, key: HistoryKey) -> StoreResult<()> {
        let dir = self.history_dir(key);
        let mut head = read_head(&dir).await?;
        let mut segment = 0;
        while segment < head.end.max(head.items_end) {
            delete_if_exists(&items_path(&dir, segment)).await?;
            segment += SEGMENT;
        }
        head.items_end = 0;
        let chain = read_chain(&dir, 0, head.end).await?;
        derive_items(&dir, &mut head, &chain).await
    }
}
