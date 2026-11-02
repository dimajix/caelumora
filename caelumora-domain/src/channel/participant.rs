//! Who is in a session or a channel, by reference: those who take part (`Participant`, issue 120),
//! the actors that belong to a channel itself without taking part (`InternalActor`, issue 420),
//! and those that left it (`FormerParticipant`, issue 164).

use crate::actor::ActorId;
use crate::agent::AgentId;
use crate::approval::Approver;
use crate::removal::Removal;
use crate::user::UserId;
use serde::{Deserialize, Serialize};

/// A participant that left a channel (issue 164): removed, by whom and
/// when. What it saw of the history is the history's to say (issue 417).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormerParticipant {
    pub participant: Participant,
    pub left: Removal,
}

/// Who takes part in a session or channel: users and agents, by reference.
/// Their records live in the `EntityStore` and are read from there, so
/// that no session keeps a copy that goes stale or shows more of them than
/// their name (issue 120).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Participant {
    User(UserId),
    Agent(AgentId),
}

impl Participant {
    pub fn actor_id(&self) -> ActorId {
        match self {
            Participant::User(id) => ActorId::User(*id),
            Participant::Agent(id) => ActorId::Agent(*id),
        }
    }
}

/// An actor that belongs to a channel itself, as its moderator does (issue
/// 420): none of its participants, nor in their group. It is never added,
/// removed or announced, and has no range of the history - what is recorded
/// to it goes to it by name. Otherwise it is an actor like any other: the
/// channel starts it with the others, delivers to it by the one rule
/// (`Envelope::concerns`) and takes in what it sends. It has no record of
/// its own and is kept here whole.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InternalActor {
    /// The approver, which collects the users' decisions on the tool calls
    /// that need approval and decides by its strategy (see APPROVALS.md).
    Approver(Approver),
}

impl InternalActor {
    pub fn actor_id(&self) -> ActorId {
        match self {
            InternalActor::Approver(approver) => ActorId::Approver(approver.id),
        }
    }
}
