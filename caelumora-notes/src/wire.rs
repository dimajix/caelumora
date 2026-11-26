//! The wire of notes (issues 17, 233, 607): the notes of a channel's board, and an agent's private
//! notes, as a client reads them.

use crate::note::{Note, PublishedNotes};
use caelumora_domain::actor::ActorId;
use caelumora_domain::channel::ChannelId;
use caelumora_domain::digest::Digest;
use serde::Serialize;

/// The notes one agent published in a channel.
#[derive(Debug, Serialize)]
pub struct PublishedNotesResponse {
    pub agent_id: ActorId,
    pub name: String,
    pub notes: Vec<Note>,
}

impl From<PublishedNotes> for PublishedNotesResponse {
    fn from(published: PublishedNotes) -> Self {
        Self { agent_id: published.agent, name: published.name, notes: published.notes }
    }
}

/// The private notes one agent wrote in one channel of a session (issue 233), with the digest of
/// their chain.
#[derive(Debug, Serialize)]
pub struct PrivateNotesResponse {
    pub channel_id: ChannelId,
    pub channel_name: String,
    pub notes: Vec<Note>,
    pub digest: Option<Digest>,
}
