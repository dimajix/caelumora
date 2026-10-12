use crate::actor::ActorId;
use crate::session::SessionId;
use crate::channel::ChannelId;
use crate::command::Command;
use crate::group::GroupId;
use crate::role::RoleId;
use crate::skill;
use crate::store::EntityKind;
use crate::store::query::FieldValue;
use crate::tool::ToolCall;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::sync::Arc;
use crate::agent::Agent;
use crate::artifact::Artifact;
use crate::channel::Channel;
use crate::group::Group;
use crate::session::Session;
use crate::skill::Skill;
use crate::workspace::Workspace;
use crate::removal::Stored;

/// Everything the `SecurityManager` is asked about.
///
/// What is done with a stored entity is asked alike for every kind (issue
/// 323): `Entity`, by the kind, a `Verb` and the entity's
/// `SecurityAttributes`, so that a policy can decide by its owner and its
/// access list (issues 11, 23) without being handed the entity itself, and
/// a new kind brings no new action. `Get`, `Update` and `Delete` carry the
/// attributes as stored - never those of a new value, which the caller
/// controls - and `Create` those of the entity to be created. `List` asks
/// whether the caller may search at all; the result is then filtered with
/// `Get` for every entity in it. The other variants are acts of the core
/// that concern no single stored entity alone.
#[derive(Debug, Clone, Copy)]
pub enum ActionType<'a> {
    /// Doing `verb` with an entity of `kind` that has `attributes`. What
    /// reading covers differs by kind:
    ///
    /// - a workspace (issue 253): the document and its files - listing,
    ///   downloading, the tools that only read (what its access list grants
    ///   `Read`); an agent using it through a channel is bounded by the
    ///   channel besides. Its files are changed by `WRITE`.
    /// - a skill (issue 22): its name, description and body; loading it into
    ///   a turn is `skill::USE`, apart from reading it.
    /// - an artifact (issue 271): the artifact, or a version's content with
    ///   what reads that version - the artifact's attributes with the
    ///   version's own list (`Artifact::version_attributes`); adding a
    ///   version is `WRITE`.
    /// - a group (issue 286): who belongs to it, granted to its owner and,
    ///   by its own access list, to its members. The participants of a
    ///   session or channel are changed only by the runtime.
    /// - a user: receiving the events about them; a stored user belongs to
    ///   the system (issue 94). A profile belongs to its user (issue 176).
    Entity { kind: EntityKind, verb: Verb, attributes: &'a SecurityAttributes },
    /// Listing the entities of a kind.
    List(EntityKind),
    /// Reading what an agent's model saw when it produced a message - its
    /// system prompt, internal messages and tool results included (issue
    /// 06). Carries the agent's attributes.
    ReadAgentContext(&'a SecurityAttributes),
    /// Reading which tools an agent has now in a channel, in their tool
    /// sets (issue 194): what it can do, not what it did. Carries the
    /// agent's attributes and those of the channel, where its tools are
    /// built.
    ReadAgentTools { agent: &'a SecurityAttributes, channel: &'a SecurityAttributes },
    /// Reading which components an agent was built from in a channel, the
    /// preconfigured ones it includes resolved (issue 270): its
    /// configuration, with what was shared with its owner. Carries the
    /// agent's attributes.
    ReadAgentComponents(&'a SecurityAttributes),
    /// Answering whether a tool call of an agent may run (issue 14): an
    /// answer that counts for the approval it was asked for. Carries the
    /// agent's attributes and those of the channel the call is made in.
    ApproveToolCall { agent: &'a SecurityAttributes, channel: &'a SecurityAttributes },
    /// Stopping a running session - all its channels and actors - without
    /// deleting it, carrying its attributes. It stops for everybody in it.
    StopSession(&'a SecurityAttributes),
    /// Ending the current turn of every agent in a channel, carrying the
    /// channel's attributes.
    CancelChannel(&'a SecurityAttributes),
    /// Waking a channel's sleeping agents deliberately, as opposed to waking
    /// them by sending them something.
    WakeChannel(&'a SecurityAttributes),
    /// Asking an agent of a channel to answer again instead of one of its
    /// messages, or of its answer under way (issues 234, 235), carrying the
    /// channel's attributes: an act on the conversation, like cancelling or
    /// waking it, not on the agent.
    RetryMessage(&'a SecurityAttributes),
    /// Answering - or declining to answer - the questions an agent's tool
    /// call asks in the channel with these attributes (issue 165): an act
    /// on the conversation, like asking for another answer. Whether the
    /// one answering was asked is the agent's to check, not the policy's.
    AnswerQuestion(&'a SecurityAttributes),
    /// Modifying a message of `author` in the channel with `channel`
    /// attributes (issue 234): what it says from now on. Asked of the one
    /// who sends the modification, before it is sent and as the channel
    /// admits it; the channel's attributes are there for a policy that lets
    /// someone moderate.
    ModifyMessage { author: ActorId, channel: &'a SecurityAttributes },
    /// Taking a message of `author` back in the channel with `channel`
    /// attributes (issue 234), asked as `ModifyMessage`.
    RevokeMessage { author: ActorId, channel: &'a SecurityAttributes },
    /// Changing the classification of `target` (issue 133): whether the
    /// actor may - its owner, its author. Whether the new classification may
    /// be given there is the `ConfidentialityPolicy`'s, asked beside it
    /// (issue 304).
    Reclassify(Reclassified<'a>),
    /// Seeing that the session's command `command` exists in the channel
    /// with `channel` attributes - in `/help` and in completion (issue 28).
    /// Shaped as a command of an agent is, so that both are decided alike
    /// (issue 304).
    ShowCommand { command: &'a Command, channel: &'a SecurityAttributes },
    /// Running the session's command `command` in the channel, alike. Whom
    /// the command is for is the command's own word (`Command::owners_only`).
    ExecuteCommand { command: &'a Command, channel: &'a SecurityAttributes },
    /// Seeing that an agent carries out `command` (issue 269) - in `/help`,
    /// in completion, in the member's snapshot. Carries the agent's
    /// attributes and those of the channel it is asked in.
    ShowAgentCommand { command: &'a Command, agent: &'a SecurityAttributes, channel: &'a SecurityAttributes },
    /// Asking the agent to carry out `command` (issue 269), alike. Whom the
    /// command is for is the command's own word (`Command::owners_only`).
    ExecuteAgentCommand { command: &'a Command, agent: &'a SecurityAttributes, channel: &'a SecurityAttributes },
    CallTool(&'a ToolCall),
    ReadSecret(&'a str),
}

impl<'a> ActionType<'a> {
    /// Doing `verb` with an entity of `kind` that has `attributes`.
    pub const fn entity(kind: EntityKind, verb: Verb, attributes: &'a SecurityAttributes) -> Self {
        ActionType::Entity { kind, verb, attributes }
    }

    /// Reading an entity of `kind` that has `attributes`.
    pub const fn get(kind: EntityKind, attributes: &'a SecurityAttributes) -> Self {
        Self::entity(kind, Verb::Get, attributes)
    }

    /// Creating an entity of `kind` that is to have `attributes`.
    pub const fn create(kind: EntityKind, attributes: &'a SecurityAttributes) -> Self {
        Self::entity(kind, Verb::Create, attributes)
    }

    /// Changing an entity of `kind` that has `attributes` as stored.
    pub const fn update(kind: EntityKind, attributes: &'a SecurityAttributes) -> Self {
        Self::entity(kind, Verb::Update, attributes)
    }

    /// Deleting an entity of `kind` that has `attributes` as stored.
    pub const fn delete(kind: EntityKind, attributes: &'a SecurityAttributes) -> Self {
        Self::entity(kind, Verb::Delete, attributes)
    }

    /// Reading a deleted entity of `kind` that has `attributes`.
    pub const fn read_deleted(kind: EntityKind, attributes: &'a SecurityAttributes) -> Self {
        Self::entity(kind, Verb::ReadDeleted, attributes)
    }

    /// Restoring a deleted entity of `kind` that has `attributes`.
    pub const fn restore(kind: EntityKind, attributes: &'a SecurityAttributes) -> Self {
        Self::entity(kind, Verb::Restore, attributes)
    }

    /// Purging an entity of `kind` that has `attributes`.
    pub const fn purge(kind: EntityKind, attributes: &'a SecurityAttributes) -> Self {
        Self::entity(kind, Verb::Purge, attributes)
    }

    /// Doing a kind's own `verb` with an entity of `kind` that has
    /// `attributes`, such as using a skill (`skill::USE`).
    pub const fn custom(kind: EntityKind, verb: &'static CustomVerb, attributes: &'a SecurityAttributes) -> Self {
        Self::entity(kind, Verb::Custom(verb), attributes)
    }

    /// The action for the log: its kind and what identifies what it is
    /// about, never content - an act on an entity by its verb and kind
    /// (`GetSession`, `UseSkill`), a tool call by its function, not its
    /// arguments; a secret by its name, not its value.
    pub fn describe(&self) -> String {
        match self {
            ActionType::Entity { kind, verb, .. } => format!("{}{}", verb.name(), kind.title()),
            ActionType::List(kind) => format!("List({kind})"),
            ActionType::ReadAgentContext(_) => "ReadAgentContext".into(),
            ActionType::ReadAgentTools { .. } => "ReadAgentTools".into(),
            ActionType::ReadAgentComponents(_) => "ReadAgentComponents".into(),
            ActionType::ApproveToolCall { .. } => "ApproveToolCall".into(),
            ActionType::StopSession(_) => "StopSession".into(),
            ActionType::CancelChannel(_) => "CancelChannel".into(),
            ActionType::WakeChannel(_) => "WakeChannel".into(),
            ActionType::RetryMessage(_) => "RetryMessage".into(),
            ActionType::AnswerQuestion(_) => "AnswerQuestion".into(),
            ActionType::ModifyMessage { author, .. } => format!("ModifyMessage(of {author})"),
            ActionType::RevokeMessage { author, .. } => format!("RevokeMessage(of {author})"),
            ActionType::Reclassify(target) => format!("Reclassify({target:?})"),
            ActionType::ShowCommand { command, .. } => format!("ShowCommand(/{})", command.name),
            ActionType::ExecuteCommand { command, .. } => format!("ExecuteCommand(/{})", command.name),
            ActionType::ShowAgentCommand { command, .. } => format!("ShowAgentCommand(/{})", command.name),
            ActionType::ExecuteAgentCommand { command, .. } => format!("ExecuteAgentCommand(/{})", command.name),
            ActionType::CallTool(call) => format!("CallTool({})", call.function_name),
            ActionType::ReadSecret(key) => format!("ReadSecret({key})"),
        }
    }

    /// This action as refused (issue 323): named by what it is about - the
    /// kind of an entity, or what one of the core's own actions concerns -
    /// and described as the log describes it.
    pub fn refused(&self) -> Refused {
        let subject = match self {
            ActionType::Entity { kind, .. } | ActionType::List(kind) => kind.name(),
            ActionType::ReadAgentContext(_) => "context",
            ActionType::ReadAgentTools { .. } | ActionType::ReadAgentComponents(_) => "agent",
            ActionType::ApproveToolCall { .. } => "approval",
            ActionType::StopSession(_) => "session",
            ActionType::CancelChannel(_) => "cancel",
            ActionType::WakeChannel(_) => "channel",
            ActionType::RetryMessage(_) | ActionType::ModifyMessage { .. } | ActionType::RevokeMessage { .. } => "message",
            ActionType::AnswerQuestion(_) => "question",
            ActionType::Reclassify(_) => "classification",
            ActionType::ShowCommand { .. } | ActionType::ExecuteCommand { .. }
            | ActionType::ShowAgentCommand { .. } | ActionType::ExecuteAgentCommand { .. } => "command",
            ActionType::CallTool(_) => "tool",
            ActionType::ReadSecret(_) => "secret",
        };
        Refused { subject, action: self.describe() }
    }
}

/// An action refused (issue 323): what it was, without the attributes it
/// was asked with. Made where the permission is checked
/// (`Enforcement::require`) and carried by the error the refusal ends in,
/// as its source, so that whoever answers the error - the REST server -
/// finds it there and says what was refused, wherever the error went.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{action} is not permitted")]
pub struct Refused {
    /// What it is about, as the REST API names it in its codes
    /// (`skill_forbidden`): the kind of an entity, or `context`, `agent`,
    /// `command`, … for the core's own actions.
    pub subject: &'static str,
    /// The action as `ActionType::describe` names it.
    pub action: String,
}

/// What is done with a stored entity (`ActionType::Entity`, issue 323):
/// the verbs every kind has, and those a kind brings of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Get,
    Create,
    Update,
    Delete,
    /// Reading what was deleted - marked, not purged (issue 164): one
    /// entity, or each of a listing. The trash.
    ReadDeleted,
    /// Taking the mark of a deletion away (issue 164).
    Restore,
    /// Removing an entity for good, marked or not, with what belongs to it
    /// (issue 164) - an administrator's act, or a retention policy's (issue
    /// 07).
    Purge,
    /// A verb of one kind or a few, decided by what it requires.
    Custom(&'static CustomVerb),
}

impl Verb {
    /// Its name, as `describe` gives it.
    pub fn name(self) -> &'static str {
        match self {
            Verb::Get => "Get",
            Verb::Create => "Create",
            Verb::Update => "Update",
            Verb::Delete => "Delete",
            Verb::ReadDeleted => "ReadDeleted",
            Verb::Restore => "Restore",
            Verb::Purge => "Purge",
            Verb::Custom(custom) => custom.name,
        }
    }
}

/// A verb a kind brings beside those every kind has (issue 323), such as
/// using a skill or writing a workspace's files. A policy decides it by
/// what it `requires`, which names a right the policy knows: a verb can be
/// mapped onto a right, never invent one. A policy that decides one verb of
/// one kind otherwise tells it by its kind and `name`.
#[derive(Debug, PartialEq, Eq)]
pub struct CustomVerb {
    /// Its name, unique among the verbs of a kind.
    pub name: &'static str,
    pub requires: Requirement,
}

/// What a `CustomVerb` requires of the one who does it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirement {
    /// What an entry of the access list grants, or owning the entity.
    Grant(Grant),
    /// Owning the entity.
    Owner,
}

/// Changing the content of what is stored, beside reading it (issue 23):
/// a workspace's files, an artifact's versions - what an access list
/// grants `Write`.
pub const WRITE: CustomVerb = CustomVerb { name: "Write", requires: Requirement::Grant(Grant::Write) };

/// What is reclassified (`ActionType::Reclassify`, issue 133), with who it
/// belongs to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reclassified<'a> {
    /// The channel itself, with its attributes: who owns it.
    Channel { attributes: &'a SecurityAttributes },
    /// A message of the channel, written by `author`; with the channel's
    /// attributes, since its owner owns what is in it.
    Message { author: ActorId, attributes: &'a SecurityAttributes },
    /// A note, public or private, of the agent `author` in the channel;
    /// with the channel's attributes, as a message, for a policy that
    /// decides by them (issue 312) - the ownership policy decides by
    /// authorship alone.
    Note { author: ActorId, attributes: &'a SecurityAttributes },
    /// An entry of the memory of the agent `author`, reclassified in the
    /// channel with these attributes, as a note.
    Memory { author: ActorId, attributes: &'a SecurityAttributes },
}

/// Who an entity belongs to and who else may reach it - what a policy
/// decides by (issues 11, 23).
///
/// The owner may do everything with it, implicitly and unrevocably; the
/// access list only adds - allow-only, the union of its entries, without
/// precedence. An entity without an owner (`ActorId::None`, as in the demo
/// data of a seed file) belongs to the system.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SecurityAttributes {
    pub owner_id: ActorId,
    /// May be left out where the attributes are written by hand - a seed
    /// file (issue 108).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acl: Vec<AclEntry>,
}

impl SecurityAttributes {
    /// Attributes of an entity owned by `owner`, with an empty access list.
    pub fn owned_by(owner: ActorId) -> Self {
        Self { owner_id: owner, acl: Vec::new() }
    }

    /// These attributes, granting `subject` what `grant` covers besides.
    pub fn granting(mut self, subject: Subject, grant: Grant) -> Self {
        self.acl.push(AclEntry { subject, grant });
        self
    }

    /// Nothing if the attributes may be stored as they are: an entry names
    /// a person, never an agent - an agent acts with its owner's rights
    /// (issue 255), and an entry for it would let it reach what the person
    /// responsible for it may not - nor the system or nobody.
    pub fn validate(&self) -> Result<(), String> {
        for entry in &self.acl {
            if let Subject::Actor(actor) = entry.subject {
                if !matches!(actor, ActorId::User(_)) {
                    return Err(format!("an access list names users, roles and groups, not {actor}"));
                }
            }
        }
        Ok(())
    }
}

/// One entry of an access list (issue 23): whom it grants what.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AclEntry {
    pub subject: Subject,
    pub grant: Grant,
}

/// Whom an entry of an access list grants something (issue 23). Matched
/// against the context's principal - an agent's owner -, and for what is
/// read also against its caller, an agent for what it takes part in itself
/// (issue 264).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subject {
    /// A user.
    Actor(ActorId),
    /// Whoever holds the role - an agent its owner's roles, narrowed by its
    /// personality's limiting roles.
    Role(RoleId),
    /// The members of a group (issues 286, 304) - a team, or the current
    /// participants of a session or a channel -, as the policy learns from
    /// its `Memberships`.
    Group(GroupId),
}

/// A subject as a query compares it, after serde's tags:
/// `{"actor": {"User": "…"}}`, `{"role": "…"}`.
impl From<Subject> for FieldValue {
    fn from(subject: Subject) -> Self {
        match subject {
            Subject::Actor(actor) => FieldValue::tagged("actor", actor),
            Subject::Role(role) => FieldValue::tagged("role", role),
            Subject::Group(group) => FieldValue::tagged("group", group),
        }
    }
}

/// What an entry of an access list grants (issue 23). What is neither -
/// changing the attributes, deleting, reclassifying - is the owner's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grant {
    /// Getting, listing, downloading, using.
    Read,
    /// Changing the content, besides reading it. No action asks for it
    /// yet; the workspace's files and an artifact's versions will.
    Write,
}

impl Grant {
    /// Whether this grant covers what `needed` asks for.
    pub fn covers(self, needed: Grant) -> bool {
        self == Grant::Write || needed == Grant::Read
    }
}

/// What a policy needs to know of actors that the attributes it is handed
/// do not say (issue 23): who belongs to a group an access list names
/// (issue 286). A fact, not a decision
/// - the decision stays the policy's. Implemented over the stores, read in
/// the system's name: one group at a time, never what an actor is a member
/// of.
#[async_trait]
pub trait Memberships: Debug + Send + Sync {
    /// The members of `group` - also of one that was deleted, whose members
    /// are told of the deletion; none of one that does not exist.
    async fn members_of(&self, group: GroupId) -> Vec<ActorId>;
}

/// Knows of no membership: for a policy without access lists
/// naming them - tests, and what is built before a store is.
#[derive(Debug, Default)]
pub struct NoMemberships;

#[async_trait]
impl Memberships for NoMemberships {
    async fn members_of(&self, _group: GroupId) -> Vec<ActorId> {
        Vec::new()
    }
}

/// An entity that carries `SecurityAttributes`.
pub trait Secured {
    fn security_attributes(&self) -> &SecurityAttributes;
}

/// Who acts, with whose rights, and for whom (issue 255): what every
/// operation is asked in.
///
/// - `caller_id` is who acts - a user, an agent, the system.
/// - `principal_id` is the person whose rights the caller exercises: a user
///   themselves, an agent its owner, the system itself. An agent is its
///   owner's delegate in all it does.
/// - `initiator_id` is who caused it - whoever asked the agent, or its owner
///   when nobody did: for the record and for the questions of a call, never
///   for rights.
///
/// - `roles` are the principal's roles, and `limiting_roles` those the
///   caller is limited to - an agent's personality's (issue 179) -, set where
///   the context is built (issue 304): what a decision needs to know of the
///   subject is in the context, not looked up while it is made. A role
///   counts if the principal holds it and the limit, if any, names it
///   (`has_role`); a clearance holds if both clear (see CONFIDENTIALITY.md).
///
/// An empty context names nobody and has no rights.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityContext {
    pub initiator_id: ActorId,
    pub caller_id: ActorId,
    pub principal_id: ActorId,
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    pub roles: Vec<RoleId>,
    pub limiting_roles: Option<Vec<RoleId>>,
}

impl SecurityContext {
    /// The context of `actor` acting for themselves, in the channel
    /// `channel_id` of session `session_id`: caller, principal and initiator
    /// at once.
    pub fn new(actor: ActorId, session_id: SessionId, channel_id: ChannelId) -> Self {
        Self {
            initiator_id: actor,
            caller_id: actor,
            principal_id: actor,
            session_id,
            channel_id,
            roles: Vec::new(),
            limiting_roles: None,
        }
    }

    /// This context with the principal's `roles`, and the `limiting_roles`
    /// the caller is limited to, if any.
    pub fn with_roles(self, roles: Vec<RoleId>, limiting_roles: Option<Vec<RoleId>>) -> Self {
        Self { roles, limiting_roles, ..self }
    }

    /// Whether `role` counts for the context: the principal holds it, and
    /// the caller's limit, if any, names it.
    pub fn has_role(&self, role: RoleId) -> bool {
        self.roles.contains(&role) && self.limiting_roles.as_ref().is_none_or(|limit| limit.contains(&role))
    }
    /// The context of the runtime acting on its own - for work that follows
    /// from an action already permitted, or that no user started (initial
    /// data, records kept in step with events). The ownership policy allows
    /// it everything, so it is used deliberately and never as a fallback.
    pub fn system() -> Self {
        Self::new(ActorId::System, SessionId::default(), ChannelId::default())
    }

    /// This context continued by `caller_id`, acting with the rights of
    /// `principal_id` - an agent taking up a request, with its owner's - for
    /// the same initiator, in the same place. Without roles: whoever
    /// continues it gives the new principal's (`with_roles`), so that the
    /// initiator lends nothing.
    pub fn next(&self, caller_id: ActorId, principal_id: ActorId) -> Self {
        Self {
            initiator_id: self.initiator_id,
            caller_id,
            principal_id,
            session_id: self.session_id,
            channel_id: self.channel_id,
            roles: Vec::new(),
            limiting_roles: None,
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
    /// Whether `context` may read the session with `session` attributes -
    /// its participants, its channels (`GetSession`).
    async fn may_read_session(&self, context: &SecurityContext, session: &SecurityAttributes) -> bool;

    /// Whether `context` may read the channel with `channel` attributes -
    /// its members, its history, its events (`GetChannel`).
    async fn may_read_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool;

    /// Whether `context` may change the channel with `channel` attributes -
    /// rename it, add and remove participants (`UpdateChannel`).
    async fn may_change_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool;

    /// Whether `context` may change the session with `session` attributes -
    /// rename it, change its privacy (`UpdateSession`).
    async fn may_change_session(&self, context: &SecurityContext, session: &SecurityAttributes) -> bool;

    /// Whether `context` may change a workspace with these attributes - its
    /// name, description, classification, access list - and attach it to a
    /// session (`UpdateWorkspace`, issue 253).
    async fn may_change_workspace(&self, context: &SecurityContext, workspace: &SecurityAttributes) -> bool;

    /// Whether `context` may change the files of a workspace with these
    /// attributes (`WriteWorkspace`, issue 23).
    async fn may_write_workspace(&self, context: &SecurityContext, workspace: &SecurityAttributes) -> bool;

    /// Whether `context` may read a workspace with these attributes - see
    /// it and read its files (`GetWorkspace`, issue 23): asked where a
    /// workspace is reached, as a channel is (issue 304).
    async fn may_read_workspace(&self, context: &SecurityContext, workspace: &SecurityAttributes) -> bool;

    /// Whether `context` may change an artifact with these attributes -
    /// keep it in another scope, describe it, share it, delete it
    /// (`UpdateArtifact`, issue 271).
    async fn may_change_artifact(&self, context: &SecurityContext, artifact: &SecurityAttributes) -> bool;

    /// Whether `context` may read what has these attributes - an artifact,
    /// or one of its versions (`GetArtifact`, issue 23).
    async fn may_read_artifact(&self, context: &SecurityContext, artifact: &SecurityAttributes) -> bool;

    /// Whether `context` may add a version to an artifact with these
    /// attributes (`WriteArtifact`, issue 23).
    async fn may_write_artifact(&self, context: &SecurityContext, artifact: &SecurityAttributes) -> bool;

    /// Whether `context` may see what the agent with `agent` attributes saw
    /// when it wrote a message (`ReadAgentContext`, issue 41).
    async fn may_read_agent_context(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool;

    /// Whether `context` may see which tools the agent with `agent`
    /// attributes has now in the channel with `channel` attributes
    /// (`ReadAgentTools`, issue 194).
    async fn may_read_agent_tools(&self, context: &SecurityContext, agent: &SecurityAttributes, channel: &SecurityAttributes) -> bool;

    /// Whether `context` may see which components the agent with `agent`
    /// attributes was built from (`ReadAgentComponents`, issue 270).
    async fn may_read_agent_components(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool;

    /// Whether `context` may approve the tool calls of the agent with
    /// `agent` attributes in the channel with `channel` attributes
    /// (`ApproveToolCall`, issue 14).
    async fn may_approve(&self, context: &SecurityContext, agent: &SecurityAttributes, channel: &SecurityAttributes) -> bool;

    /// Whether `context` may delete the agent with `agent` attributes
    /// (`DeleteAgent`, issue 164).
    async fn may_delete_agent(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool;

    /// Whether `context` may ask an agent of the channel with `channel`
    /// attributes to answer again instead of one of its messages, or of its
    /// answer under way (`RetryMessage`, issues 234, 235) - asked to offer
    /// it; the channel asks again as it admits the request (issue 304).
    async fn may_retry_message(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool;

    /// Whether `context` may end the answers under way in the channel with
    /// `channel` attributes, or one agent's (`CancelChannel`, issues 235,
    /// 243) - asked by the channel as it cancels.
    async fn may_cancel_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool;

    /// Whether `context` may take back the deletion of an entity of `kind`
    /// with `attributes` (`Restore`, issue 164).
    async fn may_restore(&self, context: &SecurityContext, kind: EntityKind, attributes: &SecurityAttributes) -> bool;

    /// Whether `context` may remove an entity of `kind` with `attributes`
    /// for good (`Purge`, issue 164).
    async fn may_purge(&self, context: &SecurityContext, kind: EntityKind, attributes: &SecurityAttributes) -> bool;

    /// Whether `context` may change the skill with `skill` attributes - its
    /// name, description, body, classification - and delete it
    /// (`UpdateSkill`, issue 22).
    async fn may_change_skill(&self, context: &SecurityContext, skill: &SecurityAttributes) -> bool;
    /// Whether `context` may rename a group with these attributes, change
    /// its members and delete it (`UpdateGroup`, issue 286).
    async fn may_change_group(&self, context: &SecurityContext, group: &SecurityAttributes) -> bool;

    /// Whether `context` may load the skill with `skill` attributes into a
    /// turn (`UseSkill`, issue 22).
    async fn may_use_skill(&self, context: &SecurityContext, skill: &SecurityAttributes) -> bool;
}

#[async_trait]
impl<T: SecurityManager + ?Sized> AccessQuestions for T {
    async fn may_read_session(&self, context: &SecurityContext, session: &SecurityAttributes) -> bool {
        self.check(context, ActionType::get(Session::KIND, session)).await
    }

    async fn may_read_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::get(Channel::KIND, channel)).await
    }

    async fn may_change_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::update(Channel::KIND, channel)).await
    }

    async fn may_change_session(&self, context: &SecurityContext, session: &SecurityAttributes) -> bool {
        self.check(context, ActionType::update(Session::KIND, session)).await
    }

    async fn may_change_workspace(&self, context: &SecurityContext, workspace: &SecurityAttributes) -> bool {
        self.check(context, ActionType::update(Workspace::KIND, workspace)).await
    }

    async fn may_write_workspace(&self, context: &SecurityContext, workspace: &SecurityAttributes) -> bool {
        self.check(context, ActionType::custom(Workspace::KIND, &WRITE, workspace)).await
    }

    async fn may_read_workspace(&self, context: &SecurityContext, workspace: &SecurityAttributes) -> bool {
        self.check(context, ActionType::get(Workspace::KIND, workspace)).await
    }

    async fn may_change_artifact(&self, context: &SecurityContext, artifact: &SecurityAttributes) -> bool {
        self.check(context, ActionType::update(Artifact::KIND, artifact)).await
    }

    async fn may_read_artifact(&self, context: &SecurityContext, artifact: &SecurityAttributes) -> bool {
        self.check(context, ActionType::get(Artifact::KIND, artifact)).await
    }

    async fn may_write_artifact(&self, context: &SecurityContext, artifact: &SecurityAttributes) -> bool {
        self.check(context, ActionType::custom(Artifact::KIND, &WRITE, artifact)).await
    }

    async fn may_read_agent_context(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool {
        self.check(context, ActionType::ReadAgentContext(agent)).await
    }

    async fn may_read_agent_tools(&self, context: &SecurityContext, agent: &SecurityAttributes, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::ReadAgentTools { agent, channel }).await
    }

    async fn may_read_agent_components(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool {
        self.check(context, ActionType::ReadAgentComponents(agent)).await
    }

    async fn may_approve(&self, context: &SecurityContext, agent: &SecurityAttributes, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::ApproveToolCall { agent, channel }).await
    }

    async fn may_delete_agent(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool {
        self.check(context, ActionType::delete(Agent::KIND, agent)).await
    }

    async fn may_retry_message(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::RetryMessage(channel)).await
    }

    async fn may_cancel_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::CancelChannel(channel)).await
    }

    async fn may_restore(&self, context: &SecurityContext, kind: EntityKind, attributes: &SecurityAttributes) -> bool {
        self.check(context, ActionType::restore(kind, attributes)).await
    }

    async fn may_purge(&self, context: &SecurityContext, kind: EntityKind, attributes: &SecurityAttributes) -> bool {
        self.check(context, ActionType::purge(kind, attributes)).await
    }

    async fn may_change_group(&self, context: &SecurityContext, group: &SecurityAttributes) -> bool {
        self.check(context, ActionType::update(Group::KIND, group)).await
    }

    async fn may_change_skill(&self, context: &SecurityContext, skill: &SecurityAttributes) -> bool {
        self.check(context, ActionType::update(Skill::KIND, skill)).await
    }

    async fn may_use_skill(&self, context: &SecurityContext, skill: &SecurityAttributes) -> bool {
        self.check(context, ActionType::custom(Skill::KIND, &skill::USE, skill)).await
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
    /// Nothing if `context` may do `action`; the caller's error made by
    /// `refusal` of what was refused if not. The error carries the
    /// [`Refused`] as its source, unless it deliberately says less, such as
    /// "not found".
    async fn require<E, F>(&self, context: &SecurityContext, action: ActionType<'_>, refusal: F) -> Result<(), E>
    where
        E: Send,
        F: FnOnce(Refused) -> E + Send;
}

#[async_trait]
impl<T: SecurityManager + ?Sized> Enforcement for T {
    async fn require<E, F>(&self, context: &SecurityContext, action: ActionType<'_>, refusal: F) -> Result<(), E>
    where
        E: Send,
        F: FnOnce(Refused) -> E + Send,
    {
        if self.check(context, action).await {
            return Ok(());
        }
        let refused = action.refused();
        tracing::debug!("Refused {} for {}", refused.action, context.principal_id);
        Err(refusal(refused))
    }
}

/// Allows every action to everybody: for tests and development only. What
/// may flow where is not its to decide (issue 304) - the
/// `ConfidentialityPolicy` applies beside it all the same.
#[derive(Debug, Default)]
pub struct DummySecurityManager;

impl DummySecurityManager {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SecurityManager for DummySecurityManager {
    async fn check(&self, _context: &SecurityContext, _action: ActionType<'_>) -> bool {
        true
    }
}

/// The ownership policy (issues 11, 23): what is yours you may do
/// everything with, what an access list grants you you may read and use,
/// what belongs to the system everybody may read and use - and nobody acts
/// in someone else's name.
///
/// - The system (`ActorId::System`) may do everything.
/// - Reading - which also covers using, e.g. starting a session from a
///   template, or working on a workspace outside a session - is allowed to
///   the owner, to whom an entry of the access list grants it, and to
///   everybody for entities owned by the system or by no one. An agent
///   reads what its owner may read, and what it takes part in itself - a
///   member as the caller (issue 264); everything else it does, it does as
///   its owner.
/// - Creating is allowed only with oneself as owner.
/// - Changing, deleting and stopping are the owner's alone, and so is reading what an
///   agent saw: its context holds more than the conversation - its system
///   prompt, internal steps, tool results.
/// - Cancelling or waking a channel is allowed to whoever may read it.
/// - What may flow where, and who is cleared for what, is not its to decide:
///   the `ConfidentialityPolicy` decides it beside it (issue 304, reversing
///   issue 138).
/// - A channel may be reclassified by its owner, a message by the channel's
///   owner or its author, a note or memory entry by its author.
/// - A tool call of an agent is approved by the agent's owner and whom its
///   access list lets read it, of an agent of the system by the channel's
///   owner (issue 14).
/// - Searching and listing are allowed; their results are filtered by what
///   the caller may read. Tool calls, secrets and commands are governed
///   elsewhere for now (issues 12 and 23).
///
/// Who "you" are is the context's principal: the person whose rights its
/// caller exercises - a user themselves, an agent its owner, whoever asked
/// it (issue 255). An agent is its owner's delegate: the owner checks the
/// request where a call is admitted, and the asker lends it nothing.
#[derive(Debug)]
pub struct OwnershipSecurityManager {
    memberships: Arc<dyn Memberships>,
}

impl Default for OwnershipSecurityManager {
    fn default() -> Self {
        Self { memberships: Arc::new(NoMemberships) }
    }
}

impl OwnershipSecurityManager {
    /// The policy knowing of no memberships.
    pub fn new() -> Self {
        Self::default()
    }

    /// The policy, learning who belongs to a group from `memberships`.
    pub fn with_memberships(memberships: Arc<dyn Memberships>) -> Self {
        Self { memberships }
    }

    /// Whether the context answers for the agent with `agent` attributes in
    /// the channel with `channel` attributes - approves its calls, runs the
    /// commands that are its owners' (issues 14, 269): who is responsible
    /// for the agent - its owner, or whom its access list lets read it -,
    /// also in someone else's channel; for an agent of the system, whose
    /// channel it is.
    async fn answers_for(&self, agent: &SecurityAttributes, channel: &SecurityAttributes, context: &SecurityContext) -> bool {
        let me = context.principal_id;
        if Self::system_owned(agent) {
            channel.owner_id == me && me != ActorId::None
        } else {
            self.granted(agent, Grant::Read, context, false).await
        }
    }

    fn system_owned(attributes: &SecurityAttributes) -> bool {
        matches!(attributes.owner_id, ActorId::None | ActorId::System)
    }

    /// Whether the context may do with an entity with `attributes` what
    /// `needed` asks for: it is the system's, the principal owns it, or an
    /// entry of its access list grants it - to the principal, or, where
    /// `as_caller` says so, to the caller itself (issue 264). The entries
    /// are matched in their order, and memberships and roles are asked for
    /// only where an entry names them.
    async fn granted(&self, attributes: &SecurityAttributes, needed: Grant, context: &SecurityContext, as_caller: bool) -> bool {
        let actors: &[ActorId] = if as_caller && context.caller_id != context.principal_id {
            &[context.principal_id, context.caller_id]
        } else {
            std::slice::from_ref(&context.principal_id)
        };
        if Self::system_owned(attributes) || actors.contains(&attributes.owner_id) {
            return true;
        }
        for entry in attributes.acl.iter().filter(|entry| entry.grant.covers(needed)) {
            let matches = match entry.subject {
                Subject::Actor(actor) => actors.contains(&actor),
                Subject::Group(group) => {
                    let members = self.memberships.members_of(group).await;
                    actors.iter().any(|actor| members.contains(actor))
                }
                Subject::Role(role) => context.has_role(role),
            };
            if matches {
                return true;
            }
        }
        false
    }
}

#[async_trait]
impl SecurityManager for OwnershipSecurityManager {
    async fn check(&self, context: &SecurityContext, action: ActionType<'_>) -> bool {
        let me = context.principal_id;
        if me == ActorId::System {
            return true;
        }
        let owns = |attributes: &SecurityAttributes| attributes.owner_id == me && me != ActorId::None;
        match action {
            ActionType::Entity { verb, attributes: a, .. } => match verb {
                // Reading: what the principal may read, or the caller itself
                // - an agent what it takes part in (issue 264).
                Verb::Get => self.granted(a, Grant::Read, context, true).await,
                // Creating is only allowed with oneself as owner; changing,
                // deleting, and what was deleted, only for the owner.
                Verb::Create | Verb::Update | Verb::Delete | Verb::ReadDeleted | Verb::Restore => owns(a),
                // Until there is an administrator (issue 125): the system
                // alone, as the retention policy (issue 07) acts.
                Verb::Purge => false,
                // What a kind's own verb requires (issue 323): using a skill
                // is granted as reading it - what the owner may read the
                // agent may follow (issue 22) -, changing a workspace's files
                // or adding an artifact's version as writing (issue 23); to
                // the principal, or an agent taking part itself.
                Verb::Custom(custom) => match custom.requires {
                    Requirement::Grant(grant) => self.granted(a, grant, context, true).await,
                    Requirement::Owner => owns(a),
                },
            },
            ActionType::List(_) => true,
            ActionType::CancelChannel(a)
            | ActionType::WakeChannel(a)
            | ActionType::RetryMessage(a)
            | ActionType::AnswerQuestion(a) => self.granted(a, Grant::Read, context, false).await,
            // What an agent can do is read by whoever reads the agent, and by
            // every member of the channel it works in with them.
            ActionType::ReadAgentTools { agent, channel } => {
                self.granted(agent, Grant::Read, context, false).await || self.granted(channel, Grant::Read, context, false).await
            }
            ActionType::ApproveToolCall { agent, channel } => self.answers_for(agent, channel, context).await,
            // A command of an agent is run by whoever may write in the
            // channel - its members -, unless the command is for the agent's
            // owners alone, as approving its calls is (issue 269).
            ActionType::ShowAgentCommand { command, agent, channel } | ActionType::ExecuteAgentCommand { command, agent, channel } => {
                if command.owners_only { self.answers_for(agent, channel, context).await } else { self.granted(channel, Grant::Read, context, false).await }
            }
            // A command of the session alike (issue 304): its members, or
            // for the owners alone, whoever may change the channel.
            ActionType::ShowCommand { command, channel } | ActionType::ExecuteCommand { command, channel } => {
                if command.owners_only { owns(channel) } else { self.granted(channel, Grant::Read, context, false).await }
            }
            ActionType::Reclassify(target) => match target {
                Reclassified::Channel { attributes } => owns(attributes),
                Reclassified::Message { author, attributes } => owns(attributes) || author == context.caller_id,
                Reclassified::Note { author, .. } | Reclassified::Memory { author, .. } => author == context.caller_id,
            },
            // A message is its author's to change: who said it, not whom
            // they acted for.
            ActionType::ModifyMessage { author, .. } | ActionType::RevokeMessage { author, .. } => author == context.caller_id && author != ActorId::None,
            ActionType::ReadAgentContext(a)
            | ActionType::ReadAgentComponents(a)
            | ActionType::StopSession(a) => owns(a),
            // Hooks for another policy (issue 24): what decides a call
            // today is the tools offered and the place's rules; secrets
            // have no owner to decide by yet (issue 310).
            ActionType::CallTool(_)
            | ActionType::ReadSecret(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::SessionTemplate;
    use crate::group::Group;
    use crate::scope::Scope;
    use crate::value::name::ScopedName;
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

    /// Owned by `owner`, read by `readers` (issue 23).
    fn shared(owner: ActorId, readers: &[ActorId]) -> SecurityAttributes {
        readers.iter().fold(SecurityAttributes::owned_by(owner), |attributes, reader| attributes.granting(Subject::Actor(*reader), Grant::Read))
    }

    /// Memberships as a test sets them.
    #[derive(Debug, Default)]
    struct Known {
        members: Vec<(ActorId, GroupId)>,
    }

    #[async_trait]
    impl Memberships for Known {
        async fn members_of(&self, group: GroupId) -> Vec<ActorId> {
            self.members.iter().filter(|(_, g)| *g == group).map(|(actor, _)| *actor).collect()
        }
    }

    fn knowing(known: Known) -> OwnershipSecurityManager {
        OwnershipSecurityManager::with_memberships(Arc::new(known))
    }

    #[tokio::test]
    async fn a_message_is_its_authors_to_modify_and_take_back() {
        let (alice, bob) = (user(), user());
        let bobs = shared(bob, &[alice]);
        for (author, who, allowed) in [(alice, alice, true), (alice, bob, false)] {
            assert_eq!(allows(who, ActionType::ModifyMessage { author, channel: &bobs }).await, allowed, "modified by {who}: the author's, not the channel owner's");
            assert_eq!(allows(who, ActionType::RevokeMessage { author, channel: &bobs }).await, allowed, "taken back by {who}");
        }
        let agent = ActorId::Agent(crate::agent::AgentId::new());
        let for_alice = context(alice).next(agent, alice);
        let policy = OwnershipSecurityManager::new();
        assert!(policy.check(&for_alice, ActionType::RevokeMessage { author: agent, channel: &bobs }).await, "an agent its own, acting for alice");
        assert!(!policy.check(&for_alice, ActionType::RevokeMessage { author: alice, channel: &bobs }).await, "but not alice's: who said it, not whom it acts for");
    }

    #[tokio::test]
    async fn an_agent_is_judged_by_its_owner_not_by_whoever_asked_it() {
        let (alice, bob) = (user(), user());
        let agent = ActorId::Agent(crate::agent::AgentId::new());
        let hers = SecurityAttributes::owned_by(alice);
        let policy = OwnershipSecurityManager::new();
        let asked_by_bob = context(bob).next(agent, alice);
        assert!(policy.check(&asked_by_bob, ActionType::get(Session::KIND, &hers)).await, "with alice's rights, whoever asked");
        assert!(policy.check(&asked_by_bob, ActionType::update(Session::KIND, &hers)).await, "all of them");
        let asked_by_alice = context(alice).next(agent, bob);
        assert!(!policy.check(&asked_by_alice, ActionType::get(Session::KIND, &hers)).await, "asking lends nothing, not even the owner's own rights to another's agent");
    }

    #[tokio::test]
    async fn access_questions_ask_the_policy() {
        let (alice, bob) = (user(), user());
        let hers = shared(alice, &[bob]);
        let policy = OwnershipSecurityManager::new();
        assert!(policy.may_read_channel(&context(bob), &hers).await, "a participant reads the channel");
        assert!(policy.may_change_channel(&context(alice), &hers).await, "the owner changes it");
        assert!(!policy.may_change_channel(&context(bob), &hers).await, "a participant does not");
        assert!(policy.may_change_session(&context(alice), &hers).await, "the owner changes a session");
        assert!(!policy.may_change_session(&context(bob), &hers).await, "a participant does not");
        assert!(!policy.may_read_agent_context(&context(bob), &hers).await, "what an agent saw is its owner's alone");
        assert!(allows(alice, ActionType::StopSession(&hers)).await, "the owner stops a session");
        assert!(!allows(bob, ActionType::StopSession(&hers)).await, "a participant does not");

    }

    #[tokio::test]
    async fn owners_do_everything_whom_the_list_grants_read_strangers_nothing() {
        let (alice, bob, carol) = (user(), user(), user());
        let hers = shared(alice, &[bob]);

        for action in [ActionType::get(Session::KIND, &hers), ActionType::update(Session::KIND, &hers), ActionType::delete(Session::KIND, &hers)] {
            assert!(allows(alice, action).await, "{action:?}");
            assert!(!allows(carol, action).await, "{action:?}");
        }
        assert!(allows(bob, ActionType::get(Session::KIND, &hers)).await);
        assert!(allows(bob, ActionType::CancelChannel(&hers)).await, "a reader acts in it");
        assert!(allows(alice, ActionType::ReadAgentContext(&hers)).await);
        assert!(!allows(bob, ActionType::ReadAgentContext(&hers)).await, "what an agent saw is its owner's alone");
        assert!(allows(alice, ActionType::ReadAgentComponents(&hers)).await);
        assert!(!allows(bob, ActionType::ReadAgentComponents(&hers)).await, "and what it was built from, which may have been shared with its owner alone");
        let theirs = SecurityAttributes::owned_by(carol);
        let tools = |agent, channel| ActionType::ReadAgentTools { agent, channel };
        assert!(allows(bob, tools(&hers, &theirs)).await, "who reads the agent sees what it can do");
        assert!(allows(carol, tools(&hers, &theirs)).await, "and every member of the channel");
        assert!(!allows(carol, tools(&hers, &hers)).await, "but nobody else");
        let approve = |agent, channel| ActionType::ApproveToolCall { agent, channel };
        assert!(allows(alice, approve(&hers, &theirs)).await, "the agent's owner approves its calls, in any channel");
        assert!(allows(bob, approve(&hers, &theirs)).await, "and so do its readers");
        assert!(!allows(carol, approve(&hers, &theirs)).await, "not whose channel it is");
        let systems = SecurityAttributes::owned_by(ActorId::System);
        assert!(allows(carol, approve(&systems, &theirs)).await, "an agent of the system: whose channel it is");
        assert!(!allows(alice, approve(&systems, &theirs)).await, "nobody else");
        assert!(!allows(bob, approve(&SecurityAttributes::default(), &hers)).await, "not even a reader of the channel");
        let command = |owners_only| Command { name: "x".to_string(), description: String::new(), usage: None, owners_only };
        let (members, owners) = (command(false), command(true));
        let run = |command, agent, channel| ActionType::ExecuteAgentCommand { command, agent, channel };
        let show = |command, agent, channel| ActionType::ShowAgentCommand { command, agent, channel };
        assert!(allows(carol, run(&members, &hers, &theirs)).await, "a command for the members: whoever may write in the channel");
        assert!(allows(carol, show(&members, &hers, &theirs)).await);
        assert!(!allows(bob, run(&members, &hers, &theirs)).await, "not the agent's reader outside the channel");
        assert!(allows(alice, run(&owners, &hers, &theirs)).await, "a command for the owners: the agent's owner");
        assert!(allows(bob, run(&owners, &hers, &theirs)).await, "and its readers");
        assert!(!allows(carol, run(&owners, &hers, &theirs)).await, "not whose channel it is");
        assert!(!allows(carol, show(&owners, &hers, &theirs)).await, "who may not run it does not see it");
        assert!(allows(carol, run(&owners, &systems, &theirs)).await, "an agent of the system: whose channel it is");
        // A command of the session is decided alike (issue 304), by the
        // channel alone.
        let run = |command, channel| ActionType::ExecuteCommand { command, channel };
        let show = |command, channel| ActionType::ShowCommand { command, channel };
        assert!(allows(carol, run(&members, &theirs)).await && allows(carol, show(&members, &theirs)).await, "a command for the members: whoever reads the channel");
        assert!(!allows(bob, run(&members, &theirs)).await, "nobody else");
        assert!(allows(carol, run(&owners, &theirs)).await, "a command for the owners: whose channel it is");
        assert!(!allows(alice, run(&owners, &theirs)).await && !allows(alice, show(&owners, &theirs)).await, "not a member who does not own it");
        assert!(!allows(bob, ActionType::update(Session::KIND, &hers)).await);
        assert!(!allows(bob, ActionType::delete(Session::KIND, &hers)).await);
        // A skill (issue 22) is used as it is read; changed by its owner.
        assert!(allows(bob, ActionType::custom(Skill::KIND, &skill::USE, &hers)).await, "a reader uses it");
        assert!(!allows(carol, ActionType::custom(Skill::KIND, &skill::USE, &hers)).await, "nobody else");
        assert!(allows(carol, ActionType::custom(Skill::KIND, &skill::USE, &systems)).await, "the system's, everybody");
        assert!(allows(bob, ActionType::get(Skill::KIND, &hers)).await && !allows(bob, ActionType::update(Skill::KIND, &hers)).await, "read, not changed");
    }

    #[tokio::test]
    async fn an_agent_reads_what_its_owner_may_read_or_it_takes_part_in_and_changes_as_its_owner() {
        let (alice, bob) = (user(), user());
        let (hers, his, member) = (ActorId::Agent(crate::agent::AgentId::new()), ActorId::Agent(crate::agent::AgentId::new()), ActorId::Agent(crate::agent::AgentId::new()));
        let place = GroupId::of_channel(ChannelId::new());
        let alices = SecurityAttributes::owned_by(alice).granting(Subject::Group(place), Grant::Read);
        let policy = knowing(Known { members: vec![(member, place)] });
        assert!(policy.check(&context(bob).next(hers, alice), ActionType::get(Channel::KIND, &alices)).await, "her agent, whoever asked");
        assert!(!policy.check(&context(bob).next(his, bob), ActionType::get(Channel::KIND, &alices)).await, "not his: he may not read it");
        assert!(policy.check(&context(bob).next(member, bob), ActionType::get(Channel::KIND, &alices)).await, "but an agent taking part in it");
        assert!(policy.check(&context(bob).next(member, bob), ActionType::get(Session::KIND, &alices)).await, "so for every read");
        assert!(!policy.check(&context(bob).next(member, bob), ActionType::update(Channel::KIND, &alices)).await, "changing stays its owner's");
        assert!(!policy.check(&context(bob).next(member, bob), ActionType::CancelChannel(&alices)).await, "and so does acting on it");
    }

    #[tokio::test]
    async fn a_channel_is_reclassified_by_its_owner_a_message_also_by_its_author() {
        let policy = OwnershipSecurityManager::new();
        let (owner, author, stranger) = (user(), user(), user());
        let attributes = shared(owner, &[author, stranger]);
        let may = |who, target| {
            let policy = &policy;
            async move { policy.check(&context(who), ActionType::Reclassify(target)).await }
        };
        let channel_itself = Reclassified::Channel { attributes: &attributes };
        assert!(may(owner, channel_itself).await, "the owner reclassifies the channel");
        assert!(!may(author, channel_itself).await, "a participant does not");

        let message = Reclassified::Message { author, attributes: &attributes };
        assert!(may(author, message).await, "the author their message");
        assert!(may(owner, message).await, "so does the channel's owner");
        assert!(!may(stranger, message).await, "nobody else");

        let agent = ActorId::Agent(crate::agent::AgentId::new());
        for own in [Reclassified::Note { author: agent, attributes: &attributes }, Reclassified::Memory { author: agent, attributes: &attributes }] {
            assert!(may(agent, own).await, "an agent what it wrote");
            assert!(!may(owner, own).await, "not even the channel's owner another's");
        }
    }

    #[tokio::test]
    async fn what_belongs_to_the_system_everybody_uses_and_only_the_system_changes() {
        let alice = user();
        for shared in [SecurityAttributes::default(), SecurityAttributes::owned_by(ActorId::System)] {
            assert!(allows(alice, ActionType::get(SessionTemplate::KIND, &shared)).await, "stored without an owner, or the system's");
            assert!(!allows(alice, ActionType::update(SessionTemplate::KIND, &shared)).await);
            assert!(allows(ActorId::System, ActionType::update(SessionTemplate::KIND, &shared)).await);
        }
    }

    #[tokio::test]
    async fn nobody_creates_in_someone_elses_name_and_no_one_at_all_creates() {
        let (alice, bob) = (user(), user());
        assert!(allows(alice, ActionType::create(Session::KIND, &SecurityAttributes::owned_by(alice))).await);
        assert!(!allows(bob, ActionType::create(Session::KIND, &SecurityAttributes::owned_by(alice))).await);
        assert!(!allows(ActorId::None, ActionType::create(Session::KIND, &SecurityAttributes::default())).await, "an empty context has no rights");
        assert!(!allows(ActorId::None, ActionType::update(Session::KIND, &SecurityAttributes::default())).await);
    }

    #[tokio::test]
    async fn members_of_a_group_read_while_they_are_members_wherever_they_stand() {
        let (alice, bob, carol) = (user(), user(), user());
        let place = GroupId::new();
        let hers = SecurityAttributes::owned_by(alice).granting(Subject::Group(place), Grant::Read);
        let policy = knowing(Known { members: vec![(bob, place)] });
        let elsewhere = SecurityContext::new(bob, SessionId::new(), ChannelId::new());
        assert!(policy.check(&elsewhere, ActionType::get(Session::KIND, &hers)).await, "a member, though he acts somewhere else");
        assert!(!policy.check(&context(carol), ActionType::get(Session::KIND, &hers)).await, "not who is no member");
        assert!(!policy.check(&context(bob), ActionType::update(Session::KIND, &hers)).await, "reading, not changing");
        let other = SecurityAttributes::owned_by(alice).granting(Subject::Group(GroupId::new()), Grant::Read);
        assert!(!policy.check(&context(bob), ActionType::get(Session::KIND, &other)).await, "a member of the group named, not of any");
    }

    #[tokio::test]
    async fn a_group_is_read_by_its_owner_and_its_members_and_changed_by_its_owner() {
        let (alice, bob, carol) = (user(), user(), user());
        let ActorId::User(alice_id) = alice else { unreachable!() };
        let team = Group::team(ScopedName::new("project", Scope::GlobalScope).unwrap(), alice_id);
        let policy = knowing(Known { members: vec![(bob, team.id)] });
        let theirs = &team.security_attributes;
        assert!(policy.check(&context(alice), ActionType::get(Group::KIND, theirs)).await, "its owner");
        assert!(policy.check(&context(bob), ActionType::get(Group::KIND, theirs)).await, "a member, by the group's own list");
        assert!(!policy.check(&context(carol), ActionType::get(Group::KIND, theirs)).await, "not a stranger");
        assert!(policy.check(&context(alice), ActionType::update(Group::KIND, theirs)).await && policy.check(&context(alice), ActionType::delete(Group::KIND, theirs)).await);
        assert!(!policy.check(&context(bob), ActionType::update(Group::KIND, theirs)).await, "a member does not change it");
    }

    #[tokio::test]
    async fn a_role_grants_to_whoever_holds_it_an_agent_narrowed_by_its_personality() {
        let (alice, bob) = (user(), user());
        let (hr, staff) = (RoleId::new(), RoleId::new());
        let agent = ActorId::Agent(crate::agent::AgentId::new());
        let hers = SecurityAttributes::owned_by(alice).granting(Subject::Role(hr), Grant::Read);
        // Bob holds hr; his agent is limited to staff, so of his roles only
        // that counts for it - as its context says (issue 304).
        let policy = OwnershipSecurityManager::new();
        let his_context = context(bob).with_roles(vec![hr, staff], None);
        let his_agents = context(bob).next(agent, bob).with_roles(vec![hr, staff], Some(vec![staff]));
        assert!(policy.check(&his_context, ActionType::get(Skill::KIND, &hers)).await, "who holds the role");
        assert!(!policy.check(&his_agents, ActionType::get(Skill::KIND, &hers)).await, "not his agent, limited below it");
        assert!(!policy.check(&context(bob).next(agent, bob), ActionType::get(Skill::KIND, &hers)).await, "nor a context continued without roles: the initiator lends none");
        let his = SecurityAttributes::owned_by(alice).granting(Subject::Actor(bob), Grant::Read);
        assert!(policy.check(&his_agents, ActionType::get(Skill::KIND, &his)).await, "what he is granted as a person, his agent is too");
    }

    #[tokio::test]
    async fn a_grant_to_read_is_not_one_to_write_and_the_owner_needs_none() {
        let (alice, bob) = (user(), user());
        assert!(Grant::Write.covers(Grant::Read) && Grant::Write.covers(Grant::Write));
        assert!(Grant::Read.covers(Grant::Read) && !Grant::Read.covers(Grant::Write));
        let policy = OwnershipSecurityManager::new();
        let hers = shared(alice, &[]);
        assert!(policy.granted(&hers, Grant::Write, &context(alice), false).await, "the owner, without an entry");
        let readable = shared(alice, &[bob]);
        assert!(policy.granted(&readable, Grant::Read, &context(bob), false).await);
        assert!(!policy.granted(&readable, Grant::Write, &context(bob), false).await, "read, not write");
        let writable = SecurityAttributes::owned_by(alice).granting(Subject::Actor(bob), Grant::Write);
        assert!(policy.granted(&writable, Grant::Read, &context(bob), false).await, "who may write may read");
    }

    #[tokio::test]
    async fn a_workspaces_files_are_read_and_changed_as_its_list_grants() {
        let (alice, bob, carol) = (user(), user(), user());
        let theirs = SecurityAttributes::owned_by(alice).granting(Subject::Actor(bob), Grant::Read).granting(Subject::Actor(carol), Grant::Write);
        assert!(allows(alice, ActionType::custom(Workspace::KIND, &WRITE, &theirs)).await, "its owner");
        assert!(allows(bob, ActionType::get(Workspace::KIND, &theirs)).await && !allows(bob, ActionType::custom(Workspace::KIND, &WRITE, &theirs)).await, "a reader reads");
        assert!(allows(carol, ActionType::get(Workspace::KIND, &theirs)).await && allows(carol, ActionType::custom(Workspace::KIND, &WRITE, &theirs)).await, "a writer both");
        assert!(!allows(carol, ActionType::update(Workspace::KIND, &theirs)).await, "changing the workspace itself stays the owner's");
    }

    #[test]
    fn an_access_list_names_people_roles_and_groups_never_an_agent() {
        let alice = user();
        let agent = ActorId::Agent(crate::agent::AgentId::new());
        assert_eq!(shared(alice, &[user()]).validate(), Ok(()));
        let place = Subject::Group(GroupId::new());
        assert_eq!(SecurityAttributes::owned_by(alice).granting(Subject::Role(RoleId::new()), Grant::Read).granting(place, Grant::Write).validate(), Ok(()));
        for nobody in [agent, ActorId::System, ActorId::None] {
            assert!(SecurityAttributes::owned_by(alice).granting(Subject::Actor(nobody), Grant::Read).validate().is_err(), "not {nobody}");
        }
        let written: SecurityAttributes = serde_json::from_value(serde_json::json!({"owner_id": alice})).unwrap();
        assert!(written.acl.is_empty(), "the list may be left out where it is written by hand");
    }

    #[test]
    fn an_action_is_described_without_its_content() {
        let call = ToolCall {
            id: None,
            function_name: "web_search".to_string(),
            arguments: serde_json::json!({ "query": "secret plans" }),
            visibility: Default::default(),
            presentation: Default::default(),
        };
        let described = ActionType::CallTool(&call).describe();
        assert_eq!(described, "CallTool(web_search)");
        assert!(!described.contains("secret plans"), "no arguments in the log");
        assert_eq!(ActionType::get(Session::KIND, &SecurityAttributes::default()).describe(), "GetSession");
    }

    #[tokio::test]
    async fn require_answers_a_refusal_with_the_callers_error() {
        #[derive(Debug, PartialEq)]
        struct Denied(Refused);
        let (owner, stranger) = (user(), user());
        let context = |actor| SecurityContext::new(actor, Default::default(), Default::default());
        let hers = SecurityAttributes::owned_by(owner);
        let policy = OwnershipSecurityManager::new();

        assert_eq!(policy.require(&context(owner), ActionType::delete(Session::KIND, &hers), Denied).await, Ok(()));
        let refused = Refused { subject: "session", action: "DeleteSession".to_string() };
        assert_eq!(policy.require(&context(stranger), ActionType::delete(Session::KIND, &hers), Denied).await, Err(Denied(refused)), "what was refused, named by its kind");
    }

    /// A subject as a query compares it (issue 293).
    #[test]
    fn a_subject_meets_exactly_the_subjects_equal_to_it() {
        let id = uuid::Uuid::new_v4();
        crate::store::query::meets_exactly_its_equals(&[
            Subject::Actor(ActorId::User(crate::user::UserId(id))), Subject::Actor(ActorId::Agent(crate::agent::AgentId(id))),
            Subject::Actor(ActorId::System), Subject::Role(RoleId(id)), Subject::Role(RoleId::new()),
            Subject::Group(GroupId(id)), Subject::Group(GroupId::new()),
        ]);
    }

}
