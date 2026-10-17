//! A history as a store keeps it (issue 379): its chain, which is the truth
//! and only appended to, and beside it the items as they stand - every
//! message with every change since folded into it -, derived from the chain
//! and rebuilt from it at any time. The chain is written by the put of the
//! state that holds it (`StateStore`); the `HistoryStore` is the read side,
//! for whoever reads a history without holding it - always as a reader, who
//! is given what of each entry they may see (`Recorded::as_seen_by`, issue
//! 417).

use crate::agent::AgentId;
use crate::chain::Node;
use crate::channel::ChannelId;
use crate::digest::Digest;
use crate::history::{HistoryEntry, HistoryPage, Reader, Recorded};
use crate::message::MessageId;
use crate::store::StoreResult;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{self, Debug, Display};

/// Whose history: a channel's, or an agent's in a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HistoryKey {
    Channel(ChannelId),
    Agent(AgentId, ChannelId),
}

impl HistoryKey {
    /// Whether it is a history of the channel `channel`: the channel's own,
    /// or that of an agent in it.
    pub fn in_channel(&self, channel: ChannelId) -> bool {
        match self {
            HistoryKey::Channel(of) | HistoryKey::Agent(_, of) => *of == channel,
        }
    }

    /// Whether it is a history of the agent `agent`, in whichever channel.
    pub fn of_agent(&self, agent: AgentId) -> bool {
        matches!(self, HistoryKey::Agent(of, _) if *of == agent)
    }
}

impl Display for HistoryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HistoryKey::Channel(channel) => write!(f, "the history of channel {channel}"),
            HistoryKey::Agent(agent, channel) => write!(f, "the history of agent {agent} in channel {channel}"),
        }
    }
}

/// Which items a page holds (`HistoryStore::page`): a viewer reading a
/// channel entry by entry keeps what was released to it, a search what it
/// looks for.
pub type Keep<'a> = dyn Fn(&HistoryEntry) -> bool + Send + Sync + 'a;

/// Keeps every item.
pub fn every(_: &HistoryEntry) -> bool {
    true
}

/// The read side of the histories a store keeps (issue 379). A history is
/// written with the state that holds it (`StateStore::put_channel_state`,
/// `put_agent_channel_state`); what is read here is as fresh as what was
/// put. Positions are those of the chain (`Node::seq`); a history no store
/// holds reads as empty.
#[async_trait]
pub trait HistoryStore: Debug + Send + Sync {
    /// The last `limit` items - at least one - before the position `before`,
    /// the last ones without it, none before the position `from`, of those
    /// `reader` may see as they may see them and `keep` keeps: a page of the
    /// history as it stands (issue 218), oldest first, with `earlier` set if
    /// an older item from `from` on is kept, and the digest of the history
    /// as it was read. Read backwards in the cost of the page and of what it
    /// passes over.
    async fn page(&self, key: HistoryKey, reader: &Reader, from: u64, before: Option<u64>, limit: usize, keep: &Keep<'_>) -> StoreResult<HistoryPage>;

    /// The item that said the message `id` - `HistoryEntry::Said`, its
    /// message as it stands, with what it was produced from - and its
    /// position, as `reader` may see it; `None` if it was never said in the
    /// history, or not to them.
    async fn said(&self, key: HistoryKey, reader: &Reader, id: MessageId) -> StoreResult<Option<(u64, HistoryEntry)>>;

    /// The entries of the chain from the position `from` on that `reader`
    /// may see, as they may see them, in the order they were added: what was
    /// added since an entry before `from` was. Each keeps its node's
    /// position and digest, the chain's.
    async fn entries(&self, key: HistoryKey, reader: &Reader, from: u64) -> StoreResult<Vec<Node<HistoryEntry>>>;

    /// Derives the items anew from the chain: what repairs them, and what
    /// shows that they are derived.
    async fn rebuild(&self, key: HistoryKey) -> StoreResult<()>;
}

/// Items of a history as they stand, each at the position of the entry that
/// added it, in the order of their positions: all of a history's items, or
/// a run of them, as a store keeps them (issue 379). Derived from the chain
/// by `take`, by the one rule of folding (`HistoryEntry::fold_into`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(from = "Vec<(u64, Recorded)>", into = "Vec<(u64, Recorded)>")]
pub struct StandingItems {
    items: Vec<(u64, Recorded)>,
    /// Where the item of each message is in `items`.
    by_message: HashMap<MessageId, usize>,
}

impl StandingItems {
    /// The items of `nodes` - a chain from its start, in the order its
    /// entries were added - as they stand.
    pub fn of<'a>(nodes: impl IntoIterator<Item = &'a Node<Recorded>>) -> Self {
        let mut items = Self::default();
        for node in nodes {
            items.take(node);
        }
        items
    }

    /// Takes `node`, the next of the chain, in: an item after the last, a
    /// change into the item it changes - a fitting into every item.
    pub fn take(&mut self, node: &Node<Recorded>) {
        if node.entry.entry.is_item() {
            self.push(node);
        } else {
            self.amend(&node.entry.entry);
        }
    }

    /// Adds `node`, an item, after the last - once: an item at a position
    /// it holds already, or before it, is not added again.
    pub fn push(&mut self, node: &Node<Recorded>) {
        if self.items.last().is_some_and(|(last, _)| *last >= node.seq) {
            return;
        }
        if let Some(message) = node.entry.entry.said() {
            self.by_message.insert(message.id, self.items.len());
        }
        self.items.push((node.seq, node.entry.clone()));
    }

    /// Applies `change` to the item it changes, if it holds it - a fitting
    /// to every item -, and says whether it held any.
    pub fn amend(&mut self, change: &HistoryEntry) -> bool {
        if matches!(change, HistoryEntry::Fitted { .. }) {
            for (_, item) in &mut self.items {
                if let HistoryEntry::Said { message, .. } = &mut item.entry {
                    change.fold_into(message);
                }
            }
            return !self.items.is_empty();
        }
        let Some(&at) = change.changed().and_then(|id| self.by_message.get(&id)) else { return false };
        if let HistoryEntry::Said { message, .. } = &mut self.items[at].1.entry {
            change.fold_into(message);
        }
        true
    }

    /// The item that said the message `id`, as it stands, with its
    /// position, if it holds it.
    pub fn said(&self, id: MessageId) -> Option<&(u64, Recorded)> {
        self.by_message.get(&id).map(|&at| &self.items[at])
    }

    /// The items before the position `before`, newest first.
    pub fn newest_before(&self, before: u64) -> impl Iterator<Item = &(u64, Recorded)> {
        let end = self.items.partition_point(|(seq, _)| *seq < before);
        self.items[..end].iter().rev()
    }

    /// The position after the last item, `0` if it holds none.
    pub fn end(&self) -> u64 {
        self.items.last().map_or(0, |(seq, _)| seq + 1)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl From<Vec<(u64, Recorded)>> for StandingItems {
    fn from(items: Vec<(u64, Recorded)>) -> Self {
        let by_message = items.iter().enumerate()
            .filter_map(|(at, (_, item))| item.entry.said().map(|message| (message.id, at)))
            .collect();
        Self { items, by_message }
    }
}

impl From<StandingItems> for Vec<(u64, Recorded)> {
    fn from(items: StandingItems) -> Self {
        items.items
    }
}

/// The item at the position `seq` as `reader` may see it, if `keep` keeps
/// that: what a store returns of an item it read.
pub(crate) fn seen(reader: &Reader, keep: &Keep<'_>, (seq, item): &(u64, Recorded)) -> Option<(u64, HistoryEntry)> {
    item.as_seen_by(reader).filter(|entry| keep(entry)).map(|entry| (*seq, entry))
}

/// The entries of `nodes` that `reader` may see, as they may see them, each
/// in its node: what a store returns of the chain it read.
pub(crate) fn seen_nodes<'a>(reader: &Reader, nodes: impl IntoIterator<Item = &'a Node<Recorded>>) -> Vec<Node<HistoryEntry>> {
    nodes.into_iter().filter_map(|node| node.entry.as_seen_by(reader).map(|entry| node.with_entry(entry))).collect()
}

/// The page that `newest` - items newest first, as `reader` may see them and
/// `keep` keeps (`seen`), as a store reads them backwards from a position -
/// gives: the first `limit` of them - at least one - from the position
/// `from` on, oldest first, and `earlier` if one more is there; read from
/// the history `digest` names.
pub(crate) fn page_of(newest: impl IntoIterator<Item = (u64, HistoryEntry)>, from: u64, limit: usize, digest: Option<Digest>) -> HistoryPage {
    let limit = limit.max(1);
    let mut read: Vec<(u64, HistoryEntry)> = newest.into_iter().take_while(|(seq, _)| *seq >= from).take(limit + 1).collect();
    let earlier = (read.len() > limit).then(|| {
        read.truncate(limit);
        read.last().map(|(seq, _)| *seq)
    }).flatten();
    read.reverse();
    HistoryPage { items: read.into_iter().map(|(_, item)| item).collect(), earlier, digest }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::testing::items_of;
    use crate::confidentiality::Classification;
    use crate::actor::ActorId;
    use crate::history::{Audience, History};
    use crate::message::Message;
    use crate::message::tests::{id, make_message};
    use chrono::Utc;

    fn label(item: &HistoryEntry) -> String {
        match item.said() {
            Some(message) if message.revoked.is_some() => format!("{} revoked", message.id.0.as_u128()),
            Some(message) => format!("{} {}", message.id.0.as_u128(), message.content.clone().unwrap_or_default()),
            None => "other".to_string(),
        }
    }

    #[test]
    fn items_derived_entry_by_entry_are_the_history_folded() {
        let mut history = History::default();
        history.push(make_message(id(1)), None);
        history.push(make_message(id(2)), None);
        history.modify(Message { content: Some("changed".to_string()), ..make_message(id(1)) });
        history.push(make_message(id(3)), None);
        assert!(history.revoke(id(2), Utc::now()));
        history.fit_into(&Classification::CHANNEL_DEFAULT);
        assert!(history.reclassify(id(3), Classification::CHANNEL_DEFAULT));

        let mut items = StandingItems::default();
        for node in history.all_entries() {
            items.take(node);
        }
        let derived: Vec<String> = items.newest_before(u64::MAX).map(|(_, item)| label(&item.entry)).collect::<Vec<_>>().into_iter().rev().collect();
        assert_eq!(derived, items_of(&history).iter().map(label).collect::<Vec<_>>());
        assert_eq!(items.said(id(1)).map(|(seq, item)| (*seq, item.entry.said().and_then(|m| m.content.clone()))), Some((0, Some("changed".to_string()))));
        assert!(items.said(id(9)).is_none());

        let again = StandingItems::of(history.all_entries());
        assert_eq!(serde_json::to_value(&again).unwrap(), serde_json::to_value(&items).unwrap(), "the same, derived at once");
        let read: StandingItems = serde_json::from_value(serde_json::to_value(&items).unwrap()).unwrap();
        assert_eq!(read.said(id(3)).map(|(seq, _)| *seq), Some(3), "the index is made anew when it is read");
        let node = history.at(0).unwrap().clone();
        items.push(&node);
        assert_eq!(items.newest_before(u64::MAX).count(), 3, "an item is added once");
    }

    #[test]
    fn a_page_is_cut_from_the_newest_items() {
        let newest = |n: u64| (0..n).rev().map(|seq| (seq, HistoryEntry::Said { message: make_message(id(seq as u128 + 1)), context: None }));
        let page = page_of(newest(5), 0, 2, None);
        assert_eq!((page.items.iter().map(label).collect::<Vec<_>>(), page.earlier), (vec![label(&newest(5).nth(1).unwrap().1), label(&newest(5).next().unwrap().1)], Some(3)));
        let page = page_of(newest(5), 3, 5, None);
        assert_eq!((page.items.len(), page.earlier), (2, None), "none before where the view begins");
        assert_eq!(page_of(newest(0), 0, 5, None).items.len(), 0);
        assert_eq!(page_of(newest(3), 0, 0, None).items.len(), 1, "at least one");
        let odd = |item: &HistoryEntry| item.said().is_some_and(|m| m.id.0.as_u128() % 2 == 1);
        let kept = newest(5).map(|(seq, entry)| (seq, Recorded::new(entry, ActorId::None, Audience::Everybody)));
        let page = page_of(kept.filter_map(|item| seen(&Reader::Channel, &odd, &item)), 0, 2, None);
        assert_eq!((page.items.iter().map(label).collect::<Vec<_>>(), page.earlier), (vec![label(&newest(5).nth(2).unwrap().1), label(&newest(5).next().unwrap().1)], Some(2)), "a page of those kept, full, and an older one kept");
    }
}
