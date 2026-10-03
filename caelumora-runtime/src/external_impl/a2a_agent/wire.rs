//! The protocol of A2A v1.0 over JSON-RPC as far as it is spoken here
//! (issue 210): the methods called and their parameters; what is read of
//! an agent card, of a JSON-RPC response and of what a task, a message or
//! an update carries; and the constants of the protocol.

use crate::external::{ExternalAgentError, ExternalAgentResult};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The version of A2A spoken, sent as `A2A-Version` with every request.
pub(super) const A2A_VERSION: &str = "1.0";

/// Where an agent card is found, below the agent's base URL.
pub(super) const CARD_PATH: &str = ".well-known/agent-card.json";

/// The protocol binding used: JSON-RPC over HTTP.
pub(super) const JSONRPC_BINDING: &str = "JSONRPC";

/// The JSON-RPC error code of a task that cannot be cancelled any more.
pub(super) const TASK_NOT_CANCELABLE: i64 = -32002;

/// The JSON-RPC methods called.
pub(super) mod method {
    pub(in super::super) const SEND_MESSAGE: &str = "SendMessage";
    pub(in super::super) const SEND_STREAMING_MESSAGE: &str = "SendStreamingMessage";
    pub(in super::super) const GET_TASK: &str = "GetTask";
    pub(in super::super) const SUBSCRIBE_TO_TASK: &str = "SubscribeToTask";
    pub(in super::super) const CANCEL_TASK: &str = "CancelTask";
}

/// Whether `version` is one of A2A 1.x.
pub(super) fn is_version_1(version: &str) -> bool {
    version == "1" || version.starts_with("1.")
}

/// The text of `parts`, one part a line; what is no text is left out.
pub(super) fn text_of(parts: &[WirePart]) -> String {
    parts.iter().filter_map(|p| p.text.as_deref()).collect::<Vec<_>>().join("\n")
}

/// A JSON-RPC request of `method` with `params`, under an id of its own.
#[derive(Debug, Serialize)]
pub(super) struct RpcRequest<'a, P> {
    jsonrpc: &'static str,
    id: String,
    method: &'a str,
    params: P,
}

impl<'a, P> RpcRequest<'a, P> {
    pub(super) fn new(method: &'a str, params: P) -> Self {
        Self { jsonrpc: "2.0", id: Uuid::new_v4().to_string(), method, params }
    }
}

/// The parameters of `SendMessage` and `SendStreamingMessage`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SendMessageParams {
    pub(super) message: WireMessage,
    pub(super) configuration: SendConfiguration,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SendConfiguration {
    /// The media types of the answer that are taken.
    pub(super) accepted_output_modes: Vec<String>,
    /// Whether the request is answered at once with the task, rather than
    /// held open until it ends or asks back; sent only if so.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(super) return_immediately: bool,
}

/// The parameters of `GetTask`, `SubscribeToTask` and `CancelTask`.
#[derive(Debug, Serialize)]
pub(super) struct TaskParams {
    pub(super) id: String,
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
    /// The result, or the error the agent answered with; the agent at
    /// `endpoint` is named if it answered with neither.
    pub(super) fn into_result(self, endpoint: &str) -> ExternalAgentResult<Result<T, RpcError>> {
        match (self.result, self.error) {
            (_, Some(error)) => Ok(Err(error)),
            (Some(result), None) => Ok(Ok(result)),
            (None, None) => Err(ExternalAgentError::InvalidResponse { endpoint: endpoint.to_string(), reason: "neither a result nor an error".to_string() }),
        }
    }
}

impl RpcError {
    /// The error of a request the agent at `endpoint` refused so.
    pub(super) fn rejected(self, endpoint: &str) -> ExternalAgentError {
        ExternalAgentError::Rejected { endpoint: endpoint.to_string(), message: format!("{} ({})", self.message, self.code) }
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

/// A message, sent or read: what is not set is left out when it is sent.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) role: Option<WireRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) context_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) task_id: Option<String>,
    #[serde(default)]
    pub(super) parts: Vec<WirePart>,
}

/// Who a message is from: the agent's user - we - or the agent; a role
/// this version does not know is read as unspecified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum WireRole {
    #[serde(rename = "ROLE_USER")]
    User,
    #[serde(rename = "ROLE_AGENT")]
    Agent,
    #[serde(rename = "ROLE_UNSPECIFIED", other)]
    Unspecified,
}

impl WireMessage {
    pub(super) fn text(&self) -> String {
        text_of(&self.parts)
    }
}

/// A part of a message or an artifact; only text is sent or read.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WirePart {
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn sent(value: impl Serialize) -> Value {
        serde_json::to_value(value).expect("json")
    }

    #[test]
    fn a_request_is_json_rpc_with_an_id_of_its_own() {
        let request = sent(RpcRequest::new(method::GET_TASK, TaskParams { id: "t-1".to_string() }));
        assert_eq!(request["jsonrpc"], "2.0");
        assert_eq!(request["method"], "GetTask");
        assert_eq!(request["params"], json!({ "id": "t-1" }));
        assert!(request["id"].as_str().is_some_and(|id| !id.is_empty()));
        assert_ne!(sent(RpcRequest::new(method::GET_TASK, ()))["id"], request["id"], "every request its own");
    }

    #[test]
    fn a_message_is_sent_as_written_by_hand_before_what_is_not_set_left_out() {
        let message = || WireMessage {
            message_id: Some("m-1".to_string()),
            role: Some(WireRole::User),
            parts: vec![WirePart { text: Some("alice: hello".to_string()) }],
            ..WireMessage::default()
        };
        let params = |return_immediately| SendMessageParams {
            message: message(),
            configuration: SendConfiguration { accepted_output_modes: vec!["text/plain".to_string()], return_immediately },
        };
        assert_eq!(sent(params(false)), json!({
            "message": { "messageId": "m-1", "role": "ROLE_USER", "parts": [{ "text": "alice: hello" }] },
            "configuration": { "acceptedOutputModes": ["text/plain"] },
        }), "streamed: no returnImmediately");
        assert_eq!(sent(params(true))["configuration"], json!({ "acceptedOutputModes": ["text/plain"], "returnImmediately": true }));

        let reply = sent(WireMessage { context_id: Some("c-1".to_string()), task_id: Some("t-1".to_string()), ..message() });
        assert_eq!((reply["contextId"].as_str(), reply["taskId"].as_str()), (Some("c-1"), Some("t-1")));
    }

    #[test]
    fn a_role_not_known_is_read_as_unspecified() {
        let read = |role: &str| serde_json::from_value::<WireMessage>(json!({ "role": role, "parts": [] })).expect("a message").role;
        assert_eq!(read("ROLE_AGENT"), Some(WireRole::Agent));
        assert_eq!(read("ROLE_USER"), Some(WireRole::User));
        assert_eq!(read("ROLE_SOMEONE_ELSE"), Some(WireRole::Unspecified));
    }
}
