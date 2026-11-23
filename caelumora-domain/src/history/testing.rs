//! What the tests of several crates read of a history held in memory, and how they record in it by
//! hand (issues 392, 599): its items and messages as they stand, what a message was produced from,
//! and what happened, recorded without an envelope where a context says. Nothing in production
//! reads a history so: the items are read from the store (`HistoryStore::page`), and so is what a
//! message was produced from (`HistoryStore::said`); and what a channel records comes in an
//! envelope (`History::enter`, `Recorded::of`), but for a fitting (`History::fit_into`).

use super::{fold, History, HistoryEntry, Recorded};
use crate::actor::EventContext;
use crate::chain::{Chain, Position};
use crate::confidentiality::Classification;
use crate::digest::Digest;
use crate::message::{Message, MessageId};
use crate::work::TurnId;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// The items of `history` as they stand, in the order they happened: every
/// message said, with every change since folded into it, and the marks of
/// turns, steps, commands, activities and compactions; no change, which is in
/// the message it changes. What a store derives as its items
/// (`StandingItems`), read by the one rule of folding (`fold`).
pub fn items_of(history: &History) -> Vec<HistoryEntry> {
    let mut folded: HashMap<Position, Message> =
        fold(history.entries()).into_iter().map(|(node, message)| (node.seq, message)).collect();
    history
        .entries()
        .filter_map(|node| match &node.entry.entry {
            HistoryEntry::Said { .. } => {
                folded.remove(&node.seq).map(|message| HistoryEntry::Said { message })
            }
            entry => entry.is_item().then(|| entry.clone()),
        })
        .collect()
}

/// The messages of `history` as they stand (`items_of`).
pub fn messages_of(history: &History) -> Vec<Message> {
    items_of(history).iter().filter_map(HistoryEntry::said).cloned().collect()
}

/// What `history` recorded the message `id` was produced from, and in which
/// turn (`HistoryEntry::context`); `None` if its holder did not produce it.
pub fn recorded_context(
    history: &History,
    id: MessageId,
) -> Option<(Option<Digest>, Digest, TurnId)> {
    let said = history
        .entries()
        .rev()
        .find(|node| node.entry.entry.said().is_some_and(|message| message.id == id))?;
    said.entry.entry.context()
}

/// What the tests build a history with by hand (issue 599): messages said, items and changes,
/// each recorded where a context says, as a channel records what comes in an envelope.
impl Chain<Recorded> {
    /// Appends `msgs`, each said where `context` says, as `push` records it.
    pub fn extend(&mut self, context: &EventContext, msgs: Vec<Message>) {
        for m in msgs {
            self.push(context, m);
        }
    }

    /// Appends `msg`, said where `context` says - with what it was produced from, if it says.
    pub fn push(&mut self, context: &EventContext, msg: Message) {
        self.record(context, HistoryEntry::Said { message: msg });
    }

    /// Records `item` where `context` says, what happened that is no change of a message
    /// (`is_item`): a message said - as `push` records it, with what it was produced from -, how a
    /// turn ended (issue 258), after its messages, where a step of its plan began (issue 263),
    /// before the messages of the step, a command carried out (issue 269) or an activity ended
    /// (issue 284), before the turn's end, a compaction (issue 16), before what replaced the older
    /// messages, someone added or removed (issue 408). A change is recorded by its own method -
    /// `modify`, `revoke` and `reclassify`, which say whether it applies, and `fit_into`, which
    /// always records.
    pub(crate) fn record(&mut self, context: &EventContext, item: HistoryEntry) {
        debug_assert!(item.is_item(), "a change is recorded by its own method");
        self.enter(Recorded::delivered(item, context));
    }

    /// Records where `context` says that the message `modified.id` reads as `modified` from now
    /// on (issue 234). `false`, and nothing is recorded, if there is no such message or it cannot
    /// be modified so as it stands (`Message::modify`): taken back, or by another sender; one
    /// before what it holds is recorded (`is_changed`).
    pub fn modify(&mut self, context: &EventContext, modified: Message) -> bool {
        self.enter(Recorded::delivered(HistoryEntry::Modified { message: modified }, context))
    }

    /// Records where `context` says that the message `id` was taken back at `at` (issue 234).
    /// `false`, and nothing is recorded, if there is no such message or it was taken back
    /// already; one before what it holds is recorded (`is_changed`).
    pub fn revoke(&mut self, context: &EventContext, id: MessageId, at: DateTime<Utc>) -> bool {
        self.enter(Recorded::delivered(HistoryEntry::Revoked { message: id, at }, context))
    }

    /// Records where `context` says that message `id` is classified `classification` from now on
    /// (issue 133). `false`, and nothing is recorded, if there is no such message or it was taken
    /// back (`Message::reclassify`); one before what it holds is recorded (`is_changed`).
    pub fn reclassify(
        &mut self,
        context: &EventContext,
        id: MessageId,
        classification: Classification,
    ) -> bool {
        let reclassified = HistoryEntry::Reclassified { message: id, classification };
        self.enter(Recorded::delivered(reclassified, context))
    }
}
