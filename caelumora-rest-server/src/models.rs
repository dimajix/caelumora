use caelumora_model::actor::ActorId;
use caelumora_model::message::Message;
use caelumora_model::meta::Metadata;
use caelumora_model::session::{ChannelId, SessionId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub template_id: Uuid,
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub id: SessionId,
    pub name: String,
    pub metadata: Metadata,
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
}

/// The state of the system when a global stream connects.
///
/// Unfiltered for now: every user sees every session, which is issue 11. The
/// snapshot must go through the same check as the REST endpoints once that
/// exists.
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
    pub metadata: Metadata,
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
    /// `"user"`, `"agent"` or `"approver"`.
    pub kind: String,
}

#[derive(Debug, Deserialize)]
pub struct RenameSessionRequest {
    pub name: String,
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
}

#[derive(Debug, Serialize)]
pub struct ChannelMemberResponse {
    pub id: ActorId,
    pub name: String,
    /// `None` if the participant cannot be mentioned.
    pub handle: Option<String>,
    pub kind: String,
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
/// the messages of its context and the prompt parts of that turn, in the
/// order the model was given them, with the digest of the context.
#[derive(Debug, Serialize)]
pub struct MessageContextResponse {
    pub message_id: caelumora_model::message::MessageId,
    pub actor_id: ActorId,
    pub context_digest: Option<caelumora_model::digest::Digest>,
    pub items: Vec<SeenItemResponse>,
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

impl From<caelumora_model::agent::SeenItem> for SeenItemResponse {
    fn from(item: caelumora_model::agent::SeenItem) -> Self {
        match item {
            caelumora_model::agent::SeenItem::Prompt { digest, role, text } => SeenItemResponse::Prompt { digest, role, text },
            caelumora_model::agent::SeenItem::Message(message) => SeenItemResponse::Message { message },
        }
    }
}

