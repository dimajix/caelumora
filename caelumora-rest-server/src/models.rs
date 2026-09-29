use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::Confidentiality;
use caelumora_model::message::Message;
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::template::{SessionTemplate, SessionTemplateId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub template_id: Uuid,
    pub name: Option<String>,
}

/// A session in a list, or as creating or renaming it answers.
#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub id: SessionId,
    pub name: String,
}

impl From<&Session> for SessionResponse {
    fn from(session: &Session) -> Self {
        Self { id: session.id, name: session.metadata.name.clone() }
    }
}

/// A session template, as a client offers it for a new session - its id,
/// name and description, not the template itself (issue 106).
#[derive(Debug, Serialize)]
pub struct TemplateResponse {
    pub id: SessionTemplateId,
    pub name: String,
    pub description: String,
}

impl From<&SessionTemplate> for TemplateResponse {
    fn from(template: &SessionTemplate) -> Self {
        Self { id: template.id, name: template.metadata.name.clone(), description: template.metadata.description.clone() }
    }
}

/// What kind of participant an actor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipantKind {
    User,
    Agent,
    Approver,
}

impl ParticipantKind {
    /// The kind of participant `id` is; `None` for an id that is none - the
    /// system, nobody.
    pub fn of(id: ActorId) -> Option<Self> {
        match id {
            ActorId::User(_) => Some(Self::User),
            ActorId::Agent(_) => Some(Self::Agent),
            ActorId::Approver(_) => Some(Self::Approver),
            _ => None,
        }
    }
}

/// Whether an event stream starts with a snapshot of the current state
/// (issue 34). Off by default, so a client that does not want one does not
/// pay for it.
#[derive(Debug, Default, Deserialize)]
pub struct StreamOptions {
    #[serde(default)]
    pub snapshot: bool,
}

/// The state of a channel when a stream connects: its members and the end of
/// its history. Older messages are fetched over REST - `has_more` says there
/// are some.
#[derive(Debug, Serialize)]
pub struct ChannelSnapshot {
    pub channel: ChannelResponse,
    pub messages: Vec<Message>,
    pub has_more: bool,
    /// The notes every agent of the channel published (issue 17); a change
    /// is announced as `AgentChannelStateChanged` on the event stream, and
    /// the notes reloaded from `GET …/channels/{cid}/notes` (issue 65).
    pub notes: Vec<PublishedNotesResponse>,
}

/// The notes one agent published in a channel.
#[derive(Debug, Serialize)]
pub struct PublishedNotesResponse {
    pub agent_id: ActorId,
    pub name: String,
    pub notes: Vec<caelumora_model::note::Note>,
}

impl From<caelumora_runtime::session_reader::PublishedNotes> for PublishedNotesResponse {
    fn from(published: caelumora_runtime::session_reader::PublishedNotes) -> Self {
        Self { agent_id: published.agent, name: published.name, notes: published.notes }
    }
}

/// The state of the system when a global stream connects: the sessions and
/// channels the user may read, as the REST endpoints decide it.
#[derive(Debug, Serialize)]
pub struct GlobalSnapshot {
    pub sessions: Vec<SessionSnapshot>,
}

/// A session with its channels: an entry of the global snapshot, and what
/// `GET /sessions/{id}` answers.
#[derive(Debug, Serialize)]
pub struct SessionSnapshot {
    pub id: SessionId,
    pub name: String,
    pub channels: Vec<ChannelResponse>,
    /// Everyone who takes part in the session - whom a new channel can be
    /// made of (issue 09).
    pub participants: Vec<ParticipantResponse>,
}

/// A participant of a session.
#[derive(Debug, Serialize)]
pub struct ParticipantResponse {
    pub id: ActorId,
    pub name: String,
    pub kind: ParticipantKind,
}

#[derive(Debug, Deserialize)]
pub struct RenameSessionRequest {
    pub name: String,
}

/// A participant of the session to add to a channel (issue 46), with how
/// much of the channel's history they may see - all of it unless asked.
#[derive(Debug, Deserialize)]
pub struct AddParticipantRequest {
    pub participant: ActorId,
    #[serde(default)]
    pub history: caelumora_model::channel::HistoryAccess,
}

/// A channel to create in a running session (issue 09).
#[derive(Debug, Deserialize)]
pub struct CreateChannelRequest {
    pub name: String,
    /// The moderator kind, e.g. `mentions_only`; the default if absent.
    pub moderator: Option<String>,
    /// Participants of the session to take part. The requesting user takes
    /// part anyway.
    #[serde(default)]
    pub participants: Vec<ActorId>,
    /// How far the channel's contents may travel (issue 61): `public`,
    /// `session` or `channel`; `session` if absent.
    pub confidentiality: Option<Confidentiality>,
}

#[derive(Debug, Serialize)]
pub struct ChannelResponse {
    pub id: ChannelId,
    pub name: String,
    /// All participants, in no particular order, with the handles they are
    /// `@`-mentioned by - used by the web UI for the member list, completion
    /// and to render the `mentions` of a message.
    pub members: Vec<ChannelMemberResponse>,
    /// The kind of moderator, e.g. `round_robin` or `mentions_only` - who is
    /// asked to respond to a message (issue 44).
    pub moderator: String,
    /// How far the channel's contents may travel (issue 61).
    pub confidentiality: Confidentiality,
    /// Whether the requesting user may rename the channel and add or remove
    /// its participants (issue 46).
    pub can_change: bool,
}

#[derive(Debug, Serialize)]
pub struct ChannelMemberResponse {
    pub id: ActorId,
    pub name: String,
    /// `None` if the participant cannot be mentioned.
    pub handle: Option<String>,
    pub kind: ParticipantKind,
    /// Whether the requesting user may see what this participant saw when it
    /// produced a message (issue 41) - only ever for agents, and only as the
    /// policy decides for this user.
    pub can_read_context: bool,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct SendApprovalRequest {
    pub approval_id: Uuid,
    pub agent_id: Uuid,
    pub approved: bool,
}

/// A user command to run in a channel (issue 28): the name without the slash
/// and the rest of the input line, which the command parses itself.
#[derive(Debug, Deserialize)]
pub struct ExecuteCommandRequest {
    pub name: String,
    #[serde(default)]
    pub args: String,
}

/// What a command answered. `output` is set only if the answer is for the
/// caller alone; an answer for the whole channel arrives on the channel's
/// event stream as a `CommandExecuted`, like everybody else sees it.
#[derive(Debug, Serialize)]
pub struct CommandReplyResponse {
    pub command: String,
    pub output: Option<String>,
}

/// What an agent's model saw when it produced a message (issues 06, 47):
/// the messages of its transcript and the prompt parts of that turn, in the
/// order the model was given them, with the digest of the context.
#[derive(Debug, Serialize)]
pub struct MessageContextResponse {
    pub message_id: caelumora_model::message::MessageId,
    pub actor_id: ActorId,
    pub transcript_digest: Option<caelumora_model::digest::Digest>,
    pub items: Vec<SeenItemResponse>,
    /// What was read from notes and histories before the message was
    /// produced (issue 17).
    pub knowledge: Vec<KnowledgeRefResponse>,
}

/// Knowledge a message was produced with, with the names of what it names
/// where they are still known.
#[derive(Debug, Serialize)]
pub struct KnowledgeRefResponse {
    pub channel_id: caelumora_model::session::ChannelId,
    pub channel_name: Option<String>,
    /// `"notes"`, `"private_notes"`, `"memory"` or `"history"`.
    pub source: &'static str,
    /// Whose notes, for `"notes"` and `"private_notes"`.
    pub agent_id: Option<ActorId>,
    pub agent_name: Option<String>,
    pub digest: Option<caelumora_model::digest::Digest>,
    /// For `"history"`: where the reader's view began, if they joined later
    /// with a limit (issue 72) - `{"from": id}` or `{"after": id}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_start: Option<caelumora_model::channel::HistoryStart>,
}

/// One element of a `MessageContextResponse`.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SeenItemResponse {
    /// A prompt part - the system prompt, notes, ... - and the role it was
    /// given as.
    Prompt { digest: caelumora_model::digest::Digest, role: caelumora_model::message::MessageRole, text: String },
    Message { message: Message },
}

impl From<caelumora_model::message::ContextItem> for SeenItemResponse {
    fn from(item: caelumora_model::message::ContextItem) -> Self {
        match item {
            caelumora_model::message::ContextItem::Prompt { digest, role, text } => SeenItemResponse::Prompt { digest, role, text },
            caelumora_model::message::ContextItem::Conversation { message, .. } => SeenItemResponse::Message { message },
        }
    }
}

