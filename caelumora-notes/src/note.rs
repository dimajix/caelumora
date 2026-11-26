//! What an agent notes in a channel (issues 17, 51, 607): its notes, published there, and its
//! private notes, read by it alone - each a ledger (`caelumora_domain::ledger`, issue 92) of the
//! agent in the channel, kept in one document of the plugin's kind per agent and channel.
//!
//! Notes belong to the agent that wrote them and are published in the channel they were written in
//! (principles 2, 3 and 3a in issue 19): a note is added, updated, revoked (removed) or
//! reclassified, by its author alone. The changes a turn makes are staged by it (`NoteChange`) and
//! committed by the plugin when it completes.

use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::AgentId;
use caelumora_domain::channel::ChannelId;
use caelumora_domain::define_id;
use caelumora_domain::digest::Digest;
use caelumora_domain::ledger::{Change, Item, Ledger, LedgerKind};
use caelumora_domain::query::Field;
use caelumora_domain::removal::Removal;
use caelumora_domain::security::{Secured, SecurityAttributes};
use caelumora_domain::store::{EntityKind, NoEntity, Revision, Stored};
use caelumora_domain::turn::StagedChange;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Not;
use uuid::Uuid;

/// The number of a note in its chain: 1 for the first, counting on. Small, because a model has to
/// repeat it to revoke the note.
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

/// The kind of ledger an agent's notes are: the channel is the notes' own, so where a change was
/// made records nothing beside the message it follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notes {}

impl LedgerKind for Notes {
    type Id = NoteId;
    type Origin = ();
    const SCHEME: &'static str = "caelumora.note-chain.v3";
    const ORIGIN_DIGESTED: bool = false;
}

/// A note as it stands now.
pub type Note = Item<Notes>;
/// One of an agent's chains of notes in a channel.
pub type NoteChain = Ledger<Notes>;

/// Which of an agent's chains in a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    /// Published in the channel: its participants read them.
    Public,
    /// Read by the agent alone, numbered independently of the public ones, so that gaps in those
    /// do not reveal private notes.
    Private,
}

impl NoteKind {
    /// What part of the agent's notes a read of this chain read (`KnowledgeSource::Actor`): none
    /// for its notes, `private` for its private ones.
    pub fn place(self) -> Option<&'static str> {
        match self {
            NoteKind::Public => None,
            NoteKind::Private => Some("private"),
        }
    }
}

/// A change to one of an agent's chains in the channel of its work, as a turn stages it: the
/// plugin commits it when the turn completes, to the chain of the work's agent in the work's
/// channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteChange {
    pub kind: NoteKind,
    pub change: Change<NoteId>,
}

impl StagedChange for NoteChange {
    const KIND: &'static str = "notes";
}

define_id!(AgentNotesId);

impl AgentNotesId {
    /// The id of the notes of `agent` in `channel`: one for each agent and channel, found without
    /// looking it up.
    pub fn of(agent: AgentId, channel: ChannelId) -> Self {
        Self(Uuid::new_v5(&agent.0, channel.0.as_bytes()))
    }
}

/// The notes of one agent in one channel, as they are kept (issue 607): its public and its private
/// chain, owned by the agent's owner. Read for the channel's board in the kernel's name, once the
/// channel's view decided what its viewer reads; written in the name of the agent's work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentNotes {
    /// Its agent and channel's (`AgentNotesId::of`).
    pub id: AgentNotesId,
    pub agent: AgentId,
    pub channel: ChannelId,
    /// The notes the agent published in the channel.
    pub public: NoteChain,
    /// The notes it keeps there for itself alone.
    pub private: NoteChain,
    /// Its owner - the agent's -, and no access list.
    pub security_attributes: SecurityAttributes,
    /// Set when it was deleted (issue 164), which only its purge does, on the way to removing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
    /// Its revision (issue 492), counted up by every write of it.
    #[serde(default)]
    pub revision: Revision,
    /// Whether its purge has begun (issue 492): set by the store alone.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub purging: bool,
}

impl AgentNotes {
    /// The notes of `agent` in `channel`, owned by `owner`, that note nothing yet.
    pub fn new(agent: AgentId, channel: ChannelId, owner: ActorId) -> Self {
        Self {
            id: AgentNotesId::of(agent, channel),
            agent,
            channel,
            public: NoteChain::default(),
            private: NoteChain::default(),
            security_attributes: SecurityAttributes::owned_by(owner),
            deleted: None,
            revision: Revision::default(),
            purging: false,
        }
    }

    /// Its chain of `kind`.
    pub fn chain(&self, kind: NoteKind) -> &NoteChain {
        match kind {
            NoteKind::Public => &self.public,
            NoteKind::Private => &self.private,
        }
    }

    /// Its chain of `kind`, to change it.
    pub fn chain_mut(&mut self, kind: NoteKind) -> &mut NoteChain {
        match kind {
            NoteKind::Public => &mut self.public,
            NoteKind::Private => &mut self.private,
        }
    }
}

/// What a query of agents' notes may name: whose they are, and where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotesField {
    Agent,
    Channel,
}

impl Field for NotesField {
    fn path(self) -> &'static [&'static str] {
        match self {
            NotesField::Agent => &["agent"],
            NotesField::Channel => &["channel"],
        }
    }
}

impl Secured for AgentNotes {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl Stored for AgentNotes {
    const KIND: EntityKind = EntityKind::new("notes");
    type Id = AgentNotesId;
    type Within = NoEntity;
    type Field = NotesField;

    fn id(&self) -> AgentNotesId {
        self.id
    }

    fn address(&self) -> AgentNotesId {
        self.id()
    }

    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }

    fn revision(&self) -> Revision {
        self.revision
    }

    fn set_revision(&mut self, revision: Revision) {
        self.revision = revision;
    }

    fn purging(&self) -> bool {
        self.purging
    }

    fn set_purging(&mut self) {
        self.purging = true;
    }
}

/// The notes one agent published in a channel, as far as their reader may read them.
#[derive(Debug, Clone, PartialEq)]
pub struct PublishedNotes {
    pub agent: ActorId,
    pub name: String,
    pub notes: Vec<Note>,
    /// The digest of the agent's chain: the state of knowledge read.
    pub digest: Option<Digest>,
}

/// An agent's private notes in another channel of its session, as far as they may be copied into
/// where it reads from: what of that channel it knows itself - no view of the channel, which it
/// may no longer reach.
#[derive(Debug, Clone, PartialEq)]
pub struct PrivateNotesElsewhere {
    pub channel: ChannelId,
    pub name: String,
    pub notes: Vec<Note>,
    pub digest: Option<Digest>,
}
