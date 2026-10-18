//! What an agent saw and has (issues 06, 47, 194, 270): the context of a message, the
//! knowledge it was produced with, its tools and components.

use caelumora_model::chain::Position;
use caelumora_model::actor::ActorId;
use caelumora_model::component::ComponentConfig;
use caelumora_model::confidentiality::{Classification, LabelCauses};
use caelumora_model::context::ContextItem;
use caelumora_model::digest::Digest;
use caelumora_model::message::{Message, MessageId, MessageRole};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::tool::ToolSet;
use serde::Serialize;
use uuid::Uuid;

/// What an agent's model saw when it produced a message (issues 06, 47):
/// the messages of its transcript and the prompt parts of that turn, in the
/// order the model was given them, with the digest of the context, and the
/// tools it was offered (issue 137).
#[derive(Debug, Serialize)]
pub struct MessageContextResponse {
    pub message_id: MessageId,
    pub actor_id: ActorId,
    pub transcript_digest: Option<Digest>,
    pub items: Vec<SeenItemResponse>,
    /// What was read from notes and histories before the message was
    /// produced (issue 17).
    pub knowledge: Vec<KnowledgeRefResponse>,
    /// The digest of the tool sets the model was offered, `None` if it was
    /// offered none (issue 137).
    pub tools_digest: Option<Digest>,
    /// The tools, as the model was told of them, in their sets (issue 194).
    pub tool_sets: Vec<ToolSet>,
}

/// The tools an agent has now in a channel, in their tool sets (issue
/// 194), each tool with what the runtime acts on - whether a call needs
/// approval, who sees it, how its results are classified. None if it has
/// none here: it is no agent run here or has left - and also while the
/// channel does not run.
#[derive(Debug, Serialize)]
pub struct AgentToolsResponse {
    pub tool_sets: Vec<ToolSet>,
}

/// The components an agent was built from in a channel, the preconfigured
/// ones it includes resolved (issue 270), in their order. None if it is no
/// agent run here or has left - and also while the channel does not run.
#[derive(Debug, Serialize)]
pub struct AgentComponentsResponse {
    pub components: ComponentConfig,
}

/// Knowledge a message was produced with, with the names of what it names
/// where they are still known.
#[derive(Debug, Serialize)]
pub struct KnowledgeRefResponse {
    /// The session of the channel: another one's, for what an agent
    /// recalled from its memory (issue 134).
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    /// `None` for a channel that is gone, or of another session.
    pub channel_name: Option<String>,
    /// Whether the channel was deleted (issue 164) - told only to whoever
    /// could read it were it there; for anyone else, it never was.
    pub channel_deleted: bool,
    /// `"notes"`, `"private_notes"`, `"memory"`, `"history"`, or for an
    /// entity its kind (issue 323) - `"workspace"`, whose files were read,
    /// `"skill"`, loaded (issue 22).
    pub source: &'static str,
    /// The entity read, for a kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<Uuid>,
    /// Whose notes, for `"notes"` and `"private_notes"`.
    pub agent_id: Option<ActorId>,
    pub agent_name: Option<String>,
    pub digest: Option<Digest>,
    /// For `"history"`: the position the reader's view began at, if they
    /// joined later with a limit (issues 72, 417).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_start: Option<Position>,
    /// How what was read was classified when it was read, `null` if that
    /// is not known (issue 168).
    pub classification: Option<Classification>,
    /// Whether it set the label of the message (issue 170).
    pub sets_label: bool,
}


/// One element of a `MessageContextResponse`, with whether it set the
/// label of the message (issue 170).
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[expect(clippy::large_enum_variant, reason = "a message is the usual element, and boxing it would cost an allocation per element")]
pub enum SeenItemResponse {
    /// A prompt part - the system prompt, notes, ... - the role it was
    /// given as, and how it was classified, `null` if that is not known.
    Prompt {
        digest: Digest,
        role: MessageRole,
        text: String,
        classification: Option<Classification>,
        sets_label: bool,
    },
    Message { message: Message, sets_label: bool },
}

impl SeenItemResponse {
    /// `item`, with whether it set the label, which `causes` tells.
    pub fn of(item: ContextItem, causes: &LabelCauses) -> Self {
        match item {
            ContextItem::Prompt { digest, role, text, classification } => {
                let sets_label = causes.set_by(classification.as_ref());
                SeenItemResponse::Prompt { digest, role, text, classification, sets_label }
            }
            ContextItem::Conversation { message } => {
                let sets_label = causes.set_by_message(&message);
                SeenItemResponse::Message { message, sets_label }
            }
        }
    }
}
