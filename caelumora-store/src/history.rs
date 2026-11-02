//! A history as a store keeps it (issue 379): its chain, which is the truth
//! and only appended to, and beside it the items as they stand - every
//! message with every change since folded into it - and who took part
//! when (`Attendance`), both derived from the chain and rebuilt from it at
//! any time. The chain is written by the put of the state that holds it
//! (`StateStore`); the `HistoryStore` is the read side, for whoever reads a
//! history without holding it - always as a reader, who is given what of
//! each entry they may see (`Recorded::as_seen_by`, issue 417): a predicate
//! over what the store keeps of an entry and the reader's ranges, which the
//! domain gives as data too (`Filter::seen_by`, issue 451), so that a
//! database answers it as a `WHERE`. What else a page keeps - the messages
//! of one sender, those heard, those of a classification the reader may
//! copy - is a condition as data as well (`Filter<HistoryField>`, issue
//! 421). Both are judged on an item as it is kept (`KeptItem`).

use async_trait::async_trait;
use caelumora_domain::actor::ActorId;
use caelumora_domain::chain::{Node, Position};
use caelumora_domain::channel::ChannelId;
use caelumora_domain::digest::Digest;
use caelumora_domain::history::{
    HistoryEntry, HistoryField, HistoryPage, KeptItem, Reader, Recorded,
};
use caelumora_domain::message::{Message, MessageId};
use caelumora_domain::query::{meeting, Filter};
use caelumora_domain::store::StoreResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Debug;
use std::ops::Range;

/// The read side of the histories a store keeps (issue 379), one per channel
/// (issue 417). A history is written with the state that holds it
/// (`StateStore::put_channel_state`); what is read here is as fresh as what
/// was put. Positions are those of the chain (`Node::seq`); a history no store
/// holds reads as empty.
#[async_trait]
pub trait HistoryStore: Debug + Send + Sync {
    /// The last `limit` items - at least one - before the position `before`,
    /// the last ones without it, none before the position `from`, of those
    /// `reader` may see as they may see them and that meet `filter`, judged
    /// on the item as it is recorded: a page of the
    /// history as it stands (issue 218), oldest first, with `earlier` set if
    /// an older item from `from` on is kept, and the digest of the history
    /// as it was read. The page counts what its reader sees; where their
    /// view begins is the store's to know (`attendance`), `from` only
    /// bounds a query - what is new since a reader last read. Read backwards
    /// in the cost of the page and of what it passes over.
    async fn page(
        &self,
        channel: ChannelId,
        reader: &Reader,
        from: Position,
        before: Option<Position>,
        limit: usize,
        filter: Option<&Filter<HistoryField>>,
    ) -> StoreResult<HistoryPage>;

    /// The item that said the message `id` - `HistoryEntry::Said`, its
    /// message as it stands, with what it was produced from - and its
    /// position, as `reader` may see it; `None` if it was never said in the
    /// history, or not to them.
    async fn said(
        &self,
        channel: ChannelId,
        reader: &Reader,
        id: MessageId,
    ) -> StoreResult<Option<(Position, HistoryEntry)>>;

    /// The entries of the chain at the positions `range` holds that `reader`
    /// may see, as they may see them, in the order they were added: what was
    /// added since an entry before the range was, up to its end. Each keeps
    /// its node's position and digest, the chain's, and the head of the
    /// envelope it came in (issue 223): what an envelope is delivered again
    /// from (`Recorded::envelope`).
    async fn entries(
        &self,
        channel: ChannelId,
        reader: &Reader,
        range: Range<Position>,
    ) -> StoreResult<Vec<Node<Recorded>>>;

    /// The answer of the turn whose end is at the position `end` (issue
    /// 223): its messages said that are not internal, as they were said,
    /// as `reader` may see them, oldest first - what its end carried as it
    /// was delivered (`ActorEvent::TurnFinished`), and what the history
    /// holds as the messages said before it (`answer_of`). Empty if nothing
    /// at `end` ends a turn.
    async fn answer(
        &self,
        channel: ChannelId,
        reader: &Reader,
        end: Position,
    ) -> StoreResult<Vec<Message>>;

    /// The ranges of positions in which `actor` was given what went to
    /// every member of the channel (`Attendance`), oldest first: none if it
    /// never was one, the last open - to `Position::MAX` - while it is.
    async fn attendance(
        &self,
        channel: ChannelId,
        actor: ActorId,
    ) -> StoreResult<Vec<Range<Position>>>;

    /// Derives the items and the attendance anew from the chain: what
    /// repairs them, and what shows that they are derived.
    async fn rebuild(&self, channel: ChannelId) -> StoreResult<()>;
}

/// Who took part in a channel when (issue 417): each participant's ranges
/// of positions in which it was given what went to every member, oldest
/// first, each from the `from` of the `HistoryShared` before its
/// `ParticipantJoined`, or else from its join, up to the `until` of its
/// `ParticipantLeft`.
/// Derived from the chain beside the items, by `take`, and rebuilt with them;
/// kept whole beside the chain - also by a database (issue 04), which would
/// otherwise rebuild in SQL what a share before a join gives.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Attendance {
    ranges: Vec<(ActorId, Range<Position>)>,
    /// What the `HistoryShared` before a join shared, from where.
    shared: Vec<(ActorId, Position)>,
    /// The position after the last entry taken in.
    end: Position,
}

impl Attendance {
    /// The attendance that `nodes` - a chain from its start, in the order its
    /// entries were added - record.
    pub fn of<'a>(nodes: impl IntoIterator<Item = &'a Node<Recorded>>) -> Self {
        let mut attendance = Self::default();
        for node in nodes {
            attendance.take(node);
        }
        attendance
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
                self.shared.retain(|(of, _)| of != participant);
                self.shared.push((*participant, *from));
            }
            HistoryEntry::ParticipantJoined { participant, .. } => {
                let from = self
                    .shared
                    .iter()
                    .position(|(of, _)| of == participant)
                    .map_or(node.seq, |at| self.shared.remove(at).1);
                self.ranges.push((*participant, from..Position::MAX));
            }
            HistoryEntry::ParticipantLeft { participant, until, .. } => {
                if let Some((_, range)) = self
                    .ranges
                    .iter_mut()
                    .rev()
                    .find(|(of, range)| of == participant && range.end == Position::MAX)
                {
                    range.end = (*until).max(range.start);
                }
            }
            _ => {}
        }
    }

    /// The ranges of `participant`, oldest first.
    pub fn of_participant(&self, participant: ActorId) -> Vec<Range<Position>> {
        self.ranges
            .iter()
            .filter(|(of, _)| *of == participant)
            .map(|(_, range)| range.clone())
            .collect()
    }

    /// The ranges `reader` reads by (`Recorded::seen_by`): a participant's
    /// own; none for anyone else.
    pub fn of_reader(&self, reader: &Reader) -> Vec<Range<Position>> {
        match reader {
            Reader::Participant(participant) => self.of_participant(*participant),
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
        let Some(&at) = change.changed().and_then(|id| self.by_message.get(&id)) else {
            return false;
        };
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
        let by_message = items
            .iter()
            .enumerate()
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
/// (`Attendance::of_reader`), may see it, if it meets `filter`: what a
/// store returns of an item it read.
///
/// # Errors
/// `StoreError::Validation` if the item cannot be judged by `filter`.
pub fn seen(
    reader: &Reader,
    ranges: &[Range<Position>],
    filter: Option<&Filter<HistoryField>>,
    (seq, item): &(Position, Recorded),
) -> StoreResult<Option<(Position, HistoryEntry)>> {
    if !item.seen_by(*seq, reader, ranges) {
        return Ok(None);
    }
    if let Some(filter) = filter {
        if meeting(KeptItem::of(*seq, item.clone()), Some(filter))?.is_none() {
            return Ok(None);
        }
    }
    Ok(item.as_seen_by(*seq, reader, ranges).map(|entry| (*seq, entry)))
}

/// The entries of `nodes` that `reader`, with `ranges` as theirs, may see,
/// as they may see them, each in its node with what else was recorded of
/// it: what a store returns of the chain it read.
pub fn seen_nodes<'a>(
    reader: &Reader,
    ranges: &[Range<Position>],
    nodes: impl IntoIterator<Item = &'a Node<Recorded>>,
) -> Vec<Node<Recorded>> {
    nodes
        .into_iter()
        .filter_map(|node| {
            let Recorded { sender, recipients, context, timestamp, .. } = &node.entry;
            let entry = node.entry.as_seen_by(node.seq, reader, ranges)?;
            Some(node.with_entry(Recorded {
                entry,
                sender: *sender,
                recipients: recipients.clone(),
                context: context.clone(),
                timestamp: *timestamp,
            }))
        })
        .collect()
}

/// The answer of the turn whose end is `ended` (`HistoryStore::answer`):
/// its messages said that are not internal, as `reader`, with `ranges` as
/// theirs, may see them, oldest first, gathered from `older` - the entries
/// before its end, newest first - back to the end of the turn before of
/// the same actor: an agent's turns do not overlap, so none of the turn's
/// messages lies before it. Empty if `ended` is no turn's end.
pub fn answer_of<'a>(
    ended: &HistoryEntry,
    older: impl IntoIterator<Item = &'a Node<Recorded>>,
    reader: &Reader,
    ranges: &[Range<Position>],
) -> Vec<Message> {
    let HistoryEntry::TurnFinished { turn_id, actor, .. } = ended else { return Vec::new() };
    let mut answer: Vec<Message> = older
        .into_iter()
        .take_while(|node| !node.entry.entry.ends_turn_of(*actor))
        .filter(|node| {
            node.entry
                .entry
                .said()
                .is_some_and(|message| message.turn_id == Some(*turn_id) && !message.internal)
        })
        .filter_map(|node| {
            node.entry.as_seen_by(node.seq, reader, ranges).and_then(|entry| entry.said().cloned())
        })
        .collect();
    answer.reverse();
    answer
}

/// The page that `newest` - items newest first, as `reader` may see them and
/// as they meet a filter (`seen`), as a store reads them backwards from a
/// position - gives: the first `limit` of them - at least one - from the
/// position `from` on, oldest first, and `earlier` if one more is there;
/// read from the history `digest` names. Reads no further than that.
///
/// # Errors
/// The first error among the items it reads.
pub fn page_of(
    newest: impl IntoIterator<Item = StoreResult<Option<(Position, HistoryEntry)>>>,
    from: Position,
    limit: usize,
    digest: Option<Digest>,
) -> StoreResult<HistoryPage> {
    let limit = limit.max(1);
    let mut read: Vec<(Position, HistoryEntry)> = Vec::new();
    for item in newest {
        if read.len() > limit {
            break;
        }
        let Some((seq, entry)) = item? else { continue };
        if seq < from {
            break;
        }
        read.push((seq, entry));
    }
    let earlier = (read.len() > limit)
        .then(|| {
            read.truncate(limit);
            read.last().map(|(seq, _)| *seq)
        })
        .flatten();
    read.reverse();
    Ok(HistoryPage { items: read, earlier, digest })
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::actor::RequestReason;
    use caelumora_domain::confidentiality::{
        Classification, Compartment, Compartments, Confidentiality, Reading,
    };
    use caelumora_domain::content::testing::make_content_ref;
    use caelumora_domain::history::testing::items_of;
    use caelumora_domain::history::{Audience, History};
    use caelumora_domain::message::testing::{id, make_message};
    use caelumora_domain::message::{Message, MessageRole};
    use caelumora_domain::user::UserId;
    use chrono::Utc;

    fn label(item: &HistoryEntry) -> String {
        match item.said() {
            Some(message) if message.revoked.is_some() => {
                format!("{} revoked", message.id.0.as_u128())
            }
            Some(message) => format!(
                "{} {}",
                message.id.0.as_u128(),
                message.content.clone().unwrap_or_default()
            ),
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
        let derived: Vec<String> = items
            .newest_before(Position::MAX)
            .map(|(_, item)| label(&item.entry))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        assert_eq!(derived, items_of(&history).iter().map(label).collect::<Vec<_>>());
        assert_eq!(
            items
                .said(id(1))
                .map(|(seq, item)| (seq.get(), item.entry.said().and_then(|m| m.content.clone()))),
            Some((0, Some("changed".to_string())))
        );
        assert!(items.said(id(9)).is_none());

        let again = StandingItems::of(history.all_entries());
        assert_eq!(
            serde_json::to_value(&again).unwrap(),
            serde_json::to_value(&items).unwrap(),
            "the same, derived at once"
        );
        let read: StandingItems =
            serde_json::from_value(serde_json::to_value(&items).unwrap()).unwrap();
        assert_eq!(
            read.said(id(3)).map(|(seq, _)| seq.get()),
            Some(3),
            "the index is made anew when it is read"
        );
        let node = history.at(Position::new(0)).unwrap().clone();
        items.push(&node);
        assert_eq!(items.newest_before(Position::MAX).count(), 3, "an item is added once");
    }

    /// `newest` as a store gives them to `page_of`, each seen.
    fn all_seen(
        newest: impl IntoIterator<Item = (Position, HistoryEntry)>,
    ) -> impl Iterator<Item = StoreResult<Option<(Position, HistoryEntry)>>> {
        newest.into_iter().map(|item| Ok(Some(item)))
    }

    #[test]
    fn a_page_is_cut_from_the_newest_items() {
        let message = |seq: u64| Message {
            content: Some(if seq.is_multiple_of(2) { "odd" } else { "even" }.to_string()),
            ..make_message(id(seq as u128 + 1))
        };
        let newest = |n: u64| {
            (0..n)
                .rev()
                .map(move |seq| (Position::new(seq), HistoryEntry::Said { message: message(seq) }))
        };
        let page = page_of(all_seen(newest(5)), Position::START, 2, None).unwrap();
        assert_eq!(
            (page.entries().map(label).collect::<Vec<_>>(), page.earlier),
            (
                vec![label(&newest(5).nth(1).unwrap().1), label(&newest(5).next().unwrap().1)],
                Some(Position::new(3))
            )
        );
        let page = page_of(all_seen(newest(5)), Position::new(3), 5, None).unwrap();
        assert_eq!(
            (page.items.len(), page.earlier),
            (2, None),
            "none before where the view begins"
        );
        assert_eq!(page_of(all_seen(newest(0)), Position::START, 5, None).unwrap().items.len(), 0);
        assert_eq!(
            page_of(all_seen(newest(3)), Position::START, 0, None).unwrap().items.len(),
            1,
            "at least one"
        );
        let odd = Filter::containing("odd");
        let kept: Vec<(Position, Recorded)> = newest(5)
            .map(|(seq, entry)| (seq, Recorded::new(entry, ActorId::None, Audience::Everybody)))
            .collect();
        let page = page_of(
            kept.iter().map(|item| seen(&Reader::Channel, &[], Some(&odd), item)),
            Position::START,
            2,
            None,
        )
        .unwrap();
        assert_eq!(
            (page.entries().map(label).collect::<Vec<_>>(), page.earlier),
            (
                vec![label(&newest(5).nth(2).unwrap().1), label(&newest(5).next().unwrap().1)],
                Some(Position::new(2))
            ),
            "a page of those kept, full, and an older one kept"
        );
    }

    /// Whether `filter` keeps `entry`, recorded as everybody was given it.
    fn keeps(filter: &Filter<HistoryField>, entry: HistoryEntry) -> bool {
        let item = (Position::START, Recorded::new(entry, ActorId::System, Audience::Everybody));
        seen(&Reader::Channel, &[], Some(filter), &item).unwrap().is_some()
    }

    /// Issue 421: what a page keeps is a condition over the message as it
    /// is recorded.
    #[test]
    fn a_filter_reads_a_message_as_it_is_recorded() {
        let (alice, bob) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        let said = |message: Message| HistoryEntry::Said { message };
        let by_alice = Message {
            sender_id: alice,
            content: Some("Zoë likes tea".to_string()),
            mentions: vec![bob],
            ..make_message(id(1))
        };
        let other = HistoryEntry::ResponseRequested {
            by: alice,
            reason: RequestReason::Mentioned,
            at: Utc::now(),
        };

        assert!(
            keeps(&Filter::said(), said(by_alice.clone()))
                && !keeps(&Filter::said(), other.clone()),
            "a message said"
        );
        assert!(
            keeps(&Filter::said_by(alice), said(by_alice.clone()))
                && !keeps(&Filter::said_by(bob), said(by_alice.clone()))
        );
        assert!(!keeps(&Filter::said_by(alice), other), "whoever asked for it, it is no message");
        assert!(keeps(&Filter::negated(Filter::said_by(bob)), said(by_alice.clone())));
        assert!(
            keeps(&Filter::containing("ZOE LIKES"), said(by_alice.clone()))
                && !keeps(&Filter::containing("coffee"), said(by_alice.clone())),
            "words, as names are compared"
        );
        assert!(
            keeps(&Filter::mentioning(bob), said(by_alice.clone()))
                && !keeps(&Filter::mentioning(alice), said(by_alice.clone()))
        );

        for message in [
            Message { content: None, ..by_alice.clone() },
            Message { content: Some(String::new()), ..by_alice.clone() },
        ] {
            assert!(
                !keeps(&Filter::written(), said(message.clone()))
                    && !keeps(&Filter::heard(), said(message)),
                "without words"
            );
        }
        let attached = Message {
            content: None,
            attachments: vec![make_content_ref("screen.png", b"png")],
            ..by_alice.clone()
        };
        assert!(
            !keeps(&Filter::written(), said(attached.clone()))
                && keeps(&Filter::heard(), said(attached.clone())),
            "an attachment alone is heard, though nothing is written"
        );
        for message in [
            by_alice.clone(),
            attached,
            Message { role: MessageRole::Assistant, ..by_alice.clone() },
            Message { role: MessageRole::Tool, ..by_alice.clone() },
            Message { internal: true, ..by_alice.clone() },
        ] {
            assert_eq!(
                keeps(&Filter::heard(), said(message.clone())),
                message.is_heard(),
                "heard as {:?}, internal {}",
                message.role,
                message.internal
            );
        }
    }

    /// Issue 421: what may be copied of a channel is read as a condition
    /// over the level and compartments of a message - the same as the
    /// rule it stands for.
    #[test]
    fn what_may_be_copied_is_read_as_the_flow_rule_decides() {
        let compartments = Compartments::new([
            (Compartment::new("hr"), vec![Compartment::new("finance")]),
            (Compartment::new("finance"), vec![]),
            (Compartment::new("legal"), vec![]),
        ])
        .unwrap();
        let classified = |level, names: &[&str]| {
            Classification::new(level, names.iter().map(|name| Compartment::new(*name))).unwrap()
        };
        let mut samples = vec![Classification::PUBLIC];
        for level in [
            Confidentiality::Internal,
            Confidentiality::Confidential,
            Confidentiality::StrictlyConfidential,
        ] {
            for names in [
                &[][..],
                &["hr"],
                &["finance"],
                &["legal"],
                &["hr", "finance"],
                &["finance", "legal"],
            ] {
                samples.push(classified(level, names));
            }
        }
        let mut readings = vec![Reading::Nothing, Reading::Whole];
        readings.extend(samples.iter().cloned().map(Reading::Released));
        for reading in &readings {
            let filter = Filter::readable(reading, &compartments);
            for classification in &samples {
                let message = HistoryEntry::Said {
                    message: Message {
                        classification: classification.clone(),
                        ..make_message(id(1))
                    },
                };
                let kept = filter.as_ref().is_none_or(|filter| keeps(filter, message));
                assert_eq!(
                    kept,
                    reading.admits(classification, &compartments),
                    "{classification} under {reading:?}"
                );
            }
        }
        let other = HistoryEntry::ResponseRequested {
            by: ActorId::System,
            reason: RequestReason::Chosen,
            at: Utc::now(),
        };
        assert!(
            !keeps(
                &Filter::readable(&Reading::Released(Classification::PUBLIC), &compartments)
                    .unwrap(),
                other
            ),
            "messages only"
        );
    }

    /// Issue 451: the rule of what a reader is given, as a filter over an
    /// item as it is kept, decides as `Recorded::seen_by` does - for every
    /// reader, sender, audience, kind of entry and position, in a member's
    /// ranges or out of them.
    #[test]
    fn what_a_reader_is_given_is_the_same_as_a_filter() {
        use caelumora_domain::actor::EventContext;
        use caelumora_domain::agent::AgentId;
        use caelumora_domain::approval::ApproverId;
        use caelumora_domain::history::Recipients;
        let alice = ActorId::User(UserId::new());
        let bob = ActorId::User(UserId::new());
        let agent = ActorId::Agent(AgentId::new());
        let approver = ActorId::Approver(ApproverId::new());
        let actors = [alice, bob, agent, approver, ActorId::System];
        let entries = [
            HistoryEntry::Said { message: make_message(id(1)) },
            HistoryEntry::Said { message: Message { content: None, ..make_message(id(2)) } },
            HistoryEntry::ResponseRequested {
                by: ActorId::System,
                reason: RequestReason::Chosen,
                at: Utc::now(),
            },
        ];
        let audiences = [
            Recipients::NOBODY,
            Recipients::members(false),
            Recipients::members(true),
            Recipients::APPROVERS,
            Recipients::only(vec![bob]),
            Recipients::only(vec![agent]),
            Recipients { named: vec![agent], ..Recipients::members(false) },
        ];
        let rangings = [
            vec![],
            vec![Position::new(2)..Position::new(6)],
            vec![Position::START..Position::new(1), Position::new(8)..Position::MAX],
        ];
        let mut readers = vec![Reader::Channel, Reader::Outsider];
        readers.extend(actors.iter().map(|actor| Reader::Participant(*actor)));
        let mut decided = [0, 0];
        for entry in &entries {
            for sender in actors {
                for recipients in &audiences {
                    let item = Recorded {
                        entry: entry.clone(),
                        sender,
                        recipients: recipients.clone(),
                        context: EventContext::default(),
                        timestamp: Utc::now(),
                    };
                    for seq in [Position::START, Position::new(5), Position::new(9)] {
                        let kept = serde_json::to_value(KeptItem::of(seq, item.clone())).unwrap();
                        for reader in &readers {
                            for ranges in &rangings {
                                let by_filter = Filter::seen_by(reader, ranges)
                                    .is_none_or(|filter| filter.matches(&kept));
                                assert_eq!(by_filter, item.seen_by(seq, reader, ranges), "{reader:?} of {entry:?} from {sender} to {recipients:?} at {seq} in {ranges:?}");
                                decided[usize::from(by_filter)] += 1;
                            }
                        }
                    }
                }
            }
        }
        assert!(decided.iter().all(|count| *count > 1000), "either way, often: {decided:?}");
    }

    /// An item is kept with its position and whether it is published, and
    /// read back as it was recorded; a page's filter may name both.
    #[test]
    fn an_item_is_kept_with_its_position_and_whether_it_is_published() {
        let item = Recorded::new(
            HistoryEntry::Said { message: make_message(id(1)) },
            ActorId::System,
            Audience::Everybody,
        );
        let kept = KeptItem::of(Position::new(7), item);
        let read: KeptItem = serde_json::from_value(serde_json::to_value(&kept).unwrap()).unwrap();
        assert_eq!(
            (read.seq, read.published, read.item.entry.said().map(|m| m.id)),
            (Position::new(7), true, Some(id(1)))
        );
        let at = |filter: Filter<HistoryField>| {
            seen(&Reader::Channel, &[], Some(&filter), &(kept.seq, kept.item.clone()))
                .unwrap()
                .is_some()
        };
        assert!(
            at(Filter::within(HistoryField::Position, 7..8))
                && !at(Filter::within(HistoryField::Position, 0..7)),
            "by its position"
        );
        assert!(at(Filter::eq(HistoryField::Published, true)), "and whether it is published");
    }

    /// Issue 417: who took part when is read off the chain - from a
    /// share before the join, or else from the join, up to where the leave
    /// says delivery stopped -, and what went to every member is given to
    /// whom it was a member then; what went to one by name, or what one
    /// sent, regardless.
    #[test]
    fn what_went_to_every_member_is_given_to_who_was_one_then() {
        use caelumora_domain::user::UserId;
        let (alice, bob) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        let joined = |participant| HistoryEntry::ParticipantJoined {
            participant,
            name: String::new(),
            by: participant,
            by_name: String::new(),
            at: Utc::now(),
        };
        let said = |n: u128, by: ActorId| HistoryEntry::Said {
            message: Message { sender_id: by, ..make_message(id(n)) },
        };
        let mut history = History::default();
        for (entry, sender, audience) in [
            (joined(alice), alice, Audience::Everybody),  // 0
            (said(1, alice), alice, Audience::Everybody), // 1
            (
                HistoryEntry::HistoryShared {
                    participant: bob,
                    from: Position::new(1),
                    by: alice,
                    by_name: String::new(),
                    at: Utc::now(),
                },
                ActorId::System,
                Audience::Everybody,
            ), // 2
            (joined(bob), alice, Audience::Everybody),    // 3
            (said(2, bob), bob, Audience::Everybody),     // 4
            (said(3, alice), alice, Audience::Everybody), // 5: hers, after delivery to her stopped
            (
                HistoryEntry::ParticipantLeft {
                    participant: alice,
                    name: String::new(),
                    by: alice,
                    by_name: String::new(),
                    until: Position::new(5),
                    at: Utc::now(),
                },
                alice,
                Audience::Everybody,
            ), // 6
            (said(4, bob), bob, Audience::Everybody),     // 7
            (said(5, bob), bob, Audience::Only(vec![alice])), // 8
        ] {
            assert!(history.enter(Recorded::new(entry, sender, audience)));
        }
        let attendance = Attendance::of(history.all_entries());
        let starts_and_ends = |participant| {
            attendance
                .of_participant(participant)
                .into_iter()
                .map(|range| (range.start.get(), range.end.get()))
                .collect::<Vec<_>>()
        };
        assert_eq!(starts_and_ends(alice), [(0, 5)], "from her join to where delivery stopped");
        assert_eq!(
            starts_and_ends(bob),
            [(1, u64::MAX)],
            "from what was shared with him, a participant still"
        );
        let seen = |reader: ActorId| -> Vec<u64> {
            let reading = Reader::Participant(reader);
            let ranges = attendance.of_reader(&reading);
            history
                .all_entries()
                .filter(|node| node.entry.seen_by(node.seq, &reading, &ranges))
                .map(|node| node.seq.get())
                .collect()
        };
        assert_eq!(
            seen(alice),
            [0, 1, 3, 4, 5, 6, 8],
            "hers, what went to every member while she was one, and what went to her by name"
        );
        assert_eq!(
            seen(bob),
            [1, 2, 3, 4, 5, 6, 7, 8],
            "what was shared with him, and since - the last his own"
        );
        assert_eq!(
            Attendance::of(history.all_entries().chain(history.all_entries())).of_participant(bob),
            attendance.of_participant(bob),
            "an entry taken in twice changes nothing"
        );
    }
}
