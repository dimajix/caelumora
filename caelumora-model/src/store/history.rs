//! A history as a store keeps it (issue 379): its chain, which is the truth
//! and only appended to, and beside it the items as they stand - every
//! message with every change since folded into it - and who was a member
//! when (`Memberships`), both derived from the chain and rebuilt from it at
//! any time. The chain is written by the put of the state that holds it
//! (`StateStore`); the `HistoryStore` is the read side, for whoever reads a
//! history without holding it - always as a reader, who is given what of
//! each entry they may see (`Recorded::as_seen_by`, issue 417): a predicate
//! over what the store keeps of an entry and the reader's ranges, which a
//! database answers as a `WHERE`.

use crate::actor::ActorId;
use crate::chain::{Node, Position};
use crate::channel::ChannelId;
use crate::digest::Digest;
use crate::history::{HistoryEntry, HistoryPage, Reader, Recorded, EVERY_POSITION};
use crate::message::MessageId;
use crate::store::StoreResult;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Debug;
use std::ops::Range;

/// Which items a page holds (`HistoryStore::page`): a viewer reading a
/// channel entry by entry keeps what was released to it, a search what it
/// looks for.
pub type Keep<'a> = dyn Fn(&HistoryEntry) -> bool + Send + Sync + 'a;

/// Keeps every item.
pub fn every(_: &HistoryEntry) -> bool {
    true
}

/// The read side of the histories a store keeps (issue 379), one per channel
/// (issue 417). A history is written with the state that holds it
/// (`StateStore::put_channel_state`); what is read here is as fresh as what
/// was put. Positions are those of the chain (`Node::seq`); a history no store
/// holds reads as empty.
#[async_trait]
pub trait HistoryStore: Debug + Send + Sync {
    /// The last `limit` items - at least one - before the position `before`,
    /// the last ones without it, none before the position `from`, of those
    /// `reader` may see as they may see them and `keep` keeps: a page of the
    /// history as it stands (issue 218), oldest first, with `earlier` set if
    /// an older item from `from` on is kept, and the digest of the history
    /// as it was read. The page counts what its reader sees; where their
    /// view begins is the store's to know (`memberships`), `from` only
    /// bounds a query - what is new since a reader last read. Read backwards
    /// in the cost of the page and of what it passes over.
    async fn page(&self, channel: ChannelId, reader: &Reader, from: Position, before: Option<Position>, limit: usize, keep: &Keep<'_>) -> StoreResult<HistoryPage>;

    /// The item that said the message `id` - `HistoryEntry::Said`, its
    /// message as it stands, with what it was produced from - and its
    /// position, as `reader` may see it; `None` if it was never said in the
    /// history, or not to them.
    async fn said(&self, channel: ChannelId, reader: &Reader, id: MessageId) -> StoreResult<Option<(Position, HistoryEntry)>>;

    /// The entries of the chain from the position `from` on that `reader`
    /// may see, as they may see them, in the order they were added: what was
    /// added since an entry before `from` was. Each keeps its node's
    /// position and digest, the chain's.
    async fn entries(&self, channel: ChannelId, reader: &Reader, from: Position) -> StoreResult<Vec<Node<HistoryEntry>>>;

    /// The ranges of positions in which `actor` was given what went to
    /// every member of the channel (`Memberships`), oldest first: none if it
    /// never was one, the last open - to `Position::MAX` - while it is.
    async fn memberships(&self, channel: ChannelId, actor: ActorId) -> StoreResult<Vec<Range<Position>>>;

    /// Derives the items and the memberships anew from the chain: what
    /// repairs them, and what shows that they are derived.
    async fn rebuild(&self, channel: ChannelId) -> StoreResult<()>;
}

/// Who was a member of a channel when (issue 417): each member's ranges of
/// positions in which it was given what went to every member, oldest first,
/// each from the `from` of the `HistoryShared` before its
/// `ParticipantJoined`, or else from its join, up to the `until` of its
/// `ParticipantLeft`.
/// Derived from the chain beside the items, by `take`, and rebuilt with them;
/// a database keeps it as a table of `(member, from, until)`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Memberships {
    ranges: Vec<(ActorId, Range<Position>)>,
    /// What the `HistoryShared` before a join shared, from where.
    shared: Vec<(ActorId, Position)>,
    /// The position after the last entry taken in.
    end: Position,
}

impl Memberships {
    /// The memberships `nodes` - a chain from its start, in the order its
    /// entries were added - record.
    pub fn of<'a>(nodes: impl IntoIterator<Item = &'a Node<Recorded>>) -> Self {
        let mut memberships = Self::default();
        for node in nodes {
            memberships.take(node);
        }
        memberships
    }

    /// Takes `node`, the next of the chain, in - once: a node before the end
    /// of what was taken in is not taken in again. A share is kept for the
    /// join that follows it, a join opens a range, a leave closes the open
    /// one of its participant.
    pub fn take(&mut self, node: &Node<Recorded>) {
        if node.seq < self.end {
            return;
        }
        self.end = node.seq.next();
        match &node.entry.entry {
            HistoryEntry::HistoryShared { participant, from, .. } => {
                self.shared.retain(|(member, _)| member != participant);
                self.shared.push((*participant, *from));
            }
            HistoryEntry::ParticipantJoined { participant, .. } => {
                let from = self.shared.iter().position(|(member, _)| member == participant)
                    .map_or(node.seq, |at| self.shared.remove(at).1);
                self.ranges.push((*participant, from..Position::MAX));
            }
            HistoryEntry::ParticipantLeft { participant, until, .. } => {
                if let Some((_, range)) = self.ranges.iter_mut().rev().find(|(member, range)| member == participant && range.end == Position::MAX) {
                    range.end = (*until).max(range.start);
                }
            }
            _ => {}
        }
    }

    /// The ranges of `member`, oldest first.
    pub fn of_member(&self, member: ActorId) -> Vec<Range<Position>> {
        self.ranges.iter().filter(|(of, _)| *of == member).map(|(_, range)| range.clone()).collect()
    }

    /// The ranges `reader` reads by (`Recorded::seen_by`): a participant's
    /// own; a newcomer's all of the history, of which it reads what it is
    /// given; none for anyone else.
    pub fn of_reader(&self, reader: &Reader) -> Vec<Range<Position>> {
        match reader {
            Reader::Participant(member) => self.of_member(*member),
            Reader::Newcomer(_) => vec![EVERY_POSITION],
            Reader::Outsider | Reader::Channel => Vec::new(),
        }
    }
}

/// Items of a history as they stand, each at the position of the entry that
/// added it, in the order of their positions: all of a history's items, or
/// a run of them, as a store keeps them (issue 379). Derived from the chain
/// by `take`, by the one rule of folding (`HistoryEntry::fold_into`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(from = "Vec<(Position, Recorded)>", into = "Vec<(Position, Recorded)>")]
pub struct StandingItems {
    items: Vec<(Position, Recorded)>,
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
    pub fn said(&self, id: MessageId) -> Option<&(Position, Recorded)> {
        self.by_message.get(&id).map(|&at| &self.items[at])
    }

    /// The items before the position `before`, newest first.
    pub fn newest_before(&self, before: Position) -> impl Iterator<Item = &(Position, Recorded)> {
        let end = self.items.partition_point(|(seq, _)| *seq < before);
        self.items[..end].iter().rev()
    }

    /// The position after the last item, `0` if it holds none.
    pub fn end(&self) -> Position {
        self.items.last().map_or(Position::START, |(seq, _)| seq.next())
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl From<Vec<(Position, Recorded)>> for StandingItems {
    fn from(items: Vec<(Position, Recorded)>) -> Self {
        let by_message = items.iter().enumerate()
            .filter_map(|(at, (_, item))| item.entry.said().map(|message| (message.id, at)))
            .collect();
        Self { items, by_message }
    }
}

impl From<StandingItems> for Vec<(Position, Recorded)> {
    fn from(items: StandingItems) -> Self {
        items.items
    }
}

/// The item at the position `seq` as `reader`, with `ranges` as theirs
/// (`Memberships::of_reader`), may see it, if `keep` keeps that: what a
/// store returns of an item it read.
pub(crate) fn seen(reader: &Reader, ranges: &[Range<Position>], keep: &Keep<'_>, (seq, item): &(Position, Recorded)) -> Option<(Position, HistoryEntry)> {
    item.as_seen_by(*seq, reader, ranges).filter(|entry| keep(entry)).map(|entry| (*seq, entry))
}

/// The entries of `nodes` that `reader`, with `ranges` as theirs, may see,
/// as they may see them, each in its node: what a store returns of the
/// chain it read.
pub(crate) fn seen_nodes<'a>(reader: &Reader, ranges: &[Range<Position>], nodes: impl IntoIterator<Item = &'a Node<Recorded>>) -> Vec<Node<HistoryEntry>> {
    nodes.into_iter().filter_map(|node| node.entry.as_seen_by(node.seq, reader, ranges).map(|entry| node.with_entry(entry))).collect()
}

/// The page that `newest` - items newest first, as `reader` may see them and
/// `keep` keeps (`seen`), as a store reads them backwards from a position -
/// gives: the first `limit` of them - at least one - from the position
/// `from` on, oldest first, and `earlier` if one more is there; read from
/// the history `digest` names.
pub(crate) fn page_of(newest: impl IntoIterator<Item = (Position, HistoryEntry)>, from: Position, limit: usize, digest: Option<Digest>) -> HistoryPage {
    let limit = limit.max(1);
    let mut read: Vec<(Position, HistoryEntry)> = newest.into_iter().take_while(|(seq, _)| *seq >= from).take(limit + 1).collect();
    let earlier = (read.len() > limit).then(|| {
        read.truncate(limit);
        read.last().map(|(seq, _)| *seq)
    }).flatten();
    read.reverse();
    HistoryPage { items: read, earlier, digest }
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
        history.push(make_message(id(1)));
        history.push(make_message(id(2)));
        history.modify(Message { content: Some("changed".to_string()), ..make_message(id(1)) });
        history.push(make_message(id(3)));
        assert!(history.revoke(id(2), Utc::now()));
        history.fit_into(&Classification::CHANNEL_DEFAULT);
        assert!(history.reclassify(id(3), Classification::CHANNEL_DEFAULT));

        let mut items = StandingItems::default();
        for node in history.all_entries() {
            items.take(node);
        }
        let derived: Vec<String> = items.newest_before(Position::MAX).map(|(_, item)| label(&item.entry)).collect::<Vec<_>>().into_iter().rev().collect();
        assert_eq!(derived, items_of(&history).iter().map(label).collect::<Vec<_>>());
        assert_eq!(items.said(id(1)).map(|(seq, item)| (seq.get(), item.entry.said().and_then(|m| m.content.clone()))), Some((0, Some("changed".to_string()))));
        assert!(items.said(id(9)).is_none());

        let again = StandingItems::of(history.all_entries());
        assert_eq!(serde_json::to_value(&again).unwrap(), serde_json::to_value(&items).unwrap(), "the same, derived at once");
        let read: StandingItems = serde_json::from_value(serde_json::to_value(&items).unwrap()).unwrap();
        assert_eq!(read.said(id(3)).map(|(seq, _)| seq.get()), Some(3), "the index is made anew when it is read");
        let node = history.at(Position::new(0)).unwrap().clone();
        items.push(&node);
        assert_eq!(items.newest_before(Position::MAX).count(), 3, "an item is added once");
    }

    #[test]
    fn a_page_is_cut_from_the_newest_items() {
        let newest = |n: u64| (0..n).rev().map(|seq| (Position::new(seq), HistoryEntry::Said { message: make_message(id(seq as u128 + 1)) }));
        let page = page_of(newest(5), Position::START, 2, None);
        assert_eq!((page.entries().map(label).collect::<Vec<_>>(), page.earlier), (vec![label(&newest(5).nth(1).unwrap().1), label(&newest(5).next().unwrap().1)], Some(Position::new(3))));
        let page = page_of(newest(5), Position::new(3), 5, None);
        assert_eq!((page.items.len(), page.earlier), (2, None), "none before where the view begins");
        assert_eq!(page_of(newest(0), Position::START, 5, None).items.len(), 0);
        assert_eq!(page_of(newest(3), Position::START, 0, None).items.len(), 1, "at least one");
        let odd = |item: &HistoryEntry| item.said().is_some_and(|m| m.id.0.as_u128() % 2 == 1);
        let kept = newest(5).map(|(seq, entry)| (seq, Recorded::new(entry, ActorId::None, Audience::Everybody)));
        let page = page_of(kept.filter_map(|item| seen(&Reader::Channel, &[], &odd, &item)), Position::START, 2, None);
        assert_eq!((page.entries().map(label).collect::<Vec<_>>(), page.earlier), (vec![label(&newest(5).nth(2).unwrap().1), label(&newest(5).next().unwrap().1)], Some(Position::new(2))), "a page of those kept, full, and an older one kept");
    }

    /// Issue 417: who was a member when is read off the chain - from a
    /// share before the join, or else from the join, up to where the leave
    /// says delivery stopped -, and what went to every member is given to
    /// whom it was a member then; what went to one by name, or what one
    /// sent, regardless.
    #[test]
    fn what_went_to_every_member_is_given_to_who_was_one_then() {
        use crate::user::UserId;
        let (alice, bob) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        let joined = |participant| HistoryEntry::ParticipantJoined { participant, name: String::new(), by: participant, by_name: String::new(), at: Utc::now() };
        let said = |n: u128, by: ActorId| HistoryEntry::Said { message: Message { sender_id: by, ..make_message(id(n)) } };
        let mut history = History::default();
        for (entry, sender, audience) in [
            (joined(alice), alice, Audience::Everybody),                                             // 0
            (said(1, alice), alice, Audience::Everybody),                                            // 1
            (HistoryEntry::HistoryShared { participant: bob, from: Position::new(1), by: alice, by_name: String::new(), at: Utc::now() }, ActorId::System, Audience::Everybody), // 2
            (joined(bob), alice, Audience::Everybody),                                               // 3
            (said(2, bob), bob, Audience::Everybody),                                                // 4
            (said(3, alice), alice, Audience::Everybody),                                            // 5: hers, after delivery to her stopped
            (HistoryEntry::ParticipantLeft { participant: alice, name: String::new(), by: alice, by_name: String::new(), until: Position::new(5), at: Utc::now() }, alice, Audience::Everybody), // 6
            (said(4, bob), bob, Audience::Everybody),                                                // 7
            (said(5, bob), bob, Audience::Only(vec![alice])),                                        // 8
        ] {
            assert!(history.enter(Recorded::new(entry, sender, audience)));
        }
        let memberships = Memberships::of(history.all_entries());
        let starts_and_ends = |member| memberships.of_member(member).into_iter().map(|range| (range.start.get(), range.end.get())).collect::<Vec<_>>();
        assert_eq!(starts_and_ends(alice), [(0, 5)], "from her join to where delivery stopped");
        assert_eq!(starts_and_ends(bob), [(1, u64::MAX)], "from what was shared with him, a member still");
        let seen = |reader: ActorId| -> Vec<u64> {
            let reading = Reader::Participant(reader);
            let ranges = memberships.of_reader(&reading);
            history.all_entries().filter(|node| node.entry.seen_by(node.seq, &reading, &ranges)).map(|node| node.seq.get()).collect()
        };
        assert_eq!(seen(alice), [0, 1, 3, 4, 5, 6, 8], "hers, what went to every member while she was one, and what went to her by name");
        assert_eq!(seen(bob), [1, 2, 3, 4, 5, 6, 7, 8], "what was shared with him, and since - the last his own");
        assert_eq!(Memberships::of(history.all_entries().chain(history.all_entries())).of_member(bob), memberships.of_member(bob), "an entry taken in twice changes nothing");
    }
}
