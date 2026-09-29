use crate::actor::ActorId;
use crate::session::SessionId;
use crate::channel::ChannelId;
use crate::confidentiality::{ChannelClassification, Confidentiality};
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
    ListPersonalities,
    CreatePersonality(&'a SecurityAttributes),
    UpdatePersonality(&'a SecurityAttributes),
    DeletePersonality(&'a SecurityAttributes),
    GetSessionTemplate(&'a SecurityAttributes),
    ListSessionTemplates,
    CreateSessionTemplate(&'a SecurityAttributes),
    UpdateSessionTemplate(&'a SecurityAttributes),
    DeleteSessionTemplate(&'a SecurityAttributes),
    CreateAgent(&'a SecurityAttributes),
    GetAgent(&'a SecurityAttributes),
    /// Reading what an agent's model saw when it produced a message - its
    /// system prompt, internal messages and tool results included (issue
    /// 06). Carries the agent's attributes.
    ReadAgentContext(&'a SecurityAttributes),
    ListAgents,
    UpdateAgent(&'a SecurityAttributes),
    DeleteAgent(&'a SecurityAttributes),
    CreateSession(&'a SecurityAttributes),
    GetSession(&'a SecurityAttributes),
    UpdateSession(&'a SecurityAttributes),
    DeleteSession(&'a SecurityAttributes),
    ListSessions,
    /// Reading a channel, carrying its attributes (issue 70).
    GetChannel(&'a SecurityAttributes),
    ListChannels,
    /// Creating a channel, with the attributes of the new channel.
    CreateChannel(&'a SecurityAttributes),
    /// Changing a channel - its name, its participants - with its attributes
    /// as stored.
    UpdateChannel(&'a SecurityAttributes),
    /// Deleting a channel, with its attributes as stored.
    DeleteChannel(&'a SecurityAttributes),
    /// `participant` joining `channel` (issue 46) - besides `UpdateChannel`,
    /// which decides who may add participants. Where a policy compares what
    /// a participant is cleared for with the channel's level (issue 64).
    JoinChannel { participant: ActorId, channel: &'a ChannelClassification },
    /// Reading a user, e.g. receiving the events about them. A stored user
    /// belongs to the system (issue 94): see `User`'s `Secured`.
    GetUser(&'a SecurityAttributes),
    ListUsers,
    CreateUser(&'a SecurityAttributes),
    UpdateUser(&'a SecurityAttributes),
    DeleteUser(&'a SecurityAttributes),
    /// Ending the current turn of every agent in a channel, carrying the
    /// channel's attributes.
    CancelChannel(&'a SecurityAttributes),
    /// Waking a channel's sleeping agents deliberately, as opposed to waking
    /// them by sending them something.
    WakeChannel(&'a SecurityAttributes),
    /// Reading a channel's notes or history (`source`) from another channel
    /// (`reader`), issue 17. Decided by the channels alone - their places and
    /// confidentiality levels (issue 61) - not by who takes part in them, so
    /// that the answer does not change when someone joins or leaves.
    ReadChannel { source: &'a ChannelClassification, reader: &'a ChannelClassification },
    /// Publishing a part of a message labelled `label` to `channel`, as
    /// `actor` (issue 61) - reasoning, a tool call or its content. Decided by
    /// the label, the actor and the channel, not by who takes part in the
    /// channel. Never asked for `Confidentiality::Private`, which does not
    /// leave the actor whatever a policy says.
    Publish { label: Confidentiality, actor: ActorId, channel: &'a ChannelClassification },
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

impl ActionType<'_> {
    /// The action for the log: its kind and what identifies what it is
    /// about, never content - a tool call by its function, not its
    /// arguments; a secret by its name, not its value.
    pub fn describe(&self) -> String {
        match self {
            ActionType::GetPersonality(_) => "GetPersonality".into(),
            ActionType::ListPersonalities => "ListPersonalities".into(),
            ActionType::CreatePersonality(_) => "CreatePersonality".into(),
            ActionType::UpdatePersonality(_) => "UpdatePersonality".into(),
            ActionType::DeletePersonality(_) => "DeletePersonality".into(),
            ActionType::GetSessionTemplate(_) => "GetSessionTemplate".into(),
            ActionType::ListSessionTemplates => "ListSessionTemplates".into(),
            ActionType::CreateSessionTemplate(_) => "CreateSessionTemplate".into(),
            ActionType::UpdateSessionTemplate(_) => "UpdateSessionTemplate".into(),
            ActionType::DeleteSessionTemplate(_) => "DeleteSessionTemplate".into(),
            ActionType::CreateAgent(_) => "CreateAgent".into(),
            ActionType::GetAgent(_) => "GetAgent".into(),
            ActionType::ReadAgentContext(_) => "ReadAgentContext".into(),
            ActionType::ListAgents => "ListAgents".into(),
            ActionType::UpdateAgent(_) => "UpdateAgent".into(),
            ActionType::DeleteAgent(_) => "DeleteAgent".into(),
            ActionType::CreateSession(_) => "CreateSession".into(),
            ActionType::GetSession(_) => "GetSession".into(),
            ActionType::UpdateSession(_) => "UpdateSession".into(),
            ActionType::DeleteSession(_) => "DeleteSession".into(),
            ActionType::ListSessions => "ListSessions".into(),
            ActionType::GetChannel(_) => "GetChannel".into(),
            ActionType::ListChannels => "ListChannels".into(),
            ActionType::CreateChannel(_) => "CreateChannel".into(),
            ActionType::UpdateChannel(_) => "UpdateChannel".into(),
            ActionType::DeleteChannel(_) => "DeleteChannel".into(),
            ActionType::JoinChannel { participant, channel } => format!("JoinChannel({participant} to {})", channel.channel_id),
            ActionType::GetUser(_) => "GetUser".into(),
            ActionType::ListUsers => "ListUsers".into(),
            ActionType::CreateUser(_) => "CreateUser".into(),
            ActionType::UpdateUser(_) => "UpdateUser".into(),
            ActionType::DeleteUser(_) => "DeleteUser".into(),
            ActionType::CancelChannel(_) => "CancelChannel".into(),
            ActionType::WakeChannel(_) => "WakeChannel".into(),
            ActionType::ReadChannel { source, reader } => format!("ReadChannel({} from {})", source.channel_id, reader.channel_id),
            ActionType::Publish { label, actor, channel } => format!("Publish({label:?} by {actor} to {})", channel.channel_id),
            ActionType::ShowCommand(name) => format!("ShowCommand(/{name})"),
            ActionType::ExecuteCommand(name) => format!("ExecuteCommand(/{name})"),
            ActionType::CallTool(call) => format!("CallTool({})", call.function_name),
            ActionType::ReadSecret(key) => format!("ReadSecret({key})"),
            ActionType::GetComponent(_) => "GetComponent".into(),
            ActionType::ListComponents => "ListComponents".into(),
            ActionType::CreateComponent(_) => "CreateComponent".into(),
            ActionType::UpdateComponent(_) => "UpdateComponent".into(),
            ActionType::DeleteComponent(_) => "DeleteComponent".into(),
        }
    }
}

/// Who an entity belongs to - what a policy decides by (issue 11).
///
/// The owner may do everything with it; contributors may read and use it -
/// the participants of a session, say - but not change or delete it. An
/// entity without an owner (`ActorId::None`, as in the demo data of a seed
/// file) belongs to the system. Richer access lists are
/// issue 23.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SecurityAttributes {
    pub owner_id: ActorId,
    /// May be left out where the attributes are written by hand - a seed
    /// file (issue 108).
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

/// The pure questions "may they?" that are asked from outside an entity -
/// by clients deciding what to offer, by readers deciding what to show -
/// each with its name, in one place (issue 71). They only translate a
/// question into an `ActionType`: the decision is `SecurityManager::check`'s
/// alone, and since every `SecurityManager` gets them by the blanket
/// implementation, no policy can answer them differently.
///
/// Operations that check their permission as part of what they do (a
/// channel's `cancel`, a store's `update_channel`) keep their check; a pure
/// question goes here, not onto the entity.
#[async_trait]
pub trait AccessQuestions {
    /// Whether `context` may read the channel with `channel` attributes -
    /// its members, its history, its events (`GetChannel`).
    async fn may_read_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool;

    /// Whether `context` may change the channel with `channel` attributes -
    /// rename it, add and remove participants (`UpdateChannel`).
    async fn may_change_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool;

    /// Whether the channel `reader` may read the notes and history of the
    /// channel `source` (`ReadChannel`, issues 17 and 61): asked in the name
    /// of `reader_owner`, who is responsible for what is read into their
    /// channel. A channel may always read itself.
    async fn may_channel_read(&self, reader: &ChannelClassification, reader_owner: ActorId, source: &ChannelClassification) -> bool;

    /// Whether `context` may see what the agent with `agent` attributes saw
    /// when it wrote a message (`ReadAgentContext`, issue 41).
    async fn may_read_agent_context(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool;

    /// Whether `actor` may publish what is labelled `label` to `channel`,
    /// asked in `context` (`Publish`, issue 61).
    async fn may_publish(&self, context: &SecurityContext, label: Confidentiality, actor: ActorId, channel: &ChannelClassification) -> bool;

    /// Whether `context` may make the tool call `call` (`CallTool`) - asked
    /// before a call is put up for approval.
    async fn may_call_tool(&self, context: &SecurityContext, call: &ToolCall) -> bool;
}

#[async_trait]
impl<T: SecurityManager + ?Sized> AccessQuestions for T {
    async fn may_read_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::GetChannel(channel)).await
    }

    async fn may_change_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::UpdateChannel(channel)).await
    }

    async fn may_channel_read(&self, reader: &ChannelClassification, reader_owner: ActorId, source: &ChannelClassification) -> bool {
        if reader.channel_id == source.channel_id {
            return true;
        }
        let context = SecurityContext::new(reader_owner, reader.session_id, reader.channel_id);
        self.check(&context, ActionType::ReadChannel { source, reader }).await
    }

    async fn may_read_agent_context(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool {
        self.check(context, ActionType::ReadAgentContext(agent)).await
    }

    async fn may_publish(&self, context: &SecurityContext, label: Confidentiality, actor: ActorId, channel: &ChannelClassification) -> bool {
        self.check(context, ActionType::Publish { label, actor, channel }).await
    }

    async fn may_call_tool(&self, context: &SecurityContext, call: &ToolCall) -> bool {
        self.check(context, ActionType::CallTool(call)).await
    }
}

/// The one way an operation checks its own permission (issue 95): asked as
/// part of what it does, a refusal ends it with the operation's own error,
/// and every refusal is logged alike - which action, for whom, at `debug!`:
/// a refusal is detail of one operation, and some are routine, such as a
/// store answering "not found" for what its caller may not see. Pure
/// questions, which only decide what to offer, show or filter, are
/// `AccessQuestions`. By blanket implementation on every `SecurityManager`,
/// like those.
#[async_trait]
pub trait Enforcement {
    /// Nothing if `context` may do `action`; `refusal()` - the caller's
    /// error, which may deliberately say less, such as "not found" - if not.
    async fn require<E, F>(&self, context: &SecurityContext, action: ActionType<'_>, refusal: F) -> Result<(), E>
    where
        E: Send,
        F: FnOnce() -> E + Send;
}

#[async_trait]
impl<T: SecurityManager + ?Sized> Enforcement for T {
    async fn require<E, F>(&self, context: &SecurityContext, action: ActionType<'_>, refusal: F) -> Result<(), E>
    where
        E: Send,
        F: FnOnce() -> E + Send,
    {
        if self.check(context, action).await {
            return Ok(());
        }
        tracing::debug!("Refused {} for {}", action.describe(), context.initiator_id);
        Err(refusal())
    }
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
/// - Changing and deleting are the owner's alone, and so is reading what an
///   agent saw: its context holds more than the conversation - its system
///   prompt, internal steps, tool results.
/// - Cancelling or waking a channel is allowed to whoever may read it.
/// - Reading one channel from another follows the confidentiality levels of
///   the two (`ChannelClassification::readable_from`, issue 61), and so does
///   publishing a labelled part of a message to a channel
///   (`ChannelClassification::admits`).
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
            | ActionType::GetChannel(a)
            | ActionType::GetComponent(a)
            | ActionType::GetUser(a)
            | ActionType::CancelChannel(a)
            | ActionType::WakeChannel(a) => Self::may_read(a, me),
            ActionType::ReadChannel { source, reader } => source.readable_from(reader),
            ActionType::Publish { label, channel, .. } => channel.admits(label),
            // Clearances are not modelled yet: whoever may change the
            // channel may add anyone of the session to it.
            ActionType::JoinChannel { .. } => true,
            ActionType::CreatePersonality(a)
            | ActionType::CreateSessionTemplate(a)
            | ActionType::CreateAgent(a)
            | ActionType::CreateSession(a)
            | ActionType::CreateChannel(a)
            | ActionType::CreateUser(a)
            | ActionType::CreateComponent(a) => a.owner_id == me && me != ActorId::None,
            ActionType::UpdatePersonality(a)
            | ActionType::DeletePersonality(a)
            | ActionType::UpdateSessionTemplate(a)
            | ActionType::DeleteSessionTemplate(a)
            | ActionType::UpdateAgent(a)
            | ActionType::DeleteAgent(a)
            | ActionType::ReadAgentContext(a)
            | ActionType::UpdateSession(a)
            | ActionType::DeleteSession(a)
            | ActionType::UpdateChannel(a)
            | ActionType::DeleteChannel(a)
            | ActionType::UpdateUser(a)
            | ActionType::DeleteUser(a)
            | ActionType::UpdateComponent(a)
            | ActionType::DeleteComponent(a) => a.owner_id == me && me != ActorId::None,
            ActionType::ListPersonalities
            | ActionType::ListSessionTemplates
            | ActionType::ListAgents
            | ActionType::ListSessions
            | ActionType::ListChannels
            | ActionType::ListUsers
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
    async fn access_questions_ask_the_policy_and_a_channel_always_reads_itself() {
        let (alice, bob) = (user(), user());
        let hers = SecurityAttributes { owner_id: alice, contributors: vec![bob] };
        let policy = OwnershipSecurityManager::new();
        assert!(policy.may_read_channel(&context(bob), &hers).await, "a participant reads the channel");
        assert!(policy.may_change_channel(&context(alice), &hers).await, "the owner changes it");
        assert!(!policy.may_change_channel(&context(bob), &hers).await, "a participant does not");
        assert!(!policy.may_read_agent_context(&context(bob), &hers).await, "what an agent saw is its owner's alone");

        /// Refuses everything.
        #[derive(Debug)]
        struct DenyAll;
        #[async_trait]
        impl SecurityManager for DenyAll {
            async fn check(&self, _context: &SecurityContext, _action: ActionType<'_>) -> bool {
                false
            }
        }
        let session_id = SessionId::new();
        let channel = |level| ChannelClassification { session_id, channel_id: ChannelId::new(), level };
        let (own, other) = (channel(Confidentiality::Channel), channel(Confidentiality::Public));
        assert!(DenyAll.may_channel_read(&own, alice, &own).await, "a channel reads itself, whatever the policy");
        assert!(!DenyAll.may_channel_read(&own, alice, &other).await, "another channel only as the policy decides");
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
        assert!(allows(alice, ActionType::ReadAgentContext(&hers)).await);
        assert!(!allows(bob, ActionType::ReadAgentContext(&hers)).await, "what an agent saw is its owner's alone");
        assert!(!allows(bob, ActionType::UpdateSession(&hers)).await);
        assert!(!allows(bob, ActionType::DeleteSession(&hers)).await);
    }

    #[tokio::test]
    async fn reading_a_channel_depends_on_the_channels_not_on_the_reader() {
        use crate::confidentiality::{ChannelClassification, Confidentiality};
        let session = SessionId::new();
        let classification = |level| ChannelClassification { session_id: session, channel_id: ChannelId::new(), level };
        let (team, secret) = (classification(Confidentiality::Session), classification(Confidentiality::Channel));
        for actor in [user(), user()] {
            assert!(allows(actor, ActionType::ReadChannel { source: &team, reader: &secret }).await);
            assert!(!allows(actor, ActionType::ReadChannel { source: &secret, reader: &team }).await);
        }
    }

    #[tokio::test]
    async fn a_part_is_published_where_the_channel_admits_its_label() {
        use crate::confidentiality::{ChannelClassification, Confidentiality};
        let team = ChannelClassification { session_id: SessionId::new(), channel_id: ChannelId::new(), level: Confidentiality::Session };
        let (owner, agent) = (user(), ActorId::Agent(crate::agent::AgentId::new()));
        let publish = |label| ActionType::Publish { label, actor: agent, channel: &team };
        assert!(allows(owner, publish(Confidentiality::Public)).await);
        assert!(allows(owner, publish(Confidentiality::Session)).await);
        assert!(!allows(owner, publish(Confidentiality::Channel)).await);
        assert!(!allows(owner, publish(Confidentiality::Private)).await);
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

    #[test]
    fn an_action_is_described_without_its_content() {
        let call = ToolCall {
            id: None,
            function_name: "web_search".to_string(),
            arguments: serde_json::json!({ "query": "secret plans" }),
            confidentiality: Default::default(),
        };
        let described = ActionType::CallTool(&call).describe();
        assert_eq!(described, "CallTool(web_search)");
        assert!(!described.contains("secret plans"), "no arguments in the log");
        assert_eq!(ActionType::GetSession(&SecurityAttributes::default()).describe(), "GetSession");
    }

    #[tokio::test]
    async fn require_answers_a_refusal_with_the_callers_error() {
        #[derive(Debug, PartialEq)]
        struct Refused;
        let (owner, stranger) = (user(), user());
        let context = |actor| SecurityContext::new(actor, Default::default(), Default::default());
        let hers = SecurityAttributes::owned_by(owner);
        let policy = OwnershipSecurityManager::new();

        assert_eq!(policy.require(&context(owner), ActionType::DeleteSession(&hers), || Refused).await, Ok(()));
        assert_eq!(policy.require(&context(stranger), ActionType::DeleteSession(&hers), || Refused).await, Err(Refused));
    }
}
