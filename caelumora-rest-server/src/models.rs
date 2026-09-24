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

#[derive(Debug, Serialize)]
pub struct SessionSnapshot {
    pub id: SessionId,
    pub name: String,
    pub metadata: Metadata,
    pub channels: Vec<ChannelResponse>,
}

#[derive(Debug, Deserialize)]
pub struct RenameSessionRequest {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct ChannelResponse {
    pub id: ChannelId,
    pub name: String,
    /// All participants, in no particular order, with the handles they are
    /// `@`-mentioned by - used by the web UI for the member list, completion
    /// and to render the `mentions` of a message.
    pub members: Vec<ChannelMemberResponse>,
}

#[derive(Debug, Serialize)]
pub struct ChannelMemberResponse {
    pub id: ActorId,
    pub name: String,
    /// `None` if the participant cannot be mentioned.
    pub handle: Option<String>,
    pub kind: String,
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
