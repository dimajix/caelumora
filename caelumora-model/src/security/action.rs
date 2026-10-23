//! What the `AccessPolicy` is asked about (issue 323): the actions, among
//! them the verbs on stored entities.

use crate::actor::ActorId;
use crate::command::Command;
use crate::store::EntityKind;
use crate::tool::ToolCall;
use std::fmt::Debug;
use super::{Grant, SecurityAttributes};

/// Everything the `AccessPolicy` is asked about.
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
    /// - a skill (issue 22, the plugin `caelumora-skills`): its name,
    ///   description and body; loading it into a turn is its verb `USE`,
    ///   apart from reading it.
    /// - an artifact (issue 271, the plugin `caelumora-artifacts`): the
    ///   artifact - its name, description and which versions it has -, or a
    ///   version's content, asked with the version's own list besides the
    ///   artifact's: reading a version needs both (issue 316), as a file in
    ///   HDFS needs its directory. Adding a version is `WRITE`.
    /// - a group (issue 286): who belongs to it, granted to its owner and,
    ///   by its own access list, to its participants. The participants of a
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
    /// bundles it includes resolved (issue 270): its
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
    /// in completion, in the participant's snapshot. Carries the agent's
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

    /// Being told that an entity of `kind` with `attributes` was deleted.
    pub const fn hear_deleted(kind: EntityKind, attributes: &'a SecurityAttributes) -> Self {
        Self::entity(kind, Verb::HearDeleted, attributes)
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
    /// `attributes`, such as using a skill (its verb `USE`).
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
    /// Being told that an entity was deleted (issue 332): allowed to whom
    /// reading it was until then, by the groups it was read by as well,
    /// though they went with it. An event's, not a store's.
    HearDeleted,
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
            Verb::HearDeleted => "HearDeleted",
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

/// Taking oneself into a session or a channel (issue 221) - what an access
/// list grants `Join`; whoever may change it takes in anyone without it.
/// Asked in the context of the one who joins.
pub const JOIN: CustomVerb = CustomVerb { name: "Join", requires: Requirement::Grant(Grant::Join) };

/// Changing the content of what is stored, beside reading it (issue 23) -
/// what an access list grants `Write`. A verb any kind may use, the core's
/// or a plugin's (issue 328), for what is written to rather than changed
/// as a whole: a workspace's files, say.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Stored;
    use crate::session::Session;

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
}
