use crate::actor::ActorId;
use crate::session::SessionId;
use crate::channel::ChannelId;
use crate::confidentiality::{ChannelClassification, Classification, Compartments, Confidentiality};
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
    /// Stopping a running session - all its channels and actors - without
    /// deleting it, carrying its attributes. It stops for everybody in it.
    StopSession(&'a SecurityAttributes),
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
    /// a participant is cleared for with the channel's classification
    /// (issue 133).
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
    /// Reading what is classified `classification` - a message, a note, a
    /// memory entry - of the channel `source` from another channel
    /// (`reader`), issue 17. Decided per entry by where it is and how it is
    /// classified (issue 133), not by who takes part in the channels, so
    /// that the answer does not change when someone joins or leaves. An
    /// entry released below its channel's classification may be read where
    /// the channel as a whole may not.
    ReadChannel { source: &'a ChannelClassification, classification: &'a Classification, reader: &'a ChannelClassification },
    /// `actor` putting what is classified `classification` into `channel`
    /// (issue 133): a message or a tool result into its history - asked by
    /// the channel for every one that enters it - or a tool result into the
    /// context of an agent in it, asked before the agent's model sees it. A
    /// classification other than the channel's releases a message to a
    /// wider audience or a lower level.
    Publish { classification: &'a Classification, actor: ActorId, channel: &'a ChannelClassification },
    /// Giving something a classification - a channel when it is created
    /// (issue 133). Where a policy refuses compartments that are not
    /// configured.
    Classify(&'a Classification),
    /// Changing the classification of `target` from `current` to `new`
    /// (issue 133) - lowering it releases what it holds to a wider circle,
    /// raising it retracts nothing already read. `channel` is the channel
    /// it belongs to, as it is classified now, with the channel's
    /// attributes: who owns the channel, and so what is in it.
    Reclassify { target: Reclassified, current: &'a Classification, new: &'a Classification, channel: &'a ChannelClassification, attributes: &'a SecurityAttributes },
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
            ActionType::StopSession(_) => "StopSession".into(),
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
            ActionType::ReadChannel { source, classification, reader } => format!("ReadChannel({classification} of {} from {})", source.channel_id, reader.channel_id),
            ActionType::Publish { classification, actor, channel } => format!("Publish({classification} by {actor} to {})", channel.channel_id),
            ActionType::Classify(classification) => format!("Classify({classification})"),
            ActionType::Reclassify { target, current, new, channel, .. } => format!("Reclassify({target:?} of {} from {current} to {new})", channel.channel_id),
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

/// What is reclassified (`ActionType::Reclassify`, issue 133).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reclassified {
    /// The channel itself.
    Channel,
    /// A message of the channel, written by `author`.
    Message { author: ActorId },
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
///
/// What a client is shown or offered - may it change a channel, read an
/// agent's context - comes from a view (`crate::view`, issue 80) that the
/// runtime builds with these questions; clients do not ask them themselves.
#[async_trait]
pub trait AccessQuestions {
    /// Whether `context` may read the channel with `channel` attributes -
    /// its members, its history, its events (`GetChannel`).
    async fn may_read_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool;

    /// Whether `context` may change the channel with `channel` attributes -
    /// rename it, add and remove participants (`UpdateChannel`).
    async fn may_change_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool;

    /// Whether the channel `reader` may read what comes from the channel
    /// `source` classified `classification` - a message, a note, or what an
    /// agent remembered there (`ReadChannel`, issues 17, 131 and 133): asked
    /// in `context`, the agent's who reads and passes it on. A channel may
    /// always read itself.
    async fn may_channel_read(&self, context: &SecurityContext, reader: &ChannelClassification, source: &ChannelClassification, classification: &Classification) -> bool;

    /// Whether `context` may see what the agent with `agent` attributes saw
    /// when it wrote a message (`ReadAgentContext`, issue 41).
    async fn may_read_agent_context(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool;

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

    async fn may_channel_read(&self, context: &SecurityContext, reader: &ChannelClassification, source: &ChannelClassification, classification: &Classification) -> bool {
        if reader.channel_id == source.channel_id {
            return true;
        }
        self.check(context, ActionType::ReadChannel { source, classification, reader }).await
    }

    async fn may_read_agent_context(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool {
        self.check(context, ActionType::ReadAgentContext(agent)).await
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
/// - Changing, deleting and stopping are the owner's alone, and so is reading what an
///   agent saw: its context holds more than the conversation - its system
///   prompt, internal steps, tool results.
/// - Cancelling or waking a channel is allowed to whoever may read it.
/// - Reading from another channel follows the flow rule
///   (`Classification::flows_to`, issue 133) with the configured
///   compartments: what is read must flow into the reading channel. Until
///   reading across sessions is a question of consent (issue 134), a channel
///   reads only from channels of its own session, and from public ones.
/// - A classification may be given if its compartments are configured. A
///   message may be put into a channel if, moreover, it flows into the
///   channel and the actor putting it there is the one acting - its author,
///   who may release what they write.
/// - A channel may be reclassified by its owner, a message by the channel's
///   owner or its author - never above the channel - and only with
///   configured compartments.
///   That these rules live in the ownership policy is issue 138.
/// - Searching and listing are allowed; their results are filtered by what
///   the caller may read. Tool calls, secrets and commands are governed
///   elsewhere for now (approvals, issues 12 and 23).
///
/// Who "you" are is the context's initiator: the person an action was
/// started for, also when an agent carries it out.
#[derive(Debug, Default)]
pub struct OwnershipSecurityManager {
    compartments: Compartments,
}

impl OwnershipSecurityManager {
    /// The policy without compartments.
    pub fn new() -> Self {
        Self::default()
    }

    /// The policy with the configured `compartments`.
    pub fn with_compartments(compartments: Compartments) -> Self {
        Self { compartments }
    }

    /// Whether `reader` may read what `source` holds classified
    /// `classification` (`ReadChannel`).
    fn may_read_from(&self, source: &ChannelClassification, classification: &Classification, reader: &ChannelClassification) -> bool {
        let reachable = source.session_id == reader.session_id || source.classification.level() == Confidentiality::Public;
        reachable && classification.flows_to(&reader.classification, &self.compartments)
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
            ActionType::ReadChannel { source, classification, reader } => {
                source.channel_id == reader.channel_id || self.may_read_from(source, classification, reader)
            }
            ActionType::Classify(classification) => self.compartments.check(classification).is_ok(),
            ActionType::Reclassify { target, new, channel, attributes, .. } => {
                let owner = attributes.owner_id == me && me != ActorId::None;
                self.compartments.check(new).is_ok() && match target {
                    Reclassified::Channel => owner,
                    Reclassified::Message { author } => {
                        (owner || author == context.caller_id) && new.flows_to(&channel.classification, &self.compartments)
                    }
                }
            }
            ActionType::Publish { classification, actor, channel } => {
                actor == context.caller_id
                    && self.compartments.check(classification).is_ok()
                    && classification.flows_to(&channel.classification, &self.compartments)
            }
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
            | ActionType::StopSession(a)
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
    use crate::confidentiality::Compartment;
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
        assert!(allows(alice, ActionType::StopSession(&hers)).await, "the owner stops a session");
        assert!(!allows(bob, ActionType::StopSession(&hers)).await, "a participant does not");

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
        let (own, other) = (channel(session_id, Confidentiality::Confidential, &[]), channel(session_id, Confidentiality::Public, &[]));
        assert!(DenyAll.may_channel_read(&context(alice), &own, &own, &own.classification).await, "a channel reads itself, whatever the policy");
        assert!(!DenyAll.may_channel_read(&context(alice), &own, &other, &other.classification).await, "another channel only as the policy decides");
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

    fn channel(session_id: SessionId, level: Confidentiality, compartments: &[&str]) -> ChannelClassification {
        let classification = Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap();
        ChannelClassification { session_id, channel_id: ChannelId::new(), classification }
    }

    /// Reading what `source` holds at its own classification.
    fn read<'a>(source: &'a ChannelClassification, reader: &'a ChannelClassification) -> ActionType<'a> {
        ActionType::ReadChannel { source, classification: &source.classification, reader }
    }

    fn hr_reads_finance() -> OwnershipSecurityManager {
        let compartments = Compartments::new([
            (Compartment::new("hr"), vec![Compartment::new("finance")]),
            (Compartment::new("finance"), vec![]),
        ]);
        OwnershipSecurityManager::with_compartments(compartments.unwrap())
    }

    #[tokio::test]
    async fn reading_a_channel_depends_on_the_channels_not_on_the_reader() {
        let session = SessionId::new();
        let (team, secret) = (channel(session, Confidentiality::Internal, &[]), channel(session, Confidentiality::Confidential, &[]));
        for actor in [user(), user()] {
            assert!(allows(actor, read(&team, &secret)).await);
            assert!(!allows(actor, read(&secret, &team)).await);
        }
    }

    #[tokio::test]
    async fn reading_a_channel_follows_the_flow_rule_with_the_configured_compartments() {
        let (policy, session, alice) = (hr_reads_finance(), SessionId::new(), context(user()));
        let (hr, finance, general) = (
            channel(session, Confidentiality::Internal, &["hr"]),
            channel(session, Confidentiality::Internal, &["finance"]),
            channel(session, Confidentiality::Internal, &[]),
        );
        assert!(policy.check(&alice, read(&finance, &hr)).await, "HR reads finance");
        assert!(!policy.check(&alice, read(&hr, &finance)).await, "not the other way round");
        assert!(!policy.check(&alice, read(&hr, &general)).await, "nor a general channel");
        assert!(policy.check(&alice, read(&general, &hr)).await, "what is general flows anywhere");
    }

    #[tokio::test]
    async fn until_consent_is_modelled_a_channel_reads_other_sessions_only_where_they_are_public() {
        let (here, elsewhere) = (SessionId::new(), SessionId::new());
        let reader = channel(here, Confidentiality::Internal, &[]);
        let alice = user();
        assert!(!allows(alice, read(&channel(elsewhere, Confidentiality::Internal, &[]), &reader)).await);
        assert!(allows(alice, read(&channel(elsewhere, Confidentiality::Public, &[]), &reader)).await);
    }

    #[tokio::test]
    async fn what_is_released_below_its_channel_is_read_where_the_channel_is_not() {
        let (policy, session, alice) = (hr_reads_finance(), SessionId::new(), context(user()));
        let (hr, finance) = (channel(session, Confidentiality::Confidential, &["hr"]), channel(session, Confidentiality::Confidential, &["finance"]));
        let released = Classification::new(Confidentiality::Confidential, [Compartment::new("hr"), Compartment::new("finance")]).unwrap();
        assert!(!policy.check(&alice, read(&hr, &finance)).await, "finance does not read HR");
        assert!(policy.check(&alice, ActionType::ReadChannel { source: &hr, classification: &released, reader: &finance }).await, "but what HR released to it");
        let elsewhere = channel(SessionId::new(), Confidentiality::Confidential, &["finance"]);
        assert!(!policy.check(&alice, ActionType::ReadChannel { source: &hr, classification: &released, reader: &elsewhere }).await, "not from another session");
    }

    #[tokio::test]
    async fn an_author_puts_into_a_channel_what_may_flow_into_it() {
        let (policy, session) = (hr_reads_finance(), SessionId::new());
        let alice = user();
        let hr = channel(session, Confidentiality::Confidential, &["hr"]);
        let classified = |level, c: &[&str]| Classification::new(level, c.iter().map(|c| Compartment::new(*c))).unwrap();
        let (policy, hr) = (&policy, &hr);
        let publish = |classification| async move {
            policy.check(&context(alice), ActionType::Publish { classification: &classification, actor: alice, channel: hr }).await
        };
        assert!(publish(classified(Confidentiality::Confidential, &["hr"])).await, "the channel's own");
        assert!(publish(classified(Confidentiality::Internal, &["hr", "finance"])).await, "released to finance, at a lower level");
        assert!(!publish(classified(Confidentiality::StrictlyConfidential, &["hr"])).await, "above the channel");
        assert!(publish(classified(Confidentiality::Confidential, &["finance"])).await, "HR may read what is finance's");
        assert!(publish(Classification::general(Confidentiality::Confidential)).await, "what is general flows into any channel");
        assert!(!publish(classified(Confidentiality::Confidential, &["hr", "payroll"])).await, "not configured");
        let bob = user();
        let own = classified(Confidentiality::Confidential, &["hr"]);
        assert!(!policy.check(&context(bob), ActionType::Publish { classification: &own, actor: alice, channel: hr }).await, "only in one's own name");
    }

    #[tokio::test]
    async fn a_channel_is_reclassified_by_its_owner_a_message_also_by_its_author() {
        let policy = hr_reads_finance();
        let (owner, author, stranger) = (user(), user(), user());
        let attributes = SecurityAttributes { owner_id: owner, contributors: vec![author, stranger] };
        let hr = channel(SessionId::new(), Confidentiality::Confidential, &["hr"]);
        let classified = |level, c: &[&str]| Classification::new(level, c.iter().map(|c| Compartment::new(*c))).unwrap();
        let current = hr.classification.clone();
        let reclassify = |who: ActorId, target: Reclassified, new: Classification| {
            let (policy, hr, attributes, current) = (&policy, &hr, &attributes, &current);
            async move {
                policy.check(&context(who), ActionType::Reclassify { target, current, new: &new, channel: hr, attributes }).await
            }
        };
        let internal = classified(Confidentiality::Internal, &["hr"]);
        assert!(reclassify(owner, Reclassified::Channel, internal.clone()).await, "the owner releases the channel");
        assert!(!reclassify(author, Reclassified::Channel, internal.clone()).await, "a participant does not");
        assert!(!reclassify(owner, Reclassified::Channel, classified(Confidentiality::Internal, &["payroll"])).await, "not configured");

        let message = Reclassified::Message { author };
        let released = classified(Confidentiality::Internal, &["hr", "finance"]);
        assert!(reclassify(author, message, released.clone()).await, "the author releases their message");
        assert!(reclassify(owner, message, released.clone()).await, "so does the channel's owner");
        assert!(!reclassify(stranger, message, released).await, "nobody else");
        assert!(!reclassify(author, message, classified(Confidentiality::StrictlyConfidential, &["hr"])).await, "never above the channel");
    }

    #[tokio::test]
    async fn only_configured_compartments_may_be_given() {
        let (policy, alice) = (hr_reads_finance(), context(user()));
        let classify = |c: &[&str]| Classification::new(Confidentiality::Internal, c.iter().map(|c| Compartment::new(*c))).unwrap();
        assert!(policy.check(&alice, ActionType::Classify(&classify(&["hr", "finance"]))).await);
        assert!(!policy.check(&alice, ActionType::Classify(&classify(&["hrr"]))).await, "a typo");
        assert!(policy.check(&alice, ActionType::Classify(&classify(&[]))).await, "general needs none");
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
            visibility: Default::default(),
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
