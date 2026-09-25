//! Reading the notes agents published (issue 17).
//!
//! Every agent writes only its own notes, into its own state
//! (`TurnContext::note`, taken over when its turn completes). What others
//! published is read from their states in the `StateStore`, where an agent's
//! state is written after every step - so a reader sees notes of finished
//! turns only, as principle 3 in issue 19 requires.

use crate::runtime::ActorRuntime;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::digest::Digest;
use caelumora_model::note::Note;
use std::fmt::Debug;

/// The notes one agent published in a channel.
#[derive(Debug, Clone)]
pub struct PublishedNotes {
    pub agent: ActorId,
    pub name: String,
    pub notes: Vec<Note>,
    /// The digest of the agent's note chain: the state of knowledge read.
    pub digest: Option<Digest>,
}

/// Read access to published notes, given to a turn. Phase 2 of issue 17
/// extends it to the other channels of the session.
#[async_trait]
pub trait NoteReader: Send + Sync + Debug {
    /// The notes every agent of the turn's channel published, in no
    /// particular order.
    async fn channel_notes(&self) -> Result<Vec<PublishedNotes>>;
}

/// Reads the notes of the channel an actor runs in.
#[derive(Debug)]
pub struct ChannelNoteReader {
    runtime: ActorRuntime,
}

impl ChannelNoteReader {
    pub fn new(runtime: ActorRuntime) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl NoteReader for ChannelNoteReader {
    async fn channel_notes(&self) -> Result<Vec<PublishedNotes>> {
        match self.runtime.channel() {
            Some(channel) => channel.published_notes().await,
            // Not attached to a channel (yet): nothing is published.
            None => Ok(Vec::new()),
        }
    }
}
