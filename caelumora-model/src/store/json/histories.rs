//! The histories of the states, as the `JsonFileStore` keeps them (issue
//! 379): per history a directory beside its state's snapshot, `<key>.history`,
//! with the chain as every chain is kept (`chains`), and laid over it the
//! items as they stand in segments of the same positions
//! (`items-<from>.json`), who was a member when (`memberships.json`, issue
//! 417), and `items-head.json`, which says up to where both were derived. A
//! put writes the chain, then the items' segments and the memberships, then
//! how far they go: what is behind the chain is derived anew from it when
//! the history is next loaded or appended to. Slow where it has to be - finding
//! a message scans the items backwards, a fitting rewrites every segment -,
//! which a database's indexes are for.

use super::chains::{self, read_chain, read_head, segment_before, segment_of, SEGMENT};
use super::{delete_if_exists, read_json_opt, write_json_atomic, JsonFileStore};
use crate::actor::ActorId;
use crate::chain::{Node, Position};
use crate::channel::ChannelId;
use crate::history::{History, HistoryEntry, HistoryPage, Reader, Recorded};
use crate::message::MessageId;
use crate::store::history::{page_of, seen, seen_nodes, HistoryField, HistoryStore, Memberships, StandingItems};
use crate::store::query::Filter;
use crate::store::{StoreError, StoreResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use tracing::warn;

/// Up to where the items of a history are derived: the position after the
/// last entry of the chain taken in.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ItemsHead {
    end: Position,
}

fn items_path(dir: &Path, from: Position) -> PathBuf {
    dir.join(format!("items-{from}.json"))
}

fn items_head_path(dir: &Path) -> PathBuf {
    dir.join("items-head.json")
}

fn memberships_path(dir: &Path) -> PathBuf {
    dir.join("memberships.json")
}

async fn read_memberships(dir: &Path) -> StoreResult<Memberships> {
    Ok(read_json_opt(&memberships_path(dir)).await?.unwrap_or_default())
}

async fn read_items_end(dir: &Path) -> StoreResult<Position> {
    Ok(read_json_opt::<ItemsHead>(&items_head_path(dir)).await?.unwrap_or_default().end)
}

async fn read_items(dir: &Path, from: Position) -> StoreResult<StandingItems> {
    Ok(read_json_opt(&items_path(dir, from)).await?.unwrap_or_default())
}

/// Item segments read while entries are taken in, written once at the end.
struct ItemSegments<'a> {
    dir: &'a Path,
    read: BTreeMap<Position, StandingItems>,
    changed: HashSet<Position>,
}

impl<'a> ItemSegments<'a> {
    fn new(dir: &'a Path) -> Self {
        Self { dir, read: BTreeMap::new(), changed: HashSet::new() }
    }

    async fn segment(&mut self, from: Position) -> StoreResult<&mut StandingItems> {
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
    async fn take(&mut self, node: &Node<Recorded>, end: Position) -> StoreResult<()> {
        if node.entry.entry.is_item() {
            let from = segment_of(node.seq);
            self.segment(from).await?.push(node);
            self.changed.insert(from);
            return Ok(());
        }
        let fitting = matches!(node.entry.entry, HistoryEntry::Fitted { .. });
        let mut segment = end.previous().map(segment_of);
        while let Some(from) = segment {
            if self.segment(from).await?.amend(&node.entry.entry) {
                self.changed.insert(from);
                if !fitting {
                    return Ok(());
                }
            }
            segment = segment_before(from);
        }
        Ok(())
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
/// into the items and the memberships, and records that it did: after a
/// put, what it added; after a crash, what was not derived yet. Taking an
/// entry in twice changes nothing.
async fn derive_items(dir: &Path, items_end: Position, end: Position, added: &[Node<Recorded>]) -> StoreResult<()> {
    let mut segments = ItemSegments::new(dir);
    let mut memberships = read_memberships(dir).await?;
    for node in added.iter().filter(|node| node.seq >= items_end && node.seq < end) {
        segments.take(node, end).await?;
        memberships.take(node);
    }
    segments.write().await?;
    write_json_atomic(&memberships_path(dir), &memberships).await?;
    write_json_atomic(&items_head_path(dir), &ItemsHead { end }).await
}

/// The items of the history in `dir` derived up to the end of its chain: a
/// crash between the chain and the items is repaired here. Returns where
/// the chain ends.
async fn derive_missing(dir: &Path, channel: ChannelId) -> StoreResult<Position> {
    let end = read_head(dir).await?.end;
    let items_end = read_items_end(dir).await?;
    if items_end < end {
        warn!("The items of the history of channel {} were derived up to position {} of {}; deriving the rest", channel, items_end, end);
        let missing = read_chain(dir, items_end, end).await?;
        derive_items(dir, items_end, end, &missing).await?;
    }
    Ok(end)
}

/// The history of `channel`, as an error names it.
fn history_of(channel: ChannelId) -> String {
    format!("the history of channel {channel}")
}

impl JsonFileStore {
    /// The directory of the history of `channel`, beside its state's
    /// snapshot.
    pub(super) fn history_dir(&self, channel: ChannelId) -> PathBuf {
        self.channel_state_path(channel).with_extension("history")
    }

    /// Appends what `history`, that of `channel` as its state holds it,
    /// added since it was last stored, derives the items of it, and marks it
    /// stored.
    pub(super) async fn append_history(&self, channel: ChannelId, history: &mut History) -> StoreResult<()> {
        let Some(changes) = history.changes() else { return Ok(()) };
        let dir = self.history_dir(channel);
        let items_end = derive_missing(&dir, channel).await?;
        let (head, added) = chains::append(&dir, history_of(channel), changes).await?;
        derive_items(&dir, items_end, head.end, &added).await?;
        history.mark_stored();
        Ok(())
    }

    /// The history of `channel` from the position `from` on, as a holder
    /// keeps it; empty if none is kept.
    pub(super) async fn load_history(&self, channel: ChannelId, from: Position) -> StoreResult<History> {
        let dir = self.history_dir(channel);
        derive_missing(&dir, channel).await?;
        chains::load(&dir, history_of(channel), from).await
    }
}

#[async_trait]
impl HistoryStore for JsonFileStore {
    async fn page(&self, channel: ChannelId, reader: &Reader, from: Position, before: Option<Position>, limit: usize, filter: Option<&Filter<HistoryField>>) -> StoreResult<HistoryPage> {
        let dir = self.history_dir(channel);
        let head = read_head(&dir).await?;
        let items_end = read_items_end(&dir).await?;
        let before = before.map_or(items_end, |before| before.min(items_end));
        if before == Position::START {
            return Ok(HistoryPage::default());
        }
        let mut newest: Vec<(Position, HistoryEntry)> = Vec::new();
        let ranges = read_memberships(&dir).await?.of_reader(reader);
        let mut segment = before.previous().map(segment_of);
        while let Some(from_here) = segment {
            for item in read_items(&dir, from_here).await?.newest_before(before).take_while(|(seq, _)| *seq >= from) {
                newest.extend(seen(reader, &ranges, filter, item)?);
            }
            if newest.len() > limit || from_here <= from {
                break;
            }
            segment = segment_before(from_here);
        }
        page_of(newest.into_iter().map(|item| Ok(Some(item))), from, limit, head.head)
    }

    async fn said(&self, channel: ChannelId, reader: &Reader, id: MessageId) -> StoreResult<Option<(Position, HistoryEntry)>> {
        let dir = self.history_dir(channel);
        let mut segment = read_items_end(&dir).await?.previous().map(segment_of);
        while let Some(from) = segment {
            if let Some(found) = read_items(&dir, from).await?.said(id) {
                let ranges = read_memberships(&dir).await?.of_reader(reader);
                return seen(reader, &ranges, None, found);
            }
            segment = segment_before(from);
        }
        Ok(None)
    }

    async fn entries(&self, channel: ChannelId, reader: &Reader, range: Range<Position>) -> StoreResult<Vec<Node<HistoryEntry>>> {
        let dir = self.history_dir(channel);
        let head = read_head(&dir).await?;
        let chain: Vec<Node<Recorded>> = read_chain(&dir, range.start, head.end.min(range.end)).await?;
        let ranges = read_memberships(&dir).await?.of_reader(reader);
        Ok(seen_nodes(reader, &ranges, &chain))
    }

    async fn memberships(&self, channel: ChannelId, actor: ActorId) -> StoreResult<Vec<Range<Position>>> {
        let dir = self.history_dir(channel);
        derive_missing(&dir, channel).await?;
        Ok(read_memberships(&dir).await?.of_member(actor))
    }

    async fn rebuild(&self, channel: ChannelId) -> StoreResult<()> {
        let dir = self.history_dir(channel);
        let end = read_head(&dir).await?.end;
        let mut segment = Position::START;
        while segment < end.max(read_items_end(&dir).await?) {
            delete_if_exists(&items_path(&dir, segment)).await?;
            segment = segment.after(SEGMENT);
        }
        delete_if_exists(&memberships_path(&dir)).await?;
        let chain = read_chain(&dir, Position::START, end).await?;
        derive_items(&dir, Position::START, end, &chain).await
    }
}
