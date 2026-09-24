use crate::actor::ActorId;
use crate::session::SessionId;
use crate::channel::ChannelId;
use crate::tool::ToolCall;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

/// Everything the `SecurityManager` is asked about.
///
/// An action on a stored entity carries that entity's `SecurityAttributes`,
/// so that a policy can decide by owner and contributors (issue 11) without
/// being handed the entity itself: `Get`, `Update` and `Delete` carry the
/// attributes as stored - never those of a new value, which the caller
/// controls - and `Create` those of the entity to be created. `Find` and
/// `List` ask whether the caller may search at all; the result is then
/// filtered with `Get` for every entity in it.
#[derive(Debug, Clone, Copy)]
pub enum ActionType<'a> {
    GetPersonality(&'a SecurityAttributes),
    FindPersonality,
    ListPersonalities,
    CreatePersonality(&'a SecurityAttributes),
    UpdatePersonality(&'a SecurityAttributes),
    DeletePersonality(&'a SecurityAttributes),
    GetSessionTemplate(&'a SecurityAttributes),
    FindSessionTemplate,
    ListSessionTemplates,
    CreateSessionTemplate(&'a SecurityAttributes),
    UpdateSessionTemplate(&'a SecurityAttributes),
    DeleteSessionTemplate(&'a SecurityAttributes),
    CreateAgent(&'a SecurityAttributes),
    GetAgent(&'a SecurityAttributes),
    FindAgent,
    UpdateAgent(&'a SecurityAttributes),
    CreateSession(&'a SecurityAttributes),
    GetSession(&'a SecurityAttributes),
    UpdateSession(&'a SecurityAttributes),
    DeleteSession(&'a SecurityAttributes),
    ListSessions,
    /// Ending the current turn of every agent in a channel, carrying the
    /// channel's attributes.
    CancelChannel(&'a SecurityAttributes),
    /// Waking a channel's sleeping agents deliberately, as opposed to waking
    /// them by sending them something.
    WakeChannel(&'a SecurityAttributes),
    /// Seeing that a user command exists - in `/help` and in completion.
    /// Keyed by the command's name, without the slash (issue 28).
    ShowCommand(&'a str),
    /// Running a user command, keyed by its name.
    ExecuteCommand(&'a str),
    CallTool(&'a ToolCall),
    ReadSecret(&'a str),
    GetComponent(&'a SecurityAttributes),
    ListComponents,
    CreateComponent(&'a SecurityAttributes),
    UpdateComponent(&'a SecurityAttributes),
    DeleteComponent(&'a SecurityAttributes),
}

/// Who an entity belongs to - what a policy decides by (issue 11).
///
/// The owner may do everything with it; contributors may read and use it -
/// the participants of a session, say - but not change or delete it. An
/// entity without an owner (`ActorId::None`, as in anything stored before
/// owners were recorded) belongs to the system. Richer access lists are
/// issue 23.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SecurityAttributes {
    pub owner_id: ActorId,
    #[serde(default)]
    pub contributors: Vec<ActorId>,
}

impl SecurityAttributes {
    /// Attributes of an entity owned by `owner`, without contributors.
    pub fn owned_by(owner: ActorId) -> Self {
        Self { owner_id: owner, contributors: Vec::new() }
    }
}

/// An entity that carries `SecurityAttributes`.
pub trait Secured {
    fn security_attributes(&self) -> &SecurityAttributes;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityContext {
    pub initiator_id: ActorId,
    pub caller_id: ActorId,
    pub delegation_ids: Vec<ActorId>,
    pub session_id: SessionId,
    pub channel_id: ChannelId
}

impl SecurityContext {
    pub fn new(initiator: ActorId, session_id: SessionId, channel_id: ChannelId) -> Self {
        Self {
            initiator_id: initiator,
            caller_id: initiator,
            delegation_ids: vec![initiator],
            session_id,
            channel_id
        }
    }
    pub fn next(&self, caller_id: ActorId) -> Self {
        let mut delegation_ids = self.delegation_ids.clone();
        delegation_ids.push(caller_id);

        Self {
            initiator_id: self.initiator_id,
            caller_id,
            delegation_ids,
            session_id: self.session_id,
            channel_id: self.channel_id
        }
    }
}

#[async_trait]
pub trait SecurityManager : Debug + Sync + Send {
    async fn check(&self, context: &SecurityContext, action: ActionType<'_>) -> bool;
}

#[derive(Debug)]
pub struct DummySecurityManager {
}

impl DummySecurityManager {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl SecurityManager for DummySecurityManager {
    async fn check(&self, _context: &SecurityContext, _action: ActionType<'_>) -> bool {
        true
    }
}
