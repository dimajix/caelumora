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
    /// Reading a user, e.g. receiving the events about them; carries
    /// attributes owned by the user themselves.
    GetUser(&'a SecurityAttributes),
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
    /// The context of the runtime acting on its own - for work that follows
    /// from an action already permitted, or that no user started (initial
    /// data, records kept in step with events). The ownership policy allows
    /// it everything, so it is used deliberately and never as a fallback.
    pub fn system() -> Self {
        Self::new(ActorId::System, SessionId::default(), ChannelId::default())
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

/// The ownership policy (issue 11): what is yours you may do everything
/// with, what you contribute to you may read and use, what belongs to the
/// system everybody may read and use - and nobody acts in someone else's
/// name.
///
/// - The system (`ActorId::System`) may do everything.
/// - Reading - which also covers using, e.g. starting a session from a
///   template - is allowed to the owner, to contributors, and to everybody
///   for entities owned by the system or by no one.
/// - Creating is allowed only with oneself as owner.
/// - Changing and deleting are the owner's alone.
/// - Cancelling or waking a channel is allowed to whoever may read it.
/// - Searching and listing are allowed; their results are filtered by what
///   the caller may read. Tool calls, secrets and commands are governed
///   elsewhere for now (approvals, issues 12 and 23).
///
/// Who "you" are is the context's initiator: the person an action was
/// started for, also when an agent carries it out.
#[derive(Debug, Default)]
pub struct OwnershipSecurityManager;

impl OwnershipSecurityManager {
    pub fn new() -> Self {
        Self
    }

    fn system_owned(attributes: &SecurityAttributes) -> bool {
        matches!(attributes.owner_id, ActorId::None | ActorId::System)
    }

    fn may_read(attributes: &SecurityAttributes, me: ActorId) -> bool {
        Self::system_owned(attributes) || attributes.owner_id == me || attributes.contributors.contains(&me)
    }
}

#[async_trait]
impl SecurityManager for OwnershipSecurityManager {
    async fn check(&self, context: &SecurityContext, action: ActionType<'_>) -> bool {
        let me = context.initiator_id;
        if me == ActorId::System {
            return true;
        }
        match action {
            ActionType::GetPersonality(a)
            | ActionType::GetSessionTemplate(a)
            | ActionType::GetAgent(a)
            | ActionType::GetSession(a)
            | ActionType::GetComponent(a)
            | ActionType::GetUser(a)
            | ActionType::CancelChannel(a)
            | ActionType::WakeChannel(a) => Self::may_read(a, me),
            ActionType::CreatePersonality(a)
            | ActionType::CreateSessionTemplate(a)
            | ActionType::CreateAgent(a)
            | ActionType::CreateSession(a)
            | ActionType::CreateComponent(a) => a.owner_id == me && me != ActorId::None,
            ActionType::UpdatePersonality(a)
            | ActionType::DeletePersonality(a)
            | ActionType::UpdateSessionTemplate(a)
            | ActionType::DeleteSessionTemplate(a)
            | ActionType::UpdateAgent(a)
            | ActionType::UpdateSession(a)
            | ActionType::DeleteSession(a)
            | ActionType::UpdateComponent(a)
            | ActionType::DeleteComponent(a) => a.owner_id == me && me != ActorId::None,
            ActionType::FindPersonality
            | ActionType::ListPersonalities
            | ActionType::FindSessionTemplate
            | ActionType::ListSessionTemplates
            | ActionType::FindAgent
            | ActionType::ListSessions
            | ActionType::ListComponents => true,
            ActionType::ShowCommand(_)
            | ActionType::ExecuteCommand(_)
            | ActionType::CallTool(_)
            | ActionType::ReadSecret(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user::UserId;

    fn user() -> ActorId {
        ActorId::User(UserId::new())
    }

    fn context(actor: ActorId) -> SecurityContext {
        SecurityContext::new(actor, SessionId::default(), ChannelId::default())
    }

    async fn allows(actor: ActorId, action: ActionType<'_>) -> bool {
        OwnershipSecurityManager::new().check(&context(actor), action).await
    }

    #[tokio::test]
    async fn owners_do_everything_contributors_read_strangers_nothing() {
        let (alice, bob, carol) = (user(), user(), user());
        let hers = SecurityAttributes { owner_id: alice, contributors: vec![bob] };

        for action in [ActionType::GetSession(&hers), ActionType::UpdateSession(&hers), ActionType::DeleteSession(&hers)] {
            assert!(allows(alice, action).await, "{action:?}");
            assert!(!allows(carol, action).await, "{action:?}");
        }
        assert!(allows(bob, ActionType::GetSession(&hers)).await);
        assert!(allows(bob, ActionType::CancelChannel(&hers)).await, "a contributor acts in it");
        assert!(!allows(bob, ActionType::UpdateSession(&hers)).await);
        assert!(!allows(bob, ActionType::DeleteSession(&hers)).await);
    }

    #[tokio::test]
    async fn what_belongs_to_the_system_everybody_uses_and_only_the_system_changes() {
        let alice = user();
        for shared in [SecurityAttributes::default(), SecurityAttributes::owned_by(ActorId::System)] {
            assert!(allows(alice, ActionType::GetSessionTemplate(&shared)).await, "stored without an owner, or the system's");
            assert!(!allows(alice, ActionType::UpdateSessionTemplate(&shared)).await);
            assert!(allows(ActorId::System, ActionType::UpdateSessionTemplate(&shared)).await);
        }
    }

    #[tokio::test]
    async fn nobody_creates_in_someone_elses_name_and_no_one_at_all_creates() {
        let (alice, bob) = (user(), user());
        assert!(allows(alice, ActionType::CreateSession(&SecurityAttributes::owned_by(alice))).await);
        assert!(!allows(bob, ActionType::CreateSession(&SecurityAttributes::owned_by(alice))).await);
        assert!(!allows(ActorId::None, ActionType::CreateSession(&SecurityAttributes::default())).await, "an empty context has no rights");
        assert!(!allows(ActorId::None, ActionType::UpdateSession(&SecurityAttributes::default())).await);
    }
}
