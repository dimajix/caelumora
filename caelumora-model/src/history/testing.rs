//! What the tests of several crates read of a history held in memory (issue
//! 392): its items and messages as they stand, and what a message was
//! produced from. Nothing in production reads a history so: the items are
//! read from the store (`HistoryStore::page`), and so is what a message was
//! produced from (`HistoryStore::said`).

use super::{fold, History, HistoryEntry};
use crate::context::ContextRef;
use crate::message::{Message, MessageId};
use crate::work::TurnId;
use std::collections::HashMap;

/// The items of the active path of `history` as they stand, in the order
/// they happened: every message said, with every change since - on whichever
/// branch - folded into it, and the marks of turns, steps, commands,
/// activities and compactions; no change, which is in the message it
/// changes. What a store derives as its items (`StandingItems`), read by the
/// one rule of folding (`fold`).
pub fn items_of(history: &History) -> Vec<HistoryEntry> {
    let mut folded: HashMap<u64, Message> = fold(history.all_entries()).into_iter().map(|(node, message)| (node.seq, message)).collect();
    history.entries()
        .filter_map(|node| match &node.entry.entry {
            HistoryEntry::Said { .. } => folded.remove(&node.seq).map(|message| HistoryEntry::Said { message }),
            entry => entry.is_item().then(|| entry.clone()),
        })
        .collect()
}

/// The messages of the active path of `history` as they stand (`items_of`).
pub fn messages_of(history: &History) -> Vec<Message> {
    items_of(history).iter().filter_map(HistoryEntry::said).cloned().collect()
}

/// What `history` recorded the message `id` - on whichever branch - was
/// produced from, and in which turn (`HistoryEntry::context`); `None` if its
/// holder did not produce it.
pub fn recorded_context(history: &History, id: MessageId) -> Option<(ContextRef, TurnId)> {
    let said = history.all_entries().rev().find(|node| node.entry.entry.said().is_some_and(|message| message.id == id))?;
    said.entry.entry.context().map(|(context, turn)| (context.clone(), turn))
}
