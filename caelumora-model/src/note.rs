//! Notes (issue 17): what an agent writes down in a channel, for itself and
//! for others to read.
//!
//! Notes belong to the agent that wrote them and are published in the
//! channel they were written in (principles 2, 3 and 3a in issue 19). Each
//! agent keeps them per channel as a [`NoteChain`], a ledger
//! (`crate::ledger`, issue 92): a note is added, revoked (removed) or
//! reclassified. It branches with the channel's conversation.

use crate::digest::Digest;
use crate::ledger::{Change, Item, Ledger, LedgerKind};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The number of a note in its chain: 1 for the first, counting on. Small,
/// because a model has to repeat it to revoke the note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NoteId(pub u32);

impl fmt::Display for NoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u32> for NoteId {
    fn from(n: u32) -> Self {
        NoteId(n)
    }
}

impl From<NoteId> for u32 {
    fn from(id: NoteId) -> Self {
        id.0
    }
}

/// Where a change to notes was made: the channel is the notes' own, so what
/// is recorded is the agent's transcript when the change was taken over.
/// Not part of the digest: a reader learns the change and the message it
/// follows, not the agent's transcript.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NoteOrigin {
    pub transcript: Option<Digest>,
}

/// The kind of ledger an agent's notes are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notes {}

impl LedgerKind for Notes {
    type Id = NoteId;
    type Origin = NoteOrigin;
    const SCHEME: &'static str = "caelumora.note-chain.v2";
    const ORIGIN_DIGESTED: bool = false;
}

/// A change to an agent's notes.
pub type NoteChange = Change<NoteId>;
/// A note as it stands now.
pub type Note = Item<Notes>;
/// An agent's notes in one channel, as a chain of changes.
pub type NoteChain = Ledger<Notes>;
