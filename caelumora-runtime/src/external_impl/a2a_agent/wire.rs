//! The protocol of A2A v1.0 over JSON-RPC as far as it is spoken here
//! (issue 210): what is read of an agent card, of a JSON-RPC response and
//! of what a task, a message or an update carries; and the constants of
//! the protocol.

use crate::external::{ExternalAgentError, ExternalAgentResult};
use serde::Deserialize;

/// The version of A2A spoken, sent as `A2A-Version` with every request.
pub(super) const A2A_VERSION: &str = "1.0";

/// Where an agent card is found, below the agent's base URL.
pub(super) const CARD_PATH: &str = ".well-known/agent-card.json";

/// The protocol binding used: JSON-RPC over HTTP.
pub(super) const JSONRPC_BINDING: &str = "JSONRPC";

/// The JSON-RPC error code of a task that cannot be cancelled any more.
pub(super) const TASK_NOT_CANCELABLE: i64 = -32002;

/// Whether `version` is one of A2A 1.x.
pub(super) fn is_version_1(version: &str) -> bool {
    version == "1" || version.starts_with("1.")
}

/// The text of `parts`, one part a line; what is no text is left out.
pub(super) fn text_of(parts: &[WirePart]) -> String {
    parts.iter().filter_map(|p| p.text.as_deref()).collect::<Vec<_>>().join("\n")
}

/// A JSON-RPC response.
#[derive(Debug, Deserialize)]
pub(super) struct RpcResponse<T> {
    pub(super) result: Option<T>,
    pub(super) error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RpcError {
    pub(super) code: i64,
    pub(super) message: String,
}

impl<T> RpcResponse<T> {
    pub(super) fn into_result(self, endpoint: &str) -> ExternalAgentResult<T> {
        match (self.result, self.error) {
            (_, Some(error)) => Err(ExternalAgentError::Rejected { endpoint: endpoint.to_string(), message: format!("{} ({})", error.message, error.code) }),
            (Some(result), None) => Ok(result),
            (None, None) => Err(ExternalAgentError::InvalidResponse { endpoint: endpoint.to_string(), reason: "neither a result nor an error".to_string() }),
        }
    }
}

/// The parts of an agent card that are read.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentCard {
    #[serde(default)]
    pub(super) supported_interfaces: Vec<AgentInterface>,
    #[serde(default)]
    pub(super) capabilities: AgentCapabilities,
}

impl AgentCard {
    /// What the card offers, for a refusal.
    pub(super) fn offered(&self) -> String {
        if self.supported_interfaces.is_empty() {
            return "nothing".to_string();
        }
        self.supported_interfaces.iter().map(|i| format!("{} {}", i.protocol_binding, i.protocol_version)).collect::<Vec<_>>().join(", ")
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentInterface {
    pub(super) url: String,
    #[serde(default)]
    pub(super) protocol_binding: String,
    #[serde(default)]
    pub(super) protocol_version: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentCapabilities {
    #[serde(default)]
    pub(super) streaming: Option<bool>,
}

/// One response of a stream, or the answer to `SendMessage`: exactly one
/// of its fields is set.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StreamResponse {
    pub(super) task: Option<WireTask>,
    pub(super) message: Option<WireMessage>,
    pub(super) status_update: Option<StatusUpdate>,
    pub(super) artifact_update: Option<ArtifactUpdate>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireMessage {
    #[serde(default)]
    pub(super) context_id: Option<String>,
    #[serde(default)]
    pub(super) parts: Vec<WirePart>,
}

impl WireMessage {
    pub(super) fn text(&self) -> String {
        text_of(&self.parts)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WirePart {
    #[serde(default)]
    pub(super) text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireTask {
    pub(super) id: String,
    #[serde(default)]
    pub(super) context_id: Option<String>,
    pub(super) status: WireStatus,
    #[serde(default)]
    pub(super) artifacts: Vec<WireArtifact>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireStatus {
    pub(super) state: WireTaskState,
    #[serde(default)]
    pub(super) message: Option<WireMessage>,
}

/// The states of a task that tell something; any other - submitted,
/// working, or one this version does not know - is `Other`.
#[derive(Debug, Deserialize)]
pub(super) enum WireTaskState {
    #[serde(rename = "TASK_STATE_COMPLETED")]
    Completed,
    #[serde(rename = "TASK_STATE_CANCELED")]
    Canceled,
    #[serde(rename = "TASK_STATE_FAILED")]
    Failed,
    #[serde(rename = "TASK_STATE_REJECTED")]
    Rejected,
    #[serde(rename = "TASK_STATE_INPUT_REQUIRED")]
    InputRequired,
    #[serde(rename = "TASK_STATE_AUTH_REQUIRED")]
    AuthRequired,
    #[serde(other)]
    Other,
}

impl WireTaskState {
    /// Whether the task still works: it has neither ended nor asks back.
    pub(super) fn works(&self) -> bool {
        matches!(self, WireTaskState::Other)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireArtifact {
    #[serde(default)]
    pub(super) artifact_id: String,
    #[serde(default)]
    pub(super) parts: Vec<WirePart>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StatusUpdate {
    #[serde(default)]
    pub(super) context_id: Option<String>,
    pub(super) status: WireStatus,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ArtifactUpdate {
    #[serde(default)]
    pub(super) context_id: Option<String>,
    pub(super) artifact: WireArtifact,
    #[serde(default)]
    pub(super) append: bool,
    #[serde(default)]
    pub(super) last_chunk: bool,
}
