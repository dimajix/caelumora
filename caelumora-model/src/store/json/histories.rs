//! The histories of the states, as the `JsonFileStore` keeps them (issue
//! 379): per history a directory beside its state's snapshot, `<key>.history`,
//! with the chain as every chain is kept (`chains`), and laid over it the
//! items as they stand in segments of the same positions
//! (`items-<from>.json`) and `items-head.json`, which says up to where they
//! were derived. A put writes the chain, then the items' segments, then how
//! far they go: items behind the chain are derived anew from it when the
//! history is next loaded or appended to. Slow where it has to be - finding
//! a message scans the items backwards, a fitting rewrites every segment -,
//! which a database's indexes are for.

use super::chains::{self, read_chain, read_head, segment_of, SEGMENT};
use super::{delete_if_exists, read_json_opt, write_json_atomic, JsonFileStore};
use crate::chain::Node;
use crate::history::{History, HistoryEntry, HistoryPage};
use crate::message::MessageId;
use crate::store::history::{page_of, HistoryKey, HistoryStore, Keep, StandingItems};
use crate::store::{StoreError, StoreResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use tracing::warn;

/// Up to where the items of a history are derived: the position after the
/// last entry of the chain taken in.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ItemsHead {
    end: u64,
}

fn items_path(dir: &Path, from: u64) -> PathBuf {
    dir.join(format!("items-{from}.json"))
}

fn items_head_path(dir: &Path) -> PathBuf {
    dir.join("items-head.json")
}

async fn read_items_end(dir: &Path) -> StoreResult<u64> {
    Ok(read_json_opt::<ItemsHead>(&items_head_path(dir)).await?.unwrap_or_default().end)
}

async fn read_items(dir: &Path, from: u64) -> StoreResult<StandingItems> {
    Ok(read_json_opt(&items_path(dir, from)).await?.unwrap_or_default())
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

/// Takes `added`, the entries of the chain from `items_end` up to `end`,
/// into the items, and records that it did: after a put, what it added;
/// after a crash, what was not derived yet. Taking an entry in twice changes
/// nothing.
async fn derive_items(dir: &Path, items_end: u64, end: u64, added: &[Node<HistoryEntry>]) -> StoreResult<()> {
    let mut segments = ItemSegments::new(dir);
    for node in added.iter().filter(|node| node.seq >= items_end && node.seq < end) {
        segments.take(node, end).await?;
    }
    segments.write().await?;
    write_json_atomic(&items_head_path(dir), &ItemsHead { end }).await
}

/// The items of the history in `dir` derived up to the end of its chain: a
/// crash between the chain and the items is repaired here. Returns where
/// the chain ends.
async fn derive_missing(dir: &Path, key: HistoryKey) -> StoreResult<u64> {
    let end = read_head(dir).await?.end;
    let items_end = read_items_end(dir).await?;
    if items_end < end {
        warn!("The items of {} were derived up to position {} of {}; deriving the rest", key, items_end, end);
        let missing = read_chain(dir, items_end, end).await?;
        derive_items(dir, items_end, end, &missing).await?;
    }
    Ok(end)
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
        let items_end = derive_missing(&dir, key).await?;
        let (head, added) = chains::append(&dir, key, changes).await?;
        derive_items(&dir, items_end, head.end, &added).await?;
        history.mark_stored();
        Ok(())
    }

    /// The history `key` names from the position `from` on, as a holder
    /// keeps it; empty if none is kept.
    pub(super) async fn load_history(&self, key: HistoryKey, from: u64) -> StoreResult<History> {
        let dir = self.history_dir(key);
        derive_missing(&dir, key).await?;
        chains::load(&dir, key, from).await
    }
}

#[async_trait]
impl HistoryStore for JsonFileStore {
    async fn page(&self, key: HistoryKey, from: u64, before: Option<u64>, limit: usize, keep: &Keep<'_>) -> StoreResult<HistoryPage> {
        let dir = self.history_dir(key);
        let head = read_head(&dir).await?;
        let items_end = read_items_end(&dir).await?;
        let before = before.map_or(items_end, |before| before.min(items_end));
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
        let items_end = read_items_end(&dir).await?;
        if items_end == 0 {
            return Ok(None);
        }
        let mut segment = segment_of(items_end - 1);
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
        let end = read_head(&dir).await?.end;
        let mut segment = 0;
        while segment < end.max(read_items_end(&dir).await?) {
            delete_if_exists(&items_path(&dir, segment)).await?;
            segment += SEGMENT;
        }
        let chain = read_chain(&dir, 0, end).await?;
        derive_items(&dir, 0, end, &chain).await
    }
}
