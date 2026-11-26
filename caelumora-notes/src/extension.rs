//! An agent's notes as its work reaches them (issue 607): what the work reads of them - its
//! agent's own in its channel, those published where it may read, its agent's private notes of
//! the session's other channels (`NotesReader`) -, and what a turn changes of them (`NotesWriter`),
//! each bound by the plugin to every piece of work, which decides what may be read and checks every
//! change as it commits it.

use crate::note::{NoteChain, NoteChange, NoteId, NoteKind, PrivateNotesElsewhere, PublishedNotes};
use async_trait::async_trait;
use caelumora_api::reader::ViewResult;
use caelumora_api::staging::Committing;
use caelumora_domain::channel::ChannelId;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::store::StoreResult;

/// What a work reads of notes: its agent's own in its channel, and what it may read elsewhere.
#[async_trait]
pub trait NotesReader: Send + Sync {
    /// The agent's chain of `kind` in the work's channel, as it was stored when the reader was
    /// made: what stands and its digest.
    fn own(&self, kind: NoteKind) -> &NoteChain;

    /// What the agents taking part in `channel` of the work's session published, as far as the
    /// work may read it - of a channel read entry by entry, those released to where it reads from.
    ///
    /// # Errors
    /// `NoChannelNamed` for a channel that cannot be read, and for one read entry by entry of which
    /// nothing is released, so that nothing can be probed; `Failed` if what is noted there cannot
    /// be read.
    async fn published_in(&self, channel: ChannelId) -> ViewResult<Vec<PublishedNotes>>;

    /// The agent's private notes in the session's other channels it takes or took part in, as far
    /// as each may be copied into where the work reads from, sorted by the channel's name.
    async fn own_private_elsewhere(&self) -> Vec<PrivateNotesElsewhere>;
}

/// What a turn changes of its agent's notes in the work's channel. The changes themselves the turn
/// stages (`Turn::stage`), and the plugin commits them when the turn completes, each checked.
#[async_trait]
pub trait NotesWriter: Committing<NoteChange> {
    /// A number for a new note of `kind`, never handed out before: reserved at once, since the
    /// agent's turns in the channel's other threads note at the same time - as the memory's are.
    ///
    /// # Errors
    /// If the notes cannot be read to count on from them.
    async fn reserve_id(&self, kind: NoteKind) -> StoreResult<NoteId>;

    /// Whether a note may be reclassified as `classification` here: what a tool answers before the
    /// turn stages it, asked again as it is committed.
    async fn may_reclassify(&self, classification: &Classification) -> bool;
}
