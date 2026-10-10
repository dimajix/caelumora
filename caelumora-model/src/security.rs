use crate::actor::ActorId;
use crate::session::SessionId;
use crate::channel::ChannelId;
use crate::command::Command;
use crate::confidentiality::{Classification, ConfidentialityRules};
use crate::privacy::SessionPrivacy;
use crate::role::{RoleId, Roles};
use crate::store::EntityKind;
use crate::store::query::FieldValue;
use crate::tool::ToolCall;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::sync::Arc;

/// Everything the `SecurityManager` is asked about.
///
/// An action on a stored entity carries that entity's `SecurityAttributes`,
/// so that a policy can decide by its owner and its access list (issues 11,
/// 23) without being handed the entity itself: `Get`, `Update` and `Delete` carry the
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
    /// `participant` joining a channel classified `channel` (issue 46), or -
    /// as a user - reading it: whether they are cleared for it (issue 133). The
    /// runtime looks up `roles` - a user's, an agent's owner's - and the
    /// policy maps them to clearances. Besides `UpdateChannel`, which
    /// decides who may add participants, and `GetChannel`, which decides who
    /// may read the channel at all.
    JoinChannel { participant: ActorId, roles: &'a [RoleId], channel: &'a Classification },
    /// Reading a user, e.g. receiving the events about them. A stored user
    /// belongs to the system (issue 94): see `User`'s `Secured`.
    GetUser(&'a SecurityAttributes),
    ListUsers,
    CreateUser(&'a SecurityAttributes),
    UpdateUser(&'a SecurityAttributes),
    DeleteUser(&'a SecurityAttributes),
    /// Reading a user's profile (issue 176), carrying its attributes. A
    /// profile belongs to its user: see `UserProfile`'s `Secured`.
    GetUserProfile(&'a SecurityAttributes),
    ListUserProfiles,
    CreateUserProfile(&'a SecurityAttributes),
    UpdateUserProfile(&'a SecurityAttributes),
    DeleteUserProfile(&'a SecurityAttributes),
    /// Reading a workspace (issue 253), carrying its attributes: the
    /// document, and its files - listing, downloading, the tools that only
    /// read (issue 23: what its access list grants `Read`). An agent using
    /// it through a channel is bounded by the channel besides.
    GetWorkspace(&'a SecurityAttributes),
    /// Changing the files of a workspace (issue 23), carrying its
    /// attributes: uploading, deleting, the tools that write, a command in
    /// its shell - what its access list grants `Write`.
    WriteWorkspace(&'a SecurityAttributes),
    ListWorkspaces,
    CreateWorkspace(&'a SecurityAttributes),
    /// Changing a workspace - its name, description, classification - with
    /// its attributes as stored.
    UpdateWorkspace(&'a SecurityAttributes),
    DeleteWorkspace(&'a SecurityAttributes),
    /// Reading a skill (issue 22): its name, description and body - in the
    /// list of skills, on its own, to change it.
    GetSkill(&'a SecurityAttributes),
    ListSkills,
    CreateSkill(&'a SecurityAttributes),
    /// Changing a skill - its name, description, body, classification - with
    /// its attributes as stored.
    UpdateSkill(&'a SecurityAttributes),
    DeleteSkill(&'a SecurityAttributes),
    /// Loading a skill into a turn (issue 22): what the agent then follows,
    /// asked in the turn's context - the agent for its owner, whoever asked
    /// as initiator. Apart from reading, so that a policy can let a skill be
    /// read but not used, or the other way round; whether its classification
    /// may flow into the channel is asked besides (`Publish`).
    UseSkill(&'a SecurityAttributes),
    /// Reading an artifact (issue 271), carrying its attributes - or a
    /// version's content, carrying what reads that version: the artifact's
    /// attributes with the version's own list (issue 23,
    /// `Artifact::version_attributes`).
    GetArtifact(&'a SecurityAttributes),
    /// Adding a version to an artifact (issue 23): what its list grants
    /// `Write`.
    WriteArtifact(&'a SecurityAttributes),
    ListArtifacts,
    CreateArtifact(&'a SecurityAttributes),
    /// Changing an artifact - a version added, its description, the scope
    /// it is kept in - with its attributes as stored.
    UpdateArtifact(&'a SecurityAttributes),
    DeleteArtifact(&'a SecurityAttributes),
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
    /// Copying something of the channel `from` - a message, a note, a
    /// memory entry, classified as `from` says - into another channel,
    /// `into`, where an agent reads it (issues 17, 264). Decided per entry
    /// by how it and `into` are classified (issue 133) and, across
    /// sessions, by whether the session of `from` lets that of `into` read
    /// it (issue 134). An entry released below its channel's classification
    /// may be copied where the channel as a whole may not - but a release
    /// adds no readers: whether the agent may reach the channel of `from`
    /// at all is asked before, as `GetChannel` (issue 264). What it knows
    /// itself of a channel - what it remembered, its private notes - is
    /// asked only this.
    CopyInformation { from: &'a Place, into: &'a Place },
    /// `actor` putting what is classified `classification` into a channel
    /// classified `channel` (issue 133): a message or a tool result into its history - asked by
    /// the channel for every one that enters it - or a tool result into the
    /// context of an agent in it, asked before the agent's model sees it. A
    /// classification other than the channel's releases a message to a
    /// wider audience or a lower level.
    Publish { classification: &'a Classification, actor: ActorId, channel: &'a Classification },
    /// Modifying a message of `author` in the channel with `channel`
    /// attributes (issue 234): what it says from now on. Asked of the one
    /// who sends the modification, before it is sent and as the channel
    /// admits it; the channel's attributes are there for a policy that lets
    /// someone moderate.
    ModifyMessage { author: ActorId, channel: &'a SecurityAttributes },
    /// Taking a message of `author` back in the channel with `channel`
    /// attributes (issue 234), asked as `ModifyMessage`.
    RevokeMessage { author: ActorId, channel: &'a SecurityAttributes },
    /// Giving something a classification - a channel when it is created
    /// (issue 133). Where a policy refuses compartments that are not
    /// configured.
    Classify(&'a Classification),
    /// Changing the classification of `target` from `current` to `new`
    /// (issue 133) - lowering it releases what it holds to a wider circle,
    /// raising it retracts nothing already read. `channel` is how the
    /// channel it belongs to, or is reclassified in, is classified now.
    Reclassify { target: Reclassified<'a>, current: &'a Classification, new: &'a Classification, channel: &'a Classification },
    /// Seeing that a user command exists - in `/help` and in completion.
    /// Keyed by the command's name, without the slash (issue 28).
    ShowCommand(&'a str),
    /// Running a user command, keyed by its name.
    ExecuteCommand(&'a str),
    /// Seeing that an agent carries out `command` (issue 269) - in `/help`,
    /// in completion, in the member's snapshot. Carries the agent's
    /// attributes and those of the channel it is asked in.
    ShowAgentCommand { command: &'a Command, agent: &'a SecurityAttributes, channel: &'a SecurityAttributes },
    /// Asking the agent to carry out `command` (issue 269), alike. Whom the
    /// command is for is the command's own word (`Command::owners_only`).
    ExecuteAgentCommand { command: &'a Command, agent: &'a SecurityAttributes, channel: &'a SecurityAttributes },
    CallTool(&'a ToolCall),
    ReadSecret(&'a str),
    GetComponents(&'a SecurityAttributes),
    ListComponents,
    CreateComponents(&'a SecurityAttributes),
    UpdateComponents(&'a SecurityAttributes),
    DeleteComponents(&'a SecurityAttributes),
    /// Reading what was deleted of a kind - marked, not purged (issue 164):
    /// one entity, carrying its stored attributes, or for each entity of a
    /// listing. The trash.
    ReadDeleted(EntityKind, &'a SecurityAttributes),
    /// Taking the mark of a deletion away (issue 164).
    Restore(EntityKind, &'a SecurityAttributes),
    /// Removing an entity for good, marked or not, with what belongs to it
    /// (issue 164) - an administrator's act, or a retention policy's (issue
    /// 07).
    Purge(EntityKind, &'a SecurityAttributes),
}

/// One side of a [`ActionType::CopyInformation`]: a channel, the
/// classification of what is read there or of the channel that reads, and
/// the session it belongs to as privacy sees it (issue 134). Whether two
/// places are one channel, or in one session, is told by their ids.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub channel: ChannelId,
    pub classification: Classification,
    pub session: SessionPrivacy,
}

/// How a channel may be read, where it may be read at all (issue 133): as
/// a whole, or only what was released to where the reader reads from,
/// entry by entry, each by its own classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelReading {
    Whole,
    Released,
}

/// How someone reads a channel (issue 133): an agent from its own place -
/// its channel, the classification it reads at, its session's privacy -,
/// a user by what their roles clear them for.
#[derive(Debug, Clone, Copy)]
pub enum ChannelReader<'a> {
    From(&'a Place),
    Cleared(&'a [RoleId]),
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
            ActionType::ReadAgentTools { .. } => "ReadAgentTools".into(),
            ActionType::ReadAgentComponents(_) => "ReadAgentComponents".into(),
            ActionType::ApproveToolCall { .. } => "ApproveToolCall".into(),
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
            ActionType::JoinChannel { participant, channel, .. } => format!("JoinChannel({participant} to {channel})"),
            ActionType::GetUser(_) => "GetUser".into(),
            ActionType::ListUsers => "ListUsers".into(),
            ActionType::CreateUser(_) => "CreateUser".into(),
            ActionType::UpdateUser(_) => "UpdateUser".into(),
            ActionType::DeleteUser(_) => "DeleteUser".into(),
            ActionType::GetUserProfile(_) => "GetUserProfile".into(),
            ActionType::ListUserProfiles => "ListUserProfiles".into(),
            ActionType::CreateUserProfile(_) => "CreateUserProfile".into(),
            ActionType::UpdateUserProfile(_) => "UpdateUserProfile".into(),
            ActionType::DeleteUserProfile(_) => "DeleteUserProfile".into(),
            ActionType::GetWorkspace(_) => "GetWorkspace".into(),
            ActionType::WriteWorkspace(_) => "WriteWorkspace".into(),
            ActionType::ListWorkspaces => "ListWorkspaces".into(),
            ActionType::CreateWorkspace(_) => "CreateWorkspace".into(),
            ActionType::UpdateWorkspace(_) => "UpdateWorkspace".into(),
            ActionType::DeleteWorkspace(_) => "DeleteWorkspace".into(),
            ActionType::GetSkill(_) => "GetSkill".into(),
            ActionType::ListSkills => "ListSkills".into(),
            ActionType::CreateSkill(_) => "CreateSkill".into(),
            ActionType::UpdateSkill(_) => "UpdateSkill".into(),
            ActionType::DeleteSkill(_) => "DeleteSkill".into(),
            ActionType::UseSkill(_) => "UseSkill".into(),
            ActionType::GetArtifact(_) => "GetArtifact".into(),
            ActionType::WriteArtifact(_) => "WriteArtifact".into(),
            ActionType::ListArtifacts => "ListArtifacts".into(),
            ActionType::CreateArtifact(_) => "CreateArtifact".into(),
            ActionType::UpdateArtifact(_) => "UpdateArtifact".into(),
            ActionType::DeleteArtifact(_) => "DeleteArtifact".into(),
            ActionType::CancelChannel(_) => "CancelChannel".into(),
            ActionType::WakeChannel(_) => "WakeChannel".into(),
            ActionType::RetryMessage(_) => "RetryMessage".into(),
            ActionType::AnswerQuestion(_) => "AnswerQuestion".into(),
            ActionType::CopyInformation { from, into, .. } => {
                format!("CopyInformation({} of {} into {})", from.classification, from.channel, into.channel)
            }
            ActionType::Publish { classification, actor, channel } => format!("Publish({classification} by {actor} into {channel})"),
            ActionType::ModifyMessage { author, .. } => format!("ModifyMessage(of {author})"),
            ActionType::RevokeMessage { author, .. } => format!("RevokeMessage(of {author})"),
            ActionType::Classify(classification) => format!("Classify({classification})"),
            ActionType::Reclassify { target, current, new, channel } => format!("Reclassify({target:?} in {channel} from {current} to {new})"),
            ActionType::ShowCommand(name) => format!("ShowCommand(/{name})"),
            ActionType::ExecuteCommand(name) => format!("ExecuteCommand(/{name})"),
            ActionType::ShowAgentCommand { command, .. } => format!("ShowAgentCommand(/{})", command.name),
            ActionType::ExecuteAgentCommand { command, .. } => format!("ExecuteAgentCommand(/{})", command.name),
            ActionType::CallTool(call) => format!("CallTool({})", call.function_name),
            ActionType::ReadSecret(key) => format!("ReadSecret({key})"),
            ActionType::GetComponents(_) => "GetComponents".into(),
            ActionType::ListComponents => "ListComponents".into(),
            ActionType::CreateComponents(_) => "CreateComponents".into(),
            ActionType::UpdateComponents(_) => "UpdateComponents".into(),
            ActionType::DeleteComponents(_) => "DeleteComponents".into(),
            ActionType::ReadDeleted(kind, _) => format!("ReadDeleted({kind})"),
            ActionType::Restore(kind, _) => format!("Restore({kind})"),
            ActionType::Purge(kind, _) => format!("Purge({kind})"),
        }
    }
}

/// What is reclassified (`ActionType::Reclassify`, issue 133), with who it
/// belongs to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reclassified<'a> {
    /// The channel itself, with its attributes: who owns it.
    Channel { attributes: &'a SecurityAttributes },
    /// A message of the channel, written by `author`; with the channel's
    /// attributes, since its owner owns what is in it.
    Message { author: ActorId, attributes: &'a SecurityAttributes },
    /// A note, public or private, of the agent `author` in the channel.
    Note { author: ActorId },
    /// An entry of the memory of the agent `author`, reclassified in the
    /// channel.
    Memory { author: ActorId },
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
                    return Err(format!("an access list names users, roles and members, not {actor}"));
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
    /// The current participants of a session or a channel, as the policy
    /// learns from its `Memberships`.
    MemberOf(Membership),
}

/// A session or a channel, as something one is a member of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Membership {
    Session(SessionId),
    Channel(ChannelId),
}

/// A subject as a query compares it, after serde's tags:
/// `{"actor": {"User": "…"}}`, `{"role": "…"}`.
impl From<Subject> for FieldValue {
    fn from(subject: Subject) -> Self {
        match subject {
            Subject::Actor(actor) => FieldValue::tagged("actor", actor),
            Subject::Role(role) => FieldValue::tagged("role", role),
            Subject::MemberOf(place) => FieldValue::tagged("member_of", place),
        }
    }
}

/// A place as a query compares it, after serde's tags: `{"session": "…"}`.
impl From<Membership> for FieldValue {
    fn from(place: Membership) -> Self {
        match place {
            Membership::Session(id) => FieldValue::tagged("session", id),
            Membership::Channel(id) => FieldValue::tagged("channel", id),
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
/// do not say (issue 23): whether someone is a participant of a session or
/// channel an access list names, and which roles count for them. A fact,
/// not a decision - the decision stays the policy's. Implemented by the
/// runtime over the stores, read in the system's name.
#[async_trait]
pub trait Memberships: Debug + Send + Sync {
    /// Whether `actor` is a current participant of `place`, a user or an
    /// agent - not an approver.
    async fn is_member_of(&self, actor: ActorId, place: Membership) -> bool;

    /// The roles that count for `actor`: a user's own; an agent's owner's,
    /// narrowed by its personality's limiting roles (issue 179).
    async fn roles_of(&self, actor: ActorId) -> Vec<RoleId>;
}

/// Knows of no membership and no roles: for a policy without access lists
/// naming them - tests, and what is built before a store is.
#[derive(Debug, Default)]
pub struct NoMemberships;

#[async_trait]
impl Memberships for NoMemberships {
    async fn is_member_of(&self, _actor: ActorId, _place: Membership) -> bool {
        false
    }

    async fn roles_of(&self, _actor: ActorId) -> Vec<RoleId> {
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
/// An empty context names nobody and has no rights.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityContext {
    pub initiator_id: ActorId,
    pub caller_id: ActorId,
    pub principal_id: ActorId,
    pub session_id: SessionId,
    pub channel_id: ChannelId
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

    /// This context continued by `caller_id`, acting with the rights of
    /// `principal_id` - an agent taking up a request, with its owner's - for
    /// the same initiator, in the same place.
    pub fn next(&self, caller_id: ActorId, principal_id: ActorId) -> Self {
        Self {
            initiator_id: self.initiator_id,
            caller_id,
            principal_id,
            session_id: self.session_id,
            channel_id: self.channel_id
        }
    }
}

#[async_trait]
pub trait SecurityManager : Debug + Sync + Send {
    async fn check(&self, context: &SecurityContext, action: ActionType<'_>) -> bool;

    /// The id of the role named `name` (issue 23), for what is written by
    /// hand; `None` if no such role is configured.
    fn role_id(&self, _name: &str) -> Option<RoleId> {
        None
    }

    /// The name of the role `id`, for what is shown to a person; `None` if
    /// no such role is configured.
    fn role_name(&self, _id: RoleId) -> Option<String> {
        None
    }
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

    /// Whether what comes from `from` - a message, a note, or what an agent
    /// remembered there - may be copied into the channel of `into`
    /// (`CopyInformation`, issues 17, 131, 133, 134 and 264): asked in
    /// `context`, the agent's who reads and passes it on. A channel may
    /// always be copied into itself.
    async fn may_copy_information(&self, context: &SecurityContext, from: &Place, into: &Place) -> bool;

    /// How `context` may read the channel with `channel` attributes, which
    /// lies `at` its place (issues 133, 264, 296): not at all if it may not
    /// read it (`GetChannel`). Beyond that, an agent reading from its place
    /// reads it as a whole if what is there may be copied into that place,
    /// and entry by entry otherwise; a user reads it as a whole if their
    /// roles clear them for its classification, and not at all otherwise.
    /// What the views reach a channel by, and what an operation in a channel
    /// asks before it acts there.
    async fn channel_reading(&self, context: &SecurityContext, channel: &SecurityAttributes, at: &Place, reader: ChannelReader<'_>) -> Option<ChannelReading>;

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

    /// Whether `context` may make the tool call `call` (`CallTool`) - asked
    /// before a call is put up for approval.
    async fn may_call_tool(&self, context: &SecurityContext, call: &ToolCall) -> bool;

    /// Whether `actor` may put what is classified `classification` into a
    /// channel classified `channel` (`Publish`, issue 133), asked in
    /// `context`: a message or a tool result entering the channel, a tool
    /// result entering the context of an agent in it, or a turn that began
    /// under another classification of the channel going on in it.
    async fn may_publish(&self, context: &SecurityContext, actor: ActorId, classification: &Classification, channel: &Classification) -> bool;

    /// Whether `context` may modify a message of `author` in the channel
    /// with `channel` attributes (`ModifyMessage`, issue 234) - asked by the
    /// channel as it admits the modification.
    async fn may_modify_message(&self, context: &SecurityContext, author: ActorId, channel: &SecurityAttributes) -> bool;

    /// Whether `context` may take a message of `author` back in the channel
    /// with `channel` attributes (`RevokeMessage`, issue 234) - asked by the
    /// channel as it admits the revocation.
    async fn may_revoke_message(&self, context: &SecurityContext, author: ActorId, channel: &SecurityAttributes) -> bool;

    /// Whether `context` may reclassify `target`, classified `current`, as
    /// `new` in a channel classified `channel` (`Reclassify`, issue 133) -
    /// asked by the channel as it admits a message's reclassification
    /// (issue 239).
    async fn may_reclassify(&self, context: &SecurityContext, target: Reclassified<'_>, current: &Classification, new: &Classification, channel: &Classification) -> bool;

    /// Whether `context` may ask an agent of the channel with `channel`
    /// attributes to answer again instead of one of its messages, or of its
    /// answer under way (`RetryMessage`, issues 234, 235) - asked by the
    /// channel as it admits the request.
    async fn may_retry_message(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool;

    /// Whether `context` may end the answers under way in the channel with
    /// `channel` attributes, or one agent's (`CancelChannel`, issues 235,
    /// 243) - asked by the channel as it cancels.
    async fn may_cancel_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool;

    /// Whether the user `context` acts for, with `roles`, is cleared for
    /// `channel` (`JoinChannel`, issue 133) - what decides, besides
    /// `GetChannel`, which channels they are shown.
    async fn may_see_channel(&self, context: &SecurityContext, roles: &[RoleId], channel: &Classification) -> bool;

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

    /// Whether `context` may load the skill with `skill` attributes into a
    /// turn (`UseSkill`, issue 22).
    async fn may_use_skill(&self, context: &SecurityContext, skill: &SecurityAttributes) -> bool;
}

#[async_trait]
impl<T: SecurityManager + ?Sized> AccessQuestions for T {
    async fn may_read_session(&self, context: &SecurityContext, session: &SecurityAttributes) -> bool {
        self.check(context, ActionType::GetSession(session)).await
    }

    async fn may_read_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::GetChannel(channel)).await
    }

    async fn may_change_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::UpdateChannel(channel)).await
    }

    async fn may_change_session(&self, context: &SecurityContext, session: &SecurityAttributes) -> bool {
        self.check(context, ActionType::UpdateSession(session)).await
    }

    async fn may_change_workspace(&self, context: &SecurityContext, workspace: &SecurityAttributes) -> bool {
        self.check(context, ActionType::UpdateWorkspace(workspace)).await
    }

    async fn may_write_workspace(&self, context: &SecurityContext, workspace: &SecurityAttributes) -> bool {
        self.check(context, ActionType::WriteWorkspace(workspace)).await
    }

    async fn may_change_artifact(&self, context: &SecurityContext, artifact: &SecurityAttributes) -> bool {
        self.check(context, ActionType::UpdateArtifact(artifact)).await
    }

    async fn may_read_artifact(&self, context: &SecurityContext, artifact: &SecurityAttributes) -> bool {
        self.check(context, ActionType::GetArtifact(artifact)).await
    }

    async fn may_write_artifact(&self, context: &SecurityContext, artifact: &SecurityAttributes) -> bool {
        self.check(context, ActionType::WriteArtifact(artifact)).await
    }

    async fn may_copy_information(&self, context: &SecurityContext, from: &Place, into: &Place) -> bool {
        if from.channel == into.channel {
            return true;
        }
        self.check(context, ActionType::CopyInformation { from, into }).await
    }

    async fn channel_reading(&self, context: &SecurityContext, channel: &SecurityAttributes, at: &Place, reader: ChannelReader<'_>) -> Option<ChannelReading> {
        if !self.may_read_channel(context, channel).await {
            return None;
        }
        match reader {
            ChannelReader::From(from) => Some(match self.may_copy_information(context, at, from).await {
                true => ChannelReading::Whole,
                false => ChannelReading::Released,
            }),
            ChannelReader::Cleared(roles) => self.may_see_channel(context, roles, &at.classification).await.then_some(ChannelReading::Whole),
        }
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
        self.check(context, ActionType::DeleteAgent(agent)).await
    }

    async fn may_call_tool(&self, context: &SecurityContext, call: &ToolCall) -> bool {
        self.check(context, ActionType::CallTool(call)).await
    }

    async fn may_publish(&self, context: &SecurityContext, actor: ActorId, classification: &Classification, channel: &Classification) -> bool {
        self.check(context, ActionType::Publish { classification, actor, channel }).await
    }

    async fn may_modify_message(&self, context: &SecurityContext, author: ActorId, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::ModifyMessage { author, channel }).await
    }

    async fn may_revoke_message(&self, context: &SecurityContext, author: ActorId, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::RevokeMessage { author, channel }).await
    }

    async fn may_reclassify(&self, context: &SecurityContext, target: Reclassified<'_>, current: &Classification, new: &Classification, channel: &Classification) -> bool {
        self.check(context, ActionType::Reclassify { target, current, new, channel }).await
    }

    async fn may_retry_message(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::RetryMessage(channel)).await
    }

    async fn may_cancel_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::CancelChannel(channel)).await
    }

    async fn may_see_channel(&self, context: &SecurityContext, roles: &[RoleId], channel: &Classification) -> bool {
        self.check(context, ActionType::JoinChannel { participant: context.principal_id, roles, channel }).await
    }

    async fn may_restore(&self, context: &SecurityContext, kind: EntityKind, attributes: &SecurityAttributes) -> bool {
        self.check(context, ActionType::Restore(kind, attributes)).await
    }

    async fn may_purge(&self, context: &SecurityContext, kind: EntityKind, attributes: &SecurityAttributes) -> bool {
        self.check(context, ActionType::Purge(kind, attributes)).await
    }

    async fn may_change_skill(&self, context: &SecurityContext, skill: &SecurityAttributes) -> bool {
        self.check(context, ActionType::UpdateSkill(skill)).await
    }

    async fn may_use_skill(&self, context: &SecurityContext, skill: &SecurityAttributes) -> bool {
        self.check(context, ActionType::UseSkill(skill)).await
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
        tracing::debug!("Refused {} for {}", action.describe(), context.principal_id);
        Err(refusal())
    }
}

#[derive(Debug, Default)]
pub struct DummySecurityManager {
    /// The configured roles, translated for what is written by hand and
    /// shown, though they decide nothing here.
    roles: Option<Roles>,
}

impl DummySecurityManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// The policy that allows everything, translating the configured
    /// `roles`.
    pub fn with_roles(roles: Option<Roles>) -> Self {
        Self { roles }
    }
}

#[async_trait]
impl SecurityManager for DummySecurityManager {
    async fn check(&self, _context: &SecurityContext, _action: ActionType<'_>) -> bool {
        true
    }

    fn role_id(&self, name: &str) -> Option<RoleId> {
        self.roles.as_ref()?.id_of(name).ok()
    }

    fn role_name(&self, id: RoleId) -> Option<String> {
        self.roles.as_ref()?.name_of(id).map(str::to_string)
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
/// - Confidentiality follows the `ConfidentialityRules` (issues 133, 134,
///   138): reading from another channel, giving a classification, joining
///   or reading a channel by clearance. The system overrides them, as it
///   overrides everything.
/// - A message may be put into a channel by the actor that is acting - its
///   author, who may release what they write - if the rules let it flow
///   there.
/// - A channel may be reclassified by its owner, a message by the channel's
///   owner or its author, a note or memory entry by its author - always to
///   a classification the rules let be given, and what is in a channel
///   never above it.
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
    rules: ConfidentialityRules,
    memberships: Arc<dyn Memberships>,
}

impl Default for OwnershipSecurityManager {
    fn default() -> Self {
        Self { rules: ConfidentialityRules::default(), memberships: Arc::new(NoMemberships) }
    }
}

impl OwnershipSecurityManager {
    /// The policy without compartments, without checking clearances and
    /// knowing of no memberships.
    pub fn new() -> Self {
        Self::default()
    }

    /// The policy, applying the confidentiality `rules` as configured and
    /// learning memberships and roles from `memberships`.
    pub fn with_rules(rules: ConfidentialityRules, memberships: Arc<dyn Memberships>) -> Self {
        Self { rules, memberships }
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
        let mut roles: Option<Vec<RoleId>> = None;
        for entry in attributes.acl.iter().filter(|entry| entry.grant.covers(needed)) {
            let matches = match entry.subject {
                Subject::Actor(actor) => actors.contains(&actor),
                Subject::MemberOf(place) => {
                    let mut member = false;
                    for actor in actors {
                        if self.memberships.is_member_of(*actor, place).await {
                            member = true;
                            break;
                        }
                    }
                    member
                }
                Subject::Role(role) => {
                    if roles.is_none() {
                        // The caller's: an agent its owner's, narrowed by
                        // its personality - a user their own.
                        let whose = if context.caller_id == ActorId::None { context.principal_id } else { context.caller_id };
                        roles = Some(self.memberships.roles_of(whose).await);
                    }
                    roles.as_ref().is_some_and(|roles| roles.contains(&role))
                }
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
        match action {
            // Reading: what the principal may read, or the caller itself -
            // an agent what it takes part in (issue 264).
            ActionType::GetPersonality(a)
            | ActionType::GetSessionTemplate(a)
            | ActionType::GetAgent(a)
            | ActionType::GetSession(a)
            | ActionType::GetChannel(a)
            | ActionType::GetComponents(a)
            | ActionType::GetUser(a)
            | ActionType::GetUserProfile(a)
            | ActionType::GetWorkspace(a)
            | ActionType::GetArtifact(a)
            | ActionType::GetSkill(a) => self.granted(a, Grant::Read, context, true).await,
            // Using a skill is granted as reading it is: what the owner may
            // read the agent may follow (issue 22).
            ActionType::UseSkill(a) => self.granted(a, Grant::Read, context, true).await,
            // Changing a workspace's files: what its list grants writing
            // (issue 23) - to the principal, or an agent taking part itself.
            ActionType::WriteWorkspace(a) => self.granted(a, Grant::Write, context, true).await,
            // Adding a version to an artifact, likewise (issue 23).
            ActionType::WriteArtifact(a) => self.granted(a, Grant::Write, context, true).await,
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
            ActionType::CopyInformation { from, into } => self.rules.may_copy(from, into),
            ActionType::Classify(classification) => self.rules.may_classify(classification),
            ActionType::Reclassify { target, new, channel, .. } => {
                let owns = |attributes: &SecurityAttributes| attributes.owner_id == me && me != ActorId::None;
                match target {
                    Reclassified::Channel { attributes } => owns(attributes) && self.rules.may_classify(new),
                    Reclassified::Message { author, attributes } => (owns(attributes) || author == context.caller_id) && self.rules.may_publish(new, channel),
                    Reclassified::Note { author } | Reclassified::Memory { author } => author == context.caller_id && self.rules.may_publish(new, channel),
                }
            }
            ActionType::Publish { classification, actor, channel } => actor == context.caller_id && self.rules.may_publish(classification, channel),
            // A message is its author's to change: who said it, not whom
            // they acted for.
            ActionType::ModifyMessage { author, .. } | ActionType::RevokeMessage { author, .. } => author == context.caller_id && author != ActorId::None,
            ActionType::JoinChannel { roles, channel, .. } => self.rules.cleared(roles, channel),
            ActionType::CreatePersonality(a)
            | ActionType::CreateSessionTemplate(a)
            | ActionType::CreateAgent(a)
            | ActionType::CreateSession(a)
            | ActionType::CreateChannel(a)
            | ActionType::CreateUser(a)
            | ActionType::CreateUserProfile(a)
            | ActionType::CreateWorkspace(a)
            | ActionType::CreateSkill(a)
            | ActionType::CreateArtifact(a)
            | ActionType::CreateComponents(a) => a.owner_id == me && me != ActorId::None,
            ActionType::UpdatePersonality(a)
            | ActionType::DeletePersonality(a)
            | ActionType::UpdateSessionTemplate(a)
            | ActionType::DeleteSessionTemplate(a)
            | ActionType::UpdateAgent(a)
            | ActionType::DeleteAgent(a)
            | ActionType::ReadAgentContext(a)
            | ActionType::ReadAgentComponents(a)
            | ActionType::UpdateSession(a)
            | ActionType::DeleteSession(a)
            | ActionType::StopSession(a)
            | ActionType::UpdateChannel(a)
            | ActionType::DeleteChannel(a)
            | ActionType::UpdateUser(a)
            | ActionType::DeleteUser(a)
            | ActionType::UpdateUserProfile(a)
            | ActionType::DeleteUserProfile(a)
            | ActionType::UpdateWorkspace(a)
            | ActionType::DeleteWorkspace(a)
            | ActionType::UpdateSkill(a)
            | ActionType::DeleteSkill(a)
            | ActionType::UpdateArtifact(a)
            | ActionType::DeleteArtifact(a)
            | ActionType::UpdateComponents(a)
            | ActionType::DeleteComponents(a)
            | ActionType::ReadDeleted(_, a)
            | ActionType::Restore(_, a) => a.owner_id == me && me != ActorId::None,
            // Until there is an administrator (issue 125): the system
            // alone, as the retention policy (issue 07) acts.
            ActionType::Purge(..) => false,
            ActionType::ListPersonalities
            | ActionType::ListSessionTemplates
            | ActionType::ListAgents
            | ActionType::ListSessions
            | ActionType::ListChannels
            | ActionType::ListUsers
            | ActionType::ListUserProfiles
            | ActionType::ListWorkspaces
            | ActionType::ListSkills
            | ActionType::ListArtifacts
            | ActionType::ListComponents => true,
            ActionType::ShowCommand(_)
            | ActionType::ExecuteCommand(_)
            | ActionType::CallTool(_)
            | ActionType::ReadSecret(_) => true,
        }
    }

    fn role_id(&self, name: &str) -> Option<RoleId> {
        self.rules.roles()?.id_of(name).ok()
    }

    fn role_name(&self, id: RoleId) -> Option<String> {
        self.rules.roles()?.name_of(id).map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidentiality::{Compartment, Compartments, Confidentiality};
    use crate::privacy::PrivacyConfig;
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

    /// Memberships and roles as a test sets them.
    #[derive(Debug, Default)]
    struct Known {
        members: Vec<(ActorId, Membership)>,
        roles: Vec<(ActorId, RoleId)>,
    }

    #[async_trait]
    impl Memberships for Known {
        async fn is_member_of(&self, actor: ActorId, place: Membership) -> bool {
            self.members.contains(&(actor, place))
        }

        async fn roles_of(&self, actor: ActorId) -> Vec<RoleId> {
            self.roles.iter().filter(|(a, _)| *a == actor).map(|(_, role)| *role).collect()
        }
    }

    fn knowing(known: Known) -> OwnershipSecurityManager {
        OwnershipSecurityManager::with_rules(ConfidentialityRules::default(), Arc::new(known))
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
        assert!(policy.may_revoke_message(&for_alice, agent, &bobs).await, "an agent its own, acting for alice");
        assert!(!policy.may_revoke_message(&for_alice, alice, &bobs).await, "but not alice's: who said it, not whom it acts for");
    }

    #[tokio::test]
    async fn an_agent_is_judged_by_its_owner_not_by_whoever_asked_it() {
        let (alice, bob) = (user(), user());
        let agent = ActorId::Agent(crate::agent::AgentId::new());
        let hers = SecurityAttributes::owned_by(alice);
        let policy = OwnershipSecurityManager::new();
        let asked_by_bob = context(bob).next(agent, alice);
        assert!(policy.check(&asked_by_bob, ActionType::GetSession(&hers)).await, "with alice's rights, whoever asked");
        assert!(policy.check(&asked_by_bob, ActionType::UpdateSession(&hers)).await, "all of them");
        let asked_by_alice = context(alice).next(agent, bob);
        assert!(!policy.check(&asked_by_alice, ActionType::GetSession(&hers)).await, "asking lends nothing, not even the owner's own rights to another's agent");
    }

    #[tokio::test]
    async fn access_questions_ask_the_policy_and_a_channel_always_reads_itself() {
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
        assert!(DenyAll.may_copy_information(&context(alice), &own, &own).await, "a channel is copied into itself, whatever the policy");
        assert!(!DenyAll.may_copy_information(&context(alice), &other, &own).await, "another channel only as the policy decides");
    }

    #[tokio::test]
    async fn owners_do_everything_whom_the_list_grants_read_strangers_nothing() {
        let (alice, bob, carol) = (user(), user(), user());
        let hers = shared(alice, &[bob]);

        for action in [ActionType::GetSession(&hers), ActionType::UpdateSession(&hers), ActionType::DeleteSession(&hers)] {
            assert!(allows(alice, action).await, "{action:?}");
            assert!(!allows(carol, action).await, "{action:?}");
        }
        assert!(allows(bob, ActionType::GetSession(&hers)).await);
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
        assert!(!allows(bob, ActionType::UpdateSession(&hers)).await);
        assert!(!allows(bob, ActionType::DeleteSession(&hers)).await);
        // A skill (issue 22) is used as it is read; changed by its owner.
        assert!(allows(bob, ActionType::UseSkill(&hers)).await, "a reader uses it");
        assert!(!allows(carol, ActionType::UseSkill(&hers)).await, "nobody else");
        assert!(allows(carol, ActionType::UseSkill(&systems)).await, "the system's, everybody");
        assert!(allows(bob, ActionType::GetSkill(&hers)).await && !allows(bob, ActionType::UpdateSkill(&hers)).await, "read, not changed");
    }

    /// A channel of the session `session_id`, a private one of nobody.
    fn channel(session_id: SessionId, level: Confidentiality, compartments: &[&str]) -> Place {
        in_session(SessionPrivacy { id: session_id, ..SessionPrivacy::default() }, level, compartments)
    }

    fn in_session(session: SessionPrivacy, level: Confidentiality, compartments: &[&str]) -> Place {
        let classification = Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap();
        Place { channel: ChannelId::new(), classification, session }
    }

    /// Copying what `from` holds, at the classification it names, into
    /// `into`, as the reading agent knows it itself: by the rules alone.
    fn copy<'a>(from: &'a Place, into: &'a Place) -> ActionType<'a> {
        ActionType::CopyInformation { from, into }
    }

    /// What `source` holds classified `classification`.
    fn released(source: &Place, classification: &Classification) -> Place {
        Place { classification: classification.clone(), ..source.clone() }
    }

    fn hr_reads_finance() -> OwnershipSecurityManager {
        let compartments = Compartments::new([
            (Compartment::new("hr"), vec![Compartment::new("finance")]),
            (Compartment::new("finance"), vec![]),
        ]);
        OwnershipSecurityManager::with_rules(ConfidentialityRules::new(compartments.unwrap()), Arc::new(NoMemberships))
    }

    #[tokio::test]
    async fn reading_a_channel_depends_on_the_channels_not_on_the_reader() {
        let session = SessionId::new();
        let (team, secret) = (channel(session, Confidentiality::Internal, &[]), channel(session, Confidentiality::Confidential, &[]));
        for actor in [user(), user()] {
            assert!(allows(actor, copy(&team, &secret)).await);
            assert!(!allows(actor, copy(&secret, &team)).await);
        }
    }

    #[tokio::test]
    async fn an_agent_reads_what_its_owner_may_read_or_it_takes_part_in_and_changes_as_its_owner() {
        let (alice, bob) = (user(), user());
        let (hers, his, member) = (ActorId::Agent(crate::agent::AgentId::new()), ActorId::Agent(crate::agent::AgentId::new()), ActorId::Agent(crate::agent::AgentId::new()));
        let place = Membership::Channel(ChannelId::new());
        let alices = SecurityAttributes::owned_by(alice).granting(Subject::MemberOf(place), Grant::Read);
        let policy = knowing(Known { members: vec![(member, place)], ..Known::default() });
        assert!(policy.check(&context(bob).next(hers, alice), ActionType::GetChannel(&alices)).await, "her agent, whoever asked");
        assert!(!policy.check(&context(bob).next(his, bob), ActionType::GetChannel(&alices)).await, "not his: he may not read it");
        assert!(policy.check(&context(bob).next(member, bob), ActionType::GetChannel(&alices)).await, "but an agent taking part in it");
        assert!(policy.check(&context(bob).next(member, bob), ActionType::GetSession(&alices)).await, "so for every read");
        assert!(!policy.check(&context(bob).next(member, bob), ActionType::UpdateChannel(&alices)).await, "changing stays its owner's");
        assert!(!policy.check(&context(bob).next(member, bob), ActionType::CancelChannel(&alices)).await, "and so does acting on it");
    }

    #[tokio::test]
    async fn reading_a_channel_follows_the_flow_rule_with_the_configured_compartments() {
        let (policy, session, alice) = (hr_reads_finance(), SessionId::new(), context(user()));
        let (hr, finance, general) = (
            channel(session, Confidentiality::Internal, &["hr"]),
            channel(session, Confidentiality::Internal, &["finance"]),
            channel(session, Confidentiality::Internal, &[]),
        );
        assert!(policy.check(&alice, copy(&finance, &hr)).await, "HR reads finance");
        assert!(!policy.check(&alice, copy(&hr, &finance)).await, "not the other way round");
        assert!(!policy.check(&alice, copy(&hr, &general)).await, "nor a general channel");
        assert!(policy.check(&alice, copy(&general, &hr)).await, "what is general flows anywhere");
    }

    #[tokio::test]
    async fn a_channel_reads_another_session_only_where_that_session_permits_it() {
        let (alice, bob) = (UserId::new(), UserId::new());
        let hers = |policy, others: &[UserId]| SessionPrivacy {
            id: SessionId::new(),
            policy,
            owner: ActorId::User(alice),
            people: others.iter().copied().chain([alice]).collect(),
        };
        let reader = in_session(hers(PrivacyConfig::Private, &[]), Confidentiality::Internal, &[]);
        let elsewhere = |policy, level| in_session(hers(policy, &[bob]), level, &[]);
        let agent = ActorId::Agent(crate::agent::AgentId::new());

        assert!(!allows(agent, copy(&elsewhere(PrivacyConfig::Private, Confidentiality::Internal), &reader)).await, "a private session is read by no other");
        assert!(!allows(agent, copy(&elsewhere(PrivacyConfig::Private, Confidentiality::Public), &reader)).await, "not even what is public");
        assert!(allows(agent, copy(&elsewhere(PrivacyConfig::SameOwner, Confidentiality::Internal), &reader)).await, "her other sessions, where she lets them");
        assert!(allows(agent, copy(&elsewhere(PrivacyConfig::SharedAudience, Confidentiality::Internal), &reader)).await, "she took part there");
        assert!(!allows(agent, copy(&elsewhere(PrivacyConfig::SameOwner, Confidentiality::Confidential), &reader)).await, "and still only what flows into the reader");
    }

    #[tokio::test]
    async fn what_is_released_below_its_channel_is_read_where_the_channel_is_not() {
        let (policy, session, alice) = (hr_reads_finance(), SessionId::new(), context(user()));
        let (hr, finance) = (channel(session, Confidentiality::Confidential, &["hr"]), channel(session, Confidentiality::Confidential, &["finance"]));
        let released = Classification::new(Confidentiality::Confidential, [Compartment::new("hr"), Compartment::new("finance")]).unwrap();
        assert!(!policy.check(&alice, copy(&hr, &finance)).await, "finance does not read HR");
        let released = self::released(&hr, &released);
        assert!(policy.check(&alice, copy(&released, &finance)).await, "but what HR released to it");
        let elsewhere = channel(SessionId::new(), Confidentiality::Confidential, &["finance"]);
        assert!(!policy.check(&alice, copy(&released, &elsewhere)).await, "not from another session");
    }

    #[tokio::test]
    async fn an_author_puts_into_a_channel_what_may_flow_into_it() {
        let (policy, session) = (hr_reads_finance(), SessionId::new());
        let alice = user();
        let hr = channel(session, Confidentiality::Confidential, &["hr"]);
        let classified = |level, c: &[&str]| Classification::new(level, c.iter().map(|c| Compartment::new(*c))).unwrap();
        let (policy, hr) = (&policy, &hr);
        let publish = |classification| async move {
            policy.check(&context(alice), ActionType::Publish { classification: &classification, actor: alice, channel: &hr.classification }).await
        };
        assert!(publish(classified(Confidentiality::Confidential, &["hr"])).await, "the channel's own");
        assert!(publish(classified(Confidentiality::Internal, &["hr", "finance"])).await, "released to finance, at a lower level");
        assert!(!publish(classified(Confidentiality::StrictlyConfidential, &["hr"])).await, "above the channel");
        assert!(publish(classified(Confidentiality::Confidential, &["finance"])).await, "HR may read what is finance's");
        assert!(publish(Classification::general(Confidentiality::Confidential)).await, "what is general flows into any channel");
        assert!(!publish(classified(Confidentiality::Confidential, &["hr", "payroll"])).await, "not configured");
        let bob = user();
        let own = classified(Confidentiality::Confidential, &["hr"]);
        assert!(!policy.check(&context(bob), ActionType::Publish { classification: &own, actor: alice, channel: &hr.classification }).await, "only in one's own name");
    }

    #[tokio::test]
    async fn a_channel_is_reclassified_by_its_owner_a_message_also_by_its_author() {
        let policy = hr_reads_finance();
        let (owner, author, stranger) = (user(), user(), user());
        // Leaked, so that the targets can be built outside the closure.
        let attributes: &'static SecurityAttributes = Box::leak(Box::new(shared(owner, &[author, stranger])));
        let channel_itself = Reclassified::Channel { attributes };
        let hr = channel(SessionId::new(), Confidentiality::Confidential, &["hr"]);
        let classified = |level, c: &[&str]| Classification::new(level, c.iter().map(|c| Compartment::new(*c))).unwrap();
        let current = hr.classification.clone();
        let reclassify = |who: ActorId, target: Reclassified<'static>, new: Classification| {
            let (policy, hr, current) = (&policy, &hr, &current);
            async move {
                policy.check(&context(who), ActionType::Reclassify { target, current, new: &new, channel: &hr.classification }).await
            }
        };
        let internal = classified(Confidentiality::Internal, &["hr"]);
        assert!(reclassify(owner, channel_itself, internal.clone()).await, "the owner releases the channel");
        assert!(!reclassify(author, channel_itself, internal.clone()).await, "a participant does not");
        assert!(!reclassify(owner, channel_itself, classified(Confidentiality::Internal, &["payroll"])).await, "not configured");

        let message = Reclassified::Message { author, attributes };
        let released = classified(Confidentiality::Internal, &["hr", "finance"]);
        assert!(reclassify(author, message, released.clone()).await, "the author releases their message");
        assert!(reclassify(owner, message, released.clone()).await, "so does the channel's owner");
        assert!(!reclassify(stranger, message, released).await, "nobody else");
        assert!(!reclassify(author, message, classified(Confidentiality::StrictlyConfidential, &["hr"])).await, "never above the channel");

        let agent = ActorId::Agent(crate::agent::AgentId::new());
        for own in [Reclassified::Note { author: agent }, Reclassified::Memory { author: agent }] {
            assert!(reclassify(agent, own, classified(Confidentiality::Internal, &["hr", "finance"])).await, "an agent releases what it wrote");
            assert!(!reclassify(owner, own, classified(Confidentiality::Internal, &["hr", "finance"])).await, "not even the channel's owner another's");
            assert!(!reclassify(agent, own, classified(Confidentiality::StrictlyConfidential, &["hr"])).await, "never above the channel");
        }
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

    #[tokio::test]
    async fn members_of_a_place_read_while_they_are_members_wherever_they_stand() {
        let (alice, bob, carol) = (user(), user(), user());
        let place = Membership::Session(SessionId::new());
        let hers = SecurityAttributes::owned_by(alice).granting(Subject::MemberOf(place), Grant::Read);
        let policy = knowing(Known { members: vec![(bob, place)], ..Known::default() });
        let elsewhere = SecurityContext::new(bob, SessionId::new(), ChannelId::new());
        assert!(policy.check(&elsewhere, ActionType::GetSession(&hers)).await, "a member, though he acts somewhere else");
        assert!(!policy.check(&context(carol), ActionType::GetSession(&hers)).await, "not who is no member");
        assert!(!policy.check(&context(bob), ActionType::UpdateSession(&hers)).await, "reading, not changing");
        let other = SecurityAttributes::owned_by(alice).granting(Subject::MemberOf(Membership::Session(SessionId::new())), Grant::Read);
        assert!(!policy.check(&context(bob), ActionType::GetSession(&other)).await, "a member of the place named, not of any");
    }

    #[tokio::test]
    async fn a_role_grants_to_whoever_holds_it_an_agent_narrowed_by_its_personality() {
        let (alice, bob) = (user(), user());
        let (hr, staff) = (RoleId::new(), RoleId::new());
        let agent = ActorId::Agent(crate::agent::AgentId::new());
        let hers = SecurityAttributes::owned_by(alice).granting(Subject::Role(hr), Grant::Read);
        // Bob holds hr; his agent is limited to staff, so its roles are
        // what remains of his - which the memberships answer for it.
        let policy = knowing(Known { roles: vec![(bob, hr), (bob, staff), (agent, staff)], ..Known::default() });
        assert!(policy.check(&context(bob), ActionType::GetSkill(&hers)).await, "who holds the role");
        assert!(!policy.check(&context(bob).next(agent, bob), ActionType::GetSkill(&hers)).await, "not his agent, limited below it");
        let his = SecurityAttributes::owned_by(alice).granting(Subject::Actor(bob), Grant::Read);
        assert!(policy.check(&context(bob).next(agent, bob), ActionType::GetSkill(&his)).await, "what he is granted as a person, his agent is too");
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
        assert!(allows(alice, ActionType::WriteWorkspace(&theirs)).await, "its owner");
        assert!(allows(bob, ActionType::GetWorkspace(&theirs)).await && !allows(bob, ActionType::WriteWorkspace(&theirs)).await, "a reader reads");
        assert!(allows(carol, ActionType::GetWorkspace(&theirs)).await && allows(carol, ActionType::WriteWorkspace(&theirs)).await, "a writer both");
        assert!(!allows(carol, ActionType::UpdateWorkspace(&theirs)).await, "changing the workspace itself stays the owner's");
    }

    #[test]
    fn an_access_list_names_people_roles_and_members_never_an_agent() {
        let alice = user();
        let agent = ActorId::Agent(crate::agent::AgentId::new());
        assert_eq!(shared(alice, &[user()]).validate(), Ok(()));
        let place = Subject::MemberOf(Membership::Channel(ChannelId::new()));
        assert_eq!(SecurityAttributes::owned_by(alice).granting(Subject::Role(RoleId::new()), Grant::Read).granting(place, Grant::Write).validate(), Ok(()));
        for nobody in [agent, ActorId::System, ActorId::None] {
            assert!(SecurityAttributes::owned_by(alice).granting(Subject::Actor(nobody), Grant::Read).validate().is_err(), "not {nobody}");
        }
        let written: SecurityAttributes = serde_json::from_value(serde_json::json!({"owner_id": alice})).unwrap();
        assert!(written.acl.is_empty(), "the list may be left out where it is written by hand");
    }

    #[test]
    fn the_policy_translates_role_ids_and_names() {
        let staff = RoleId::new();
        let roles = Roles::new([(staff, "staff".to_string(), vec![])]).unwrap();
        let policy = OwnershipSecurityManager::with_rules(ConfidentialityRules::default().with_roles(roles.clone()), Arc::new(NoMemberships));
        assert_eq!((policy.role_id("staff"), policy.role_name(staff)), (Some(staff), Some("staff".to_string())));
        assert_eq!((policy.role_id("hr"), OwnershipSecurityManager::new().role_id("staff")), (None, None), "only what is configured");
        let dummy = DummySecurityManager::with_roles(Some(roles));
        assert_eq!(dummy.role_id("staff"), Some(staff), "the policy that allows everything translates too");
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

    /// A subject and a place as a query compares them (issue 293).
    #[test]
    fn a_subject_meets_exactly_the_subjects_equal_to_it() {
        let id = uuid::Uuid::new_v4();
        let places = [Membership::Session(SessionId(id)), Membership::Channel(ChannelId(id)), Membership::Channel(ChannelId::new())];
        crate::store::query::meets_exactly_its_equals(&places);
        crate::store::query::meets_exactly_its_equals(&[
            Subject::Actor(ActorId::User(crate::user::UserId(id))), Subject::Actor(ActorId::Agent(crate::agent::AgentId(id))),
            Subject::Actor(ActorId::System), Subject::Role(RoleId(id)), Subject::Role(RoleId::new()),
            Subject::MemberOf(places[0]), Subject::MemberOf(places[1]), Subject::MemberOf(places[2]),
        ]);
    }

}
