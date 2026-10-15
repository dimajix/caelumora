//! A ledger: entries of text that are added, updated, removed and
//! reclassified, kept as a chain of those changes (issue 92). An agent's notes
//! (`crate::note`, issue 17) and its memory (`crate::memory`, issue 62) are
//! ledgers; they work alike and differ only in what they are called and in
//! where a change is recorded to have been made - which a [`LedgerKind`]
//! says.
//!
//! What stands now is the chain's active branch read from start to end
//! (issue 08). A new version of an entry keeps its number (issue 302). The
//! number of an entry is never given twice, on any branch: a reader may have
//! read it before it was removed, or on a branch no longer active.

use crate::chain::Chain;
use crate::confidentiality::Classification;
use crate::message::MessageId;
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use crate::digest;

/// What makes a ledger of one kind: the number of its entries, where a
/// change is recorded to have been made, and how its chain is digested.
pub trait LedgerKind {
    /// The number of an entry: 1 for the first, counting on. Small, because
    /// a model has to repeat it to name the entry.
    type Id: Copy + Eq + Debug + Serialize + DeserializeOwned + From<u32> + Into<u32>;
    /// Where a change was made, recorded with it; what an entry stands with
    /// is where it was added.
    type Origin: Clone + PartialEq + Debug + Serialize + DeserializeOwned;
    /// The scheme of the chain's digest (`crate::digest::ledger_chain`).
    const SCHEME: &'static str;
    /// Whether the origin is part of what a reader learns, and so of the
    /// digest.
    const ORIGIN_DIGESTED: bool;
}

/// A change to a ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Change<Id> {
    /// An entry, classified as the turn that wrote it had seen (issues 133,
    /// 168): what the agent derives carries its turn's label, at most its
    /// channel's classification, and a later change of the channel changes
    /// none of its entries.
    Add { id: Id, text: String, classification: Classification },
    /// The entry `id` says `text` from now on (issue 302), classified as the
    /// turn that wrote it had seen, as a new entry is: a release of the old
    /// text does not carry over. It stands where it was updated.
    Update { id: Id, text: String, classification: Classification },
    Remove { id: Id },
    /// The entry `id` is classified `classification` from now on (issue
    /// 133): released, or raised.
    Reclassify { id: Id, classification: Classification },
}

/// An entry of a [`Ledger`]'s chain: a change and where it was made; its
/// node carries the digest of the chain up to and including it.
#[derive(Debug, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct LedgerEntry<K: LedgerKind> {
    pub change: Change<K::Id>,
    pub origin: K::Origin,
    /// The message the change follows: the last one of the turn that made
    /// it (`MessageId::default()` if there was none).
    pub after: MessageId,
    pub written_at: DateTime<Utc>,
}

// By hand: the kind is a marker, and a derived `Clone` would ask it to be
// `Clone` too.
impl<K: LedgerKind> Clone for LedgerEntry<K> {
    fn clone(&self) -> Self {
        Self { change: self.change.clone(), origin: self.origin.clone(), after: self.after, written_at: self.written_at }
    }
}

/// An entry of a ledger as it stands now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct Item<K: LedgerKind> {
    pub id: K::Id,
    pub text: String,
    /// The label of the turn that wrote it, unless it was reclassified
    /// since (issues 133, 168).
    pub classification: Classification,
    /// Where it was added, or last updated.
    pub origin: K::Origin,
    /// The message it follows: the one its text was written after.
    pub after: MessageId,
    pub written_at: DateTime<Utc>,
}

/// A ledger, as a chain of changes.
pub type Ledger<K> = Chain<LedgerEntry<K>>;

impl<K: LedgerKind> Chain<LedgerEntry<K>> {
    /// Appends `change`, made at `origin` in the turn that ended with
    /// message `after`.
    pub fn push(&mut self, change: Change<K::Id>, origin: K::Origin, after: MessageId) {
        self.push_with(
            LedgerEntry { change, origin, after, written_at: Utc::now() },
            |previous, entry| digest::ledger_chain::<K>(previous, &entry.change, &entry.origin, entry.after),
        );
    }

    /// The entries as they stand on the active branch: every entry that was
    /// not removed, oldest first.
    pub fn current(&self) -> Vec<Item<K>> {
        let mut items: Vec<Item<K>> = Vec::new();
        for node in self.entries() {
            let entry = &node.entry;
            fold(&mut items, &entry.change, &entry.origin, entry.after, entry.written_at);
        }
        items
    }

    /// The number the next entry gets: one more than any entry ever added,
    /// on any branch.
    pub fn next_id(&self) -> K::Id {
        let highest = self.all_entries().filter_map(|e| added(&e.entry.change)).max().unwrap_or(0);
        K::Id::from(highest + 1)
    }

    /// The ledger as it stands with `staged` - changes not part of it yet,
    /// each with where it is made: a turn's, until the turn completes
    /// (issue 156) - as if they had been appended.
    pub fn with_staged<'c>(&self, staged: impl IntoIterator<Item = (&'c Change<K::Id>, K::Origin)>) -> Staged<K>
    where
        K::Id: 'c,
    {
        let mut items = self.current();
        let mut next_id: u32 = self.next_id().into();
        let now = Utc::now();
        for (change, origin) in staged {
            if let Some(id) = added(change) {
                next_id = next_id.max(id + 1);
            }
            fold(&mut items, change, &origin, MessageId::default(), now);
        }
        Staged { items, next_id: K::Id::from(next_id) }
    }
}

/// The number of the entry `change` adds, if it adds one.
fn added<Id: Copy + Into<u32>>(change: &Change<Id>) -> Option<u32> {
    match change {
        Change::Add { id, .. } => Some((*id).into()),
        Change::Update { .. } | Change::Remove { .. } | Change::Reclassify { .. } => None,
    }
}

/// Folds `change`, made at `origin` after message `after` at `written_at`,
/// into `items`: what stands before it becomes what stands after it.
fn fold<K: LedgerKind>(items: &mut Vec<Item<K>>, change: &Change<K::Id>, origin: &K::Origin, after: MessageId, written_at: DateTime<Utc>) {
    match change {
        Change::Add { id, text, classification } => items.push(Item {
            id: *id,
            text: text.clone(),
            classification: classification.clone(),
            origin: origin.clone(),
            after,
            written_at,
        }),
        Change::Update { id, text, classification } => {
            if let Some(item) = items.iter_mut().find(|i| i.id == *id) {
                item.text = text.clone();
                item.classification = classification.clone();
                item.origin = origin.clone();
                item.after = after;
                item.written_at = written_at;
            }
        }
        Change::Remove { id } => items.retain(|i| i.id != *id),
        Change::Reclassify { id, classification } => {
            if let Some(item) = items.iter_mut().find(|i| i.id == *id) {
                item.classification = classification.clone();
            }
        }
    }
}

/// A ledger as it stands with changes not part of it yet (see
/// [`Ledger::with_staged`]). An entry added by one of them follows
/// `MessageId::default()` and was written when this was made: where it
/// will follow and when it will be written is known only once it is
/// appended.
#[derive(Debug, Clone)]
pub struct Staged<K: LedgerKind> {
    items: Vec<Item<K>>,
    next_id: K::Id,
}

impl<K: LedgerKind> Staged<K> {
    /// The entries as they stand, oldest first.
    pub fn current(&self) -> &[Item<K>] {
        &self.items
    }

    /// Entry `id`, if it stands.
    pub fn get(&self, id: K::Id) -> Option<&Item<K>> {
        self.items.iter().find(|i| i.id == id)
    }

    /// The number the next entry gets: one more than any entry ever added,
    /// on any branch or by the staged changes.
    pub fn next_id(&self) -> K::Id {
        self.next_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::{NoteChain, NoteId, NoteOrigin};
    use crate::confidentiality::Confidentiality;

    fn add(id: u32, text: &str) -> Change<NoteId> {
        Change::Add { id: NoteId(id), text: text.to_string(), classification: Classification::CHANNEL_DEFAULT }
    }

    fn push(chain: &mut NoteChain, change: Change<NoteId>, after: MessageId) {
        chain.push(change, NoteOrigin::default(), after);
    }

    #[test]
    fn what_stands_is_the_chain_read_from_start_to_end() {
        let mut chain = NoteChain::default();
        assert_eq!(chain.next_id(), NoteId(1));
        push(&mut chain, add(1, "The user likes tea."), MessageId::new());
        push(&mut chain, add(2, "The user lives in Berlin."), MessageId::new());
        push(&mut chain, Change::Remove { id: NoteId(1) }, MessageId::new());
        push(&mut chain, add(3, "The user likes coffee."), MessageId::new());

        let texts: Vec<String> = chain.current().into_iter().map(|n| n.text).collect();
        assert_eq!(texts, ["The user lives in Berlin.", "The user likes coffee."]);
        assert_eq!(chain.next_id(), NoteId(4), "a removed number is not given again");
    }

    #[test]
    fn a_reclassified_entry_keeps_its_number_and_text() {
        let mut chain = NoteChain::default();
        push(&mut chain, add(1, "a"), MessageId::new());
        let released = Classification::general(Confidentiality::Public);
        push(&mut chain, Change::Reclassify { id: NoteId(1), classification: released.clone() }, MessageId::new());
        let current = chain.current();
        assert_eq!((current[0].id, current[0].text.as_str(), &current[0].classification), (NoteId(1), "a", &released));
        assert_eq!(chain.next_id(), NoteId(2));
    }

    #[test]
    fn an_updated_entry_keeps_its_number_and_place_and_stands_where_it_was_updated() {
        let mut chain = NoteChain::default();
        push(&mut chain, add(1, "The user likes tea."), MessageId::new());
        push(&mut chain, add(2, "The user lives in Berlin."), MessageId::new());
        let released = Classification::general(Confidentiality::Public);
        push(&mut chain, Change::Reclassify { id: NoteId(1), classification: released }, MessageId::new());
        let (later, internal) = (MessageId::new(), Classification::general(Confidentiality::Internal));
        push(&mut chain, Change::Update { id: NoteId(1), text: "The user likes coffee.".to_string(), classification: internal.clone() }, later);
        push(&mut chain, Change::Update { id: NoteId(7), text: "nothing".to_string(), classification: internal.clone() }, MessageId::new());

        let current = chain.current();
        let texts: Vec<(NoteId, &str)> = current.iter().map(|n| (n.id, n.text.as_str())).collect();
        assert_eq!(texts, [(NoteId(1), "The user likes coffee."), (NoteId(2), "The user lives in Berlin.")], "in its place; no entry for a number that does not stand");
        assert_eq!((&current[0].classification, current[0].after), (&internal, later), "as the turn that updated it, which the release did not cover");
        assert_eq!(chain.next_id(), NoteId(3), "an update uses up no number");

        let staged = [Change::Update { id: NoteId(2), text: "The user lives in Hamburg.".to_string(), classification: internal }];
        let standing = chain.with_staged(staged.iter().map(|c| (c, NoteOrigin::default())));
        assert_eq!(standing.get(NoteId(2)).map(|n| n.text.as_str()), Some("The user lives in Hamburg."));
        assert_eq!(standing.next_id(), NoteId(3));
    }

    #[test]
    fn a_branch_no_longer_active_does_not_stand_but_its_numbers_are_not_given_again() {
        let mut chain = NoteChain::default();
        push(&mut chain, add(1, "a"), MessageId::new());
        let fork = chain.digest();
        push(&mut chain, add(2, "b"), MessageId::new());
        push(&mut chain, Change::Remove { id: NoteId(1) }, MessageId::new());
        chain.move_head(fork.as_ref());

        let texts: Vec<String> = chain.current().into_iter().map(|n| n.text).collect();
        assert_eq!(texts, ["a"], "the removal was on the other branch too");
        assert_eq!(chain.next_id(), NoteId(3), "2 was given on the other branch");
    }

    #[test]
    fn the_digest_covers_removals_and_the_place_in_the_conversation() {
        let after = MessageId::new();
        let mut noted = NoteChain::default();
        push(&mut noted, add(1, "a"), after);
        let before_removal = noted.digest();
        push(&mut noted, Change::Remove { id: NoteId(1) }, after);
        assert_ne!(noted.digest(), before_removal);

        let mut elsewhere = NoteChain::default();
        push(&mut elsewhere, add(1, "a"), MessageId::new());
        assert_ne!(elsewhere.digest(), before_removal, "the same entry after another message");

        let loaded: NoteChain = serde_json::from_value(serde_json::to_value(&noted).unwrap()).unwrap();
        assert_eq!(loaded.digest(), noted.digest());
        assert_eq!(loaded.current(), noted.current());
    }

    #[test]
    fn what_was_appended_since_the_ledger_was_stored_replays_onto_it() {
        let mut chain = NoteChain::default();
        push(&mut chain, add(1, "a"), MessageId::new());
        let mut stored: NoteChain = serde_json::from_value(serde_json::to_value(&chain).unwrap()).unwrap();
        chain.mark_stored();
        push(&mut chain, add(2, "b"), MessageId::new());
        push(&mut chain, Change::Remove { id: NoteId(1) }, MessageId::new());

        let changes = chain.changes().expect("changed");
        assert_eq!(changes.entries.len(), 2);
        stored.apply(changes).unwrap();
        assert_eq!(stored.digest(), chain.digest());
        assert_eq!(stored.current(), chain.current());
        assert_eq!(stored.next_id(), NoteId(3));
    }

    #[test]
    fn staged_changes_stand_as_if_appended_and_use_up_their_numbers() {
        let mut chain = NoteChain::default();
        push(&mut chain, add(1, "a"), MessageId::new());
        push(&mut chain, add(2, "b"), MessageId::new());
        let released = Classification::general(Confidentiality::Public);
        let staged = [
            add(3, "c"),
            Change::Remove { id: NoteId(1) },
            Change::Reclassify { id: NoteId(3), classification: released.clone() },
            add(4, "d"),
            Change::Remove { id: NoteId(4) },
        ];
        let standing = chain.with_staged(staged.iter().map(|c| (c, NoteOrigin::default())));

        let texts: Vec<&str> = standing.current().iter().map(|n| n.text.as_str()).collect();
        assert_eq!(texts, ["b", "c"]);
        assert_eq!(standing.get(NoteId(3)).map(|n| &n.classification), Some(&released));
        assert!(standing.get(NoteId(4)).is_none());
        assert_eq!(standing.next_id(), NoteId(5), "4 was added and removed, and is used up");
        assert_eq!(chain.current().len(), 2, "the ledger itself is unchanged");
    }
}
