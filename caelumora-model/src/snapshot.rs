//! Sessions and channels as one viewer sees them at one moment (issue 80):
//! what a client lists and offers, taken by the runtime's views
//! (`caelumora_runtime::view`, issue 163) for running and stopped sessions
//! alike, and only of what the viewer may read.
//!
//! A snapshot answers the access questions that decide what to show or
//! offer - may the viewer change the channel, read an agent's context - at
//! the time it is taken. It is data for listing and offering: an operation
//! still checks its own permission when it acts, never by a snapshot.

use crate::actor::ActorId;
use crate::confidentiality::Classification;
use crate::privacy::PrivacyConfig;
use crate::session::{ChannelId, SessionId};

/// A session with its participants and the channels the viewer may read.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSnapshot {
    pub id: SessionId,
    pub name: String,
    /// Whom the session lets read what was said in it (issue 134) - shown
    /// to everyone who takes part.
    pub privacy: PrivacyConfig,
    /// Whether the viewer may change the session - rename it, change its
    /// privacy (`UpdateSession`).
    pub can_change: bool,
    /// Everyone who takes part in the session - whom a new channel can be
    /// made of (issue 09).
    pub participants: Vec<ParticipantSnapshot>,
    /// The channels the viewer may read, in the order of the session.
    pub channels: Vec<ChannelSnapshot>,
}

/// A participant of a session.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticipantSnapshot {
    pub id: ActorId,
    /// `None` for a user or agent that no longer exists.
    pub name: Option<String>,
}

/// A channel as the viewer sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelSnapshot {
    pub id: ChannelId,
    pub name: String,
    /// The kind of moderator, e.g. `round_robin` (issue 44).
    pub moderator: String,
    /// The classification of the channel's contents (issue 133).
    pub classification: Classification,
    /// Whether the viewer may rename the channel and add or remove its
    /// participants (`UpdateChannel`, issue 46).
    pub can_change: bool,
    /// Its participants, in the order of the channel.
    pub members: Vec<MemberSnapshot>,
}

/// A participant of a channel as the viewer sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberSnapshot {
    pub id: ActorId,
    /// `None` for a user or agent that no longer exists.
    pub name: Option<String>,
    /// The handle the participant is `@`-mentioned by (see
    /// `crate::mention`), or `None` if it cannot be mentioned - approvers,
    /// participants that no longer exist, names that yield no handle.
    pub handle: Option<String>,
    /// Whether the viewer may see what this participant saw when it produced
    /// a message (`ReadAgentContext`, issue 41) - only ever for agents.
    pub can_read_context: bool,
}
