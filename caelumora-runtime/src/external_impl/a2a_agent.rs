//! An external agent reached over the Agent2Agent protocol, A2A v1.0
//! (issue 179): its agent card says where and how to reach it; what is
//! said is sent as a message over JSON-RPC, and what comes back - a task,
//! its status, its artifacts, or a message of its own - is translated to
//! `ExternalEvent`s. Streams over server-sent events where the card offers
//! streaming, and waits for the task to end or ask back where it does not.
//! Only A2A 1.x over JSON-RPC: a card that offers nothing else is refused
//! when the agent is asked, not when the component is built.

use crate::component::{ComponentError, ComponentFacets, ComponentKind, ComponentRegistry};
use crate::external::{ExternalAgent, ExternalAgentError, ExternalAgentResult, ExternalEvent, ExternalStream, OutboundMessage};
use crate::llm::LLMError;
use crate::remote::RemoteAuthConfig;
use arc_swap::ArcSwapOption;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::external_agent::{ConversationId, TaskId};
use caelumora_model::secret::SecretManager;
use caelumora_model::security::SecurityContext;
use futures::StreamExt;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use sse_stream::SseStream;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, trace, warn};
use uuid::Uuid;

pub const A2A_AGENT_KIND: &str = "a2a_agent";

/// The version of A2A spoken, sent as `A2A-Version` with every request.
const A2A_VERSION: &str = "1.0";

/// Where an agent card is found, below the agent's base URL.
const CARD_PATH: &str = ".well-known/agent-card.json";

/// The protocol binding used: JSON-RPC over HTTP.
const JSONRPC_BINDING: &str = "JSONRPC";

/// The JSON-RPC error code of a task that cannot be cancelled any more.
const TASK_NOT_CANCELABLE: i64 = -32002;

fn default_timeout_secs() -> u64 {
    30
}

/// The parameters of an `a2a_agent`, written by hand.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct A2aAgentConfig {
    /// The agent's base URL, below which its card is published - or the
    /// card's own URL, ending in `.json`.
    url: String,
    #[serde(default)]
    auth: Option<RemoteAuthConfig>,
    /// How long one request may take to be answered - a streamed one until
    /// its stream begins; not how long a task may run.
    #[serde(default = "default_timeout_secs")]
    timeout_secs: u64,
    /// How the agent's answers are classified (issue 133); the channel's
    /// unless named.
    #[serde(default)]
    classification: Option<Classification>,
}

impl A2aAgentConfig {
    fn card_url(&self) -> String {
        if self.url.ends_with(".json") {
            self.url.clone()
        } else {
            format!("{}/{CARD_PATH}", self.url.trim_end_matches('/'))
        }
    }
}

/// What of the agent card is used: where to send requests, and whether
/// they are streamed.
#[derive(Debug, Clone)]
struct Endpoint {
    url: String,
    streaming: bool,
}

/// An agent reached over A2A, as the `ExternalAgent` of an external
/// personality.
#[derive(Debug)]
pub struct A2aAgentComponent {
    id: String,
    config: A2aAgentConfig,
    /// Read from the card when the component is built, or when the agent is
    /// first asked if that failed.
    endpoint: ArcSwapOption<Endpoint>,
    http: reqwest::Client,
    secret_manager: Arc<SecretManager>,
}

impl A2aAgentComponent {
    /// The component `id` with `config`, its card read if it can be, in
    /// `security`: an agent that cannot be reached yet does not keep its
    /// personality from being built.
    async fn build(id: String, config: A2aAgentConfig, secret_manager: Arc<SecretManager>, security: &SecurityContext) -> Result<Self, ComponentError> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| ComponentError::InvalidConfig(format!("no HTTP client for '{}': {e}", config.url)))?;
        let component = Self { id, config, endpoint: ArcSwapOption::empty(), http, secret_manager };
        if let Err(e) = component.endpoint(security).await {
            warn!("A2A agent component '{}' cannot read its agent card yet: {}", component.id, e);
        }
        Ok(component)
    }

    /// The URL of the agent, for messages: without credentials.
    fn shown(&self) -> String {
        LLMError::redact_endpoint(&self.config.url)
    }

    /// Where to reach the agent, read from its card the first time.
    async fn endpoint(&self, security: &SecurityContext) -> ExternalAgentResult<Endpoint> {
        if let Some(endpoint) = self.endpoint.load_full() {
            return Ok((*endpoint).clone());
        }
        let endpoint = self.discover(security).await?;
        self.endpoint.store(Some(Arc::new(endpoint.clone())));
        Ok(endpoint)
    }

    /// Reads the agent card and chooses its JSON-RPC interface of A2A 1.x.
    async fn discover(&self, security: &SecurityContext) -> ExternalAgentResult<Endpoint> {
        let mut request = self.http.get(self.config.card_url()).timeout(Duration::from_secs(self.config.timeout_secs));
        if let Some(token) = self.token(security).await? {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|e| self.connection(&e))?;
        let card: AgentCard = self.json_of(response).await?;
        let interface = card.supported_interfaces.iter()
            .find(|i| i.protocol_binding == JSONRPC_BINDING && is_version_1(&i.protocol_version))
            .ok_or_else(|| ExternalAgentError::Rejected {
                endpoint: self.shown(),
                message: format!("its agent card offers no JSON-RPC interface of A2A 1.x, only {}", card.offered()),
            })?;
        debug!("A2A agent component '{}' reaches its agent at {}", self.id, LLMError::redact_endpoint(&interface.url));
        Ok(Endpoint { url: interface.url.clone(), streaming: card.capabilities.streaming.unwrap_or(false) })
    }

    async fn token(&self, security: &SecurityContext) -> ExternalAgentResult<Option<String>> {
        RemoteAuthConfig::bearer_token(self.config.auth.as_ref(), &self.secret_manager, security).await
            .map_err(|source| ExternalAgentError::Credential { endpoint: self.shown(), source })
    }

    fn connection(&self, e: &reqwest::Error) -> ExternalAgentError {
        ExternalAgentError::Connection { endpoint: self.shown(), reason: LLMError::describe_chain(e) }
    }

    /// The body of `response` as `T`, if it answered successfully.
    async fn json_of<T: DeserializeOwned>(&self, response: reqwest::Response) -> ExternalAgentResult<T> {
        self.require_success(&response)?;
        let text = response.text().await.map_err(|e| self.connection(&e))?;
        serde_json::from_str(&text).map_err(|e| ExternalAgentError::InvalidResponse { endpoint: self.shown(), reason: e.to_string() })
    }

    /// Refuses `response` unless it answered successfully.
    fn require_success(&self, response: &reqwest::Response) -> ExternalAgentResult<()> {
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(ExternalAgentError::Rejected { endpoint: self.shown(), message: format!("HTTP {status}") })
        }
    }

    /// Posts the JSON-RPC request `method` with `params`, streamed or not:
    /// either is answered within the timeout, a streamed one with the
    /// beginning of its stream (issue 186).
    async fn post(&self, security: &SecurityContext, endpoint: &Endpoint, method: &str, params: Value, streamed: bool) -> ExternalAgentResult<reqwest::Response> {
        let body = json!({ "jsonrpc": "2.0", "id": Uuid::new_v4().to_string(), "method": method, "params": params });
        trace!("A2A agent component '{}' sends {}: {}", self.id, method, body);
        let mut request = self.http.post(&endpoint.url).header("A2A-Version", A2A_VERSION).json(&body);
        request = if streamed {
            request.header(reqwest::header::ACCEPT, "text/event-stream")
        } else {
            request.timeout(Duration::from_secs(self.config.timeout_secs))
        };
        if let Some(token) = self.token(security).await? {
            request = request.bearer_auth(token);
        }
        let timeout = Duration::from_secs(self.config.timeout_secs);
        match tokio::time::timeout(timeout, request.send()).await {
            Ok(response) => response.map_err(|e| self.connection(&e)),
            Err(_) => Err(ExternalAgentError::Connection { endpoint: self.shown(), reason: format!("no answer within {} s", self.config.timeout_secs) }),
        }
    }

    /// The result of the JSON-RPC request `method`.
    async fn call<T: DeserializeOwned>(&self, security: &SecurityContext, endpoint: &Endpoint, method: &str, params: Value) -> ExternalAgentResult<T> {
        let response = self.post(security, endpoint, method, params, false).await?;
        let answer: RpcResponse<T> = self.json_of(response).await?;
        answer.into_result(&self.shown())
    }

    /// The events of the streamed JSON-RPC request `method`.
    async fn stream(&self, security: &SecurityContext, endpoint: &Endpoint, method: &str, params: Value) -> ExternalAgentResult<ExternalStream> {
        let response = self.post(security, endpoint, method, params, true).await?;
        self.require_success(&response)?;
        let is_stream = response.headers().get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        if !is_stream {
            // Refused before it began: a JSON-RPC error, not a stream.
            let answer: RpcResponse<StreamResponse> = self.json_of(response).await?;
            return Ok(answered(answer.into_result(&self.shown())?));
        }
        Ok(Box::pin(events_of(response.bytes_stream(), self.shown())))
    }
}

/// Whether `version` is one of A2A 1.x.
fn is_version_1(version: &str) -> bool {
    version == "1" || version.starts_with("1.")
}

/// The events of a single `response`, as a stream.
fn answered(response: StreamResponse) -> ExternalStream {
    Box::pin(futures::stream::iter(Translation::default().events(response).into_iter().map(Ok)))
}

/// The events of a stream of server-sent events, each a JSON-RPC response
/// carrying a `StreamResponse`. Ends with the stream, or after an error.
/// The events are framed in bytes, so a character or a line end the
/// network splits arrives whole (issue 185).
fn events_of<S, D, E>(bytes: S, endpoint: String) -> impl futures::Stream<Item = ExternalAgentResult<ExternalEvent>> + Send + 'static
where
    S: futures::Stream<Item = Result<D, E>> + Send + 'static,
    D: bytes::Buf + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    async_stream::stream! {
        let mut sse = std::pin::pin!(SseStream::from_bytes_stream(bytes));
        let mut translation = Translation::default();
        while let Some(event) = sse.next().await {
            let data = match event {
                Ok(event) => event.data.unwrap_or_default(),
                Err(sse_stream::Error::Body(e)) => {
                    yield Err(ExternalAgentError::Connection { endpoint: endpoint.clone(), reason: LLMError::describe_chain(e.as_ref()) });
                    return;
                }
                Err(e) => {
                    yield Err(ExternalAgentError::InvalidResponse { endpoint: endpoint.clone(), reason: e.to_string() });
                    return;
                }
            };
            if data.is_empty() {
                continue;
            }
            let answer = serde_json::from_str::<RpcResponse<StreamResponse>>(&data)
                .map_err(|e| ExternalAgentError::InvalidResponse { endpoint: endpoint.clone(), reason: e.to_string() })
                .and_then(|answer| answer.into_result(&endpoint));
            match answer {
                Ok(response) => {
                    for event in translation.events(response) {
                        yield Ok(event);
                    }
                }
                Err(e) => {
                    yield Err(e);
                    return;
                }
            }
        }
    }
}

#[async_trait]
impl ExternalAgent for A2aAgentComponent {
    async fn send(&self, security: &SecurityContext, conversation: Option<&ConversationId>, message: OutboundMessage) -> ExternalAgentResult<ExternalStream> {
        let endpoint = self.endpoint(security).await?;
        // What each said, as the remote agent's user: by name, one part each.
        let parts: Vec<Value> = message.said.iter().map(|s| json!({ "text": format!("{}: {}", s.sender, s.text) })).collect();
        let mut wire = json!({ "messageId": Uuid::new_v4().to_string(), "role": "ROLE_USER", "parts": parts });
        if let Some(conversation) = conversation {
            wire["contextId"] = json!(conversation.0);
        }
        if let Some(task) = &message.answers {
            wire["taskId"] = json!(task.0);
        }
        let params = json!({ "message": wire, "configuration": { "acceptedOutputModes": ["text/plain"] } });
        if endpoint.streaming {
            self.stream(security, &endpoint, "SendStreamingMessage", params).await
        } else {
            let response: StreamResponse = self.call(security, &endpoint, "SendMessage", params).await?;
            Ok(answered(response))
        }
    }

    async fn resume(&self, security: &SecurityContext, task: &TaskId) -> ExternalAgentResult<ExternalStream> {
        let endpoint = self.endpoint(security).await?;
        let params = json!({ "id": task.0 });
        if endpoint.streaming {
            self.stream(security, &endpoint, "SubscribeToTask", params).await
        } else {
            // Where it stands now; a task still working ends the stream
            // without an end, and is asked again on the next run.
            let found: WireTask = self.call(security, &endpoint, "GetTask", params).await?;
            Ok(answered(StreamResponse { task: Some(found), ..StreamResponse::default() }))
        }
    }

    async fn cancel(&self, security: &SecurityContext, task: &TaskId) -> ExternalAgentResult<()> {
        let endpoint = self.endpoint(security).await?;
        let response = self.post(security, &endpoint, "CancelTask", json!({ "id": task.0 }), false).await?;
        let answer: RpcResponse<Value> = self.json_of(response).await?;
        match answer.error {
            Some(error) if error.code == TASK_NOT_CANCELABLE => Ok(()),
            Some(error) => Err(ExternalAgentError::Rejected { endpoint: self.shown(), message: error.message }),
            None => Ok(()),
        }
    }

    fn classification(&self) -> Option<Classification> {
        self.config.classification.clone()
    }
}

/// What a stream's responses become, as `ExternalEvent`s: keeps what it
/// told already - the conversation - and the pieces of artifacts sent in
/// chunks.
#[derive(Debug, Default)]
struct Translation {
    conversation: Option<String>,
    artifacts: HashMap<String, String>,
}

impl Translation {
    fn events(&mut self, response: StreamResponse) -> Vec<ExternalEvent> {
        let mut events = Vec::new();
        if let Some(message) = response.message {
            // An answer without a task: it is the whole of it.
            self.conversation(message.context_id.as_deref(), &mut events);
            let text = message.text();
            if !text.is_empty() {
                events.push(ExternalEvent::Message(text));
            }
            events.push(ExternalEvent::Completed);
        }
        if let Some(task) = response.task {
            self.conversation(task.context_id.as_deref(), &mut events);
            events.push(ExternalEvent::Started(TaskId(task.id)));
            for artifact in task.artifacts {
                let text = text_of(&artifact.parts);
                if !text.is_empty() {
                    events.push(ExternalEvent::Message(text));
                }
            }
            status_events(task.status, &mut events);
        }
        if let Some(update) = response.status_update {
            self.conversation(update.context_id.as_deref(), &mut events);
            status_events(update.status, &mut events);
        }
        if let Some(update) = response.artifact_update {
            self.conversation(update.context_id.as_deref(), &mut events);
            let piece = text_of(&update.artifact.parts);
            let whole = self.artifacts.entry(update.artifact.artifact_id.clone()).or_default();
            if !update.append {
                whole.clear();
            }
            whole.push_str(&piece);
            if update.last_chunk {
                let whole = self.artifacts.remove(&update.artifact.artifact_id).unwrap_or_default();
                if !whole.is_empty() {
                    events.push(ExternalEvent::Message(whole));
                }
            } else if !piece.is_empty() {
                events.push(ExternalEvent::Partial(piece));
            }
        }
        events
    }

    /// The conversation, told once - and again if it changes.
    fn conversation(&mut self, context: Option<&str>, events: &mut Vec<ExternalEvent>) {
        if let Some(context) = context.filter(|c| !c.is_empty()) {
            if self.conversation.as_deref() != Some(context) {
                self.conversation = Some(context.to_string());
                events.push(ExternalEvent::Conversation(ConversationId(context.to_string())));
            }
        }
    }
}

/// What a task's `status` tells: nothing while it works; its end, its
/// question, or why it failed.
fn status_events(status: WireStatus, events: &mut Vec<ExternalEvent>) {
    let text = status.message.as_ref().map(WireMessage::text).unwrap_or_default();
    let or = |fallback: &str| if text.is_empty() { fallback.to_string() } else { text.clone() };
    match status.state {
        WireTaskState::Completed => {
            if !text.is_empty() {
                events.push(ExternalEvent::Message(text.clone()));
            }
            events.push(ExternalEvent::Completed);
        }
        WireTaskState::Canceled => events.push(ExternalEvent::Canceled),
        WireTaskState::Failed => events.push(ExternalEvent::Failed(or("the task failed"))),
        WireTaskState::Rejected => events.push(ExternalEvent::Failed(or("the task was refused"))),
        WireTaskState::InputRequired => events.push(ExternalEvent::InputRequired(or("it needs more to go on"))),
        WireTaskState::AuthRequired => events.push(ExternalEvent::Failed("it asks for an authentication it was not given".to_string())),
        // Submitted, working: what it says meanwhile is not the answer.
        WireTaskState::Other => {}
    }
}

/// The text of `parts`, one part a line; what is no text is left out.
fn text_of(parts: &[WirePart]) -> String {
    parts.iter().filter_map(|p| p.text.as_deref()).collect::<Vec<_>>().join("\n")
}

/// A JSON-RPC response.
#[derive(Debug, Deserialize)]
struct RpcResponse<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

impl<T> RpcResponse<T> {
    fn into_result(self, endpoint: &str) -> ExternalAgentResult<T> {
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
struct AgentCard {
    #[serde(default)]
    supported_interfaces: Vec<AgentInterface>,
    #[serde(default)]
    capabilities: AgentCapabilities,
}

impl AgentCard {
    /// What the card offers, for a refusal.
    fn offered(&self) -> String {
        if self.supported_interfaces.is_empty() {
            return "nothing".to_string();
        }
        self.supported_interfaces.iter().map(|i| format!("{} {}", i.protocol_binding, i.protocol_version)).collect::<Vec<_>>().join(", ")
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentInterface {
    url: String,
    #[serde(default)]
    protocol_binding: String,
    #[serde(default)]
    protocol_version: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentCapabilities {
    #[serde(default)]
    streaming: Option<bool>,
}

/// One response of a stream, or the answer to `SendMessage`: exactly one
/// of its fields is set.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamResponse {
    task: Option<WireTask>,
    message: Option<WireMessage>,
    status_update: Option<StatusUpdate>,
    artifact_update: Option<ArtifactUpdate>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireMessage {
    #[serde(default)]
    context_id: Option<String>,
    #[serde(default)]
    parts: Vec<WirePart>,
}

impl WireMessage {
    fn text(&self) -> String {
        text_of(&self.parts)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePart {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireTask {
    id: String,
    #[serde(default)]
    context_id: Option<String>,
    status: WireStatus,
    #[serde(default)]
    artifacts: Vec<WireArtifact>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireStatus {
    state: WireTaskState,
    #[serde(default)]
    message: Option<WireMessage>,
}

/// The states of a task that tell something; any other - submitted,
/// working, or one this version does not know - is `Other`.
#[derive(Debug, Deserialize)]
enum WireTaskState {
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireArtifact {
    #[serde(default)]
    artifact_id: String,
    #[serde(default)]
    parts: Vec<WirePart>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusUpdate {
    #[serde(default)]
    context_id: Option<String>,
    status: WireStatus,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactUpdate {
    #[serde(default)]
    context_id: Option<String>,
    artifact: WireArtifact,
    #[serde(default)]
    append: bool,
    #[serde(default)]
    last_chunk: bool,
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        ComponentKind::new(A2A_AGENT_KIND).provides::<dyn ExternalAgent>(),
        |descriptor, context| async move {
            let config: A2aAgentConfig = serde_json::from_value(descriptor.params.clone())
                .map_err(|e| ComponentError::InvalidConfig(format!("invalid parameters: {e}")))?;
            let component = A2aAgentComponent::build(descriptor.id.clone(), config, context.secret_manager.clone(), &context.security_context).await?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ExternalAgent>(Arc::new(component));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::external::Said;
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use caelumora_model::secret::{SecretProvider, SecretResult};
    use caelumora_model::security::DummySecurityManager;
    use futures::StreamExt;
    use std::sync::Mutex;

    /// The one secret of the tests.
    #[derive(Debug)]
    struct Token;

    #[async_trait]
    impl SecretProvider for Token {
        async fn get_secret(&self, key: &str) -> SecretResult<Option<String>> {
            Ok((key == "HERMES_TOKEN").then(|| "secret-token".to_string()))
        }
    }

    /// What the stub answers and what it was asked.
    #[derive(Default)]
    struct Stub {
        card: Value,
        /// By method: a JSON answer, or with `sse` the results streamed.
        answers: HashMap<String, (bool, Value)>,
        asked: Vec<(String, Value, Option<String>, Option<String>)>,
    }

    type Shared = Arc<Mutex<Stub>>;

    async fn card(State(stub): State<Shared>) -> Json<Value> {
        Json(stub.lock().unwrap().card.clone())
    }

    async fn rpc(State(stub): State<Shared>, headers: HeaderMap, Json(body): Json<Value>) -> axum::response::Response {
        use axum::response::IntoResponse;
        let method = body["method"].as_str().unwrap_or_default().to_string();
        let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        let answer = {
            let mut stub = stub.lock().unwrap();
            stub.asked.push((method.clone(), body["params"].clone(), header("a2a-version"), header("authorization")));
            stub.answers.get(&method).cloned()
        };
        match answer {
            // A server that takes the request and never answers.
            Some((true, Value::Null)) => std::future::pending().await,
            Some((true, Value::Array(results))) => {
                let body: String = results.iter().map(|r| format!("data: {}\n\n", json!({ "jsonrpc": "2.0", "id": "1", "result": r }))).collect();
                ([(axum::http::header::CONTENT_TYPE, "text/event-stream")], body).into_response()
            }
            Some((_, answer)) => Json(answer).into_response(),
            None => Json(json!({ "jsonrpc": "2.0", "id": "1", "error": { "code": -32601, "message": "no such method" } })).into_response(),
        }
    }

    /// A stub agent whose card offers JSON-RPC of `version`, streaming or
    /// not, with `answers` by method; and the component that reaches it.
    async fn agent(version: &str, streaming: bool, answers: Vec<(&str, bool, Value)>) -> (A2aAgentComponent, Shared) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stub: Shared = Arc::new(Mutex::new(Stub {
            card: json!({
                "name": "stub",
                "supportedInterfaces": [{ "url": format!("http://{address}/a2a"), "protocolBinding": "JSONRPC", "protocolVersion": version }],
                "capabilities": { "streaming": streaming },
            }),
            answers: answers.into_iter().map(|(m, sse, v)| (m.to_string(), (sse, v))).collect(),
            asked: Vec::new(),
        }));
        let app = Router::new().route("/.well-known/agent-card.json", get(card)).route("/a2a", post(rpc)).with_state(stub.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (component(&format!("http://{address}")).await, stub)
    }

    async fn component(url: &str) -> A2aAgentComponent {
        let config: A2aAgentConfig = serde_json::from_value(json!({ "url": url, "auth": { "type": "bearer", "secret": "HERMES_TOKEN" }, "timeout_secs": 2 })).unwrap();
        let secrets = Arc::new(SecretManager::new(vec![Arc::new(Token)], Arc::new(DummySecurityManager::new())));
        A2aAgentComponent::build("hermes".to_string(), config, secrets, &SecurityContext::default()).await.unwrap()
    }

    fn hello() -> OutboundMessage {
        OutboundMessage { said: vec![Said { sender: "alice".to_string(), text: "hello".to_string() }], answers: None }
    }

    async fn all(stream: ExternalStream) -> Vec<ExternalEvent> {
        stream.map(|e| e.expect("no error")).collect().await
    }

    fn task(id: &str) -> TaskId {
        TaskId(id.to_string())
    }

    #[test]
    fn the_parameters_name_the_agent_and_how_to_reach_it() {
        let config: A2aAgentConfig = serde_json::from_value(json!({ "url": "https://hermes.example/" })).unwrap();
        assert_eq!(config.card_url(), "https://hermes.example/.well-known/agent-card.json");
        assert_eq!(config.timeout_secs, 30);
        let own: A2aAgentConfig = serde_json::from_value(json!({ "url": "https://hermes.example/card.json" })).unwrap();
        assert_eq!(own.card_url(), "https://hermes.example/card.json", "a card's own URL");
        assert!(serde_json::from_value::<A2aAgentConfig>(json!({ "url": "x", "roles": [] })).is_err(), "nothing unknown");
    }

    #[tokio::test]
    async fn a_streamed_answer_is_told_as_it_comes_and_what_was_said_is_sent_with_version_and_token() {
        let (agent, stub) = agent("1.0", true, vec![("SendStreamingMessage", true, json!([
            { "task": { "id": "t-1", "contextId": "c-1", "status": { "state": "TASK_STATE_SUBMITTED" } } },
            { "statusUpdate": { "taskId": "t-1", "contextId": "c-1", "status": { "state": "TASK_STATE_WORKING", "message": { "parts": [{ "text": "thinking" }] } } } },
            { "artifactUpdate": { "taskId": "t-1", "contextId": "c-1", "artifact": { "artifactId": "a", "parts": [{ "text": "Hel" }] } } },
            { "artifactUpdate": { "taskId": "t-1", "contextId": "c-1", "artifact": { "artifactId": "a", "parts": [{ "text": "lo." }] }, "append": true, "lastChunk": true } },
            { "statusUpdate": { "taskId": "t-1", "contextId": "c-1", "status": { "state": "TASK_STATE_COMPLETED" } } },
        ]))]).await;

        let events = all(agent.send(&SecurityContext::default(), None, hello()).await.unwrap()).await;
        assert_eq!(events, vec![
            ExternalEvent::Conversation(ConversationId("c-1".to_string())),
            ExternalEvent::Started(task("t-1")),
            ExternalEvent::Partial("Hel".to_string()),
            ExternalEvent::Message("Hello.".to_string()),
            ExternalEvent::Completed,
        ], "the conversation once, no progress text, the artifact whole at its last chunk");

        let asked = stub.lock().unwrap().asked.clone();
        let (method, params, version, authorization) = &asked[0];
        assert_eq!(method, "SendStreamingMessage");
        assert_eq!(version.as_deref(), Some("1.0"));
        assert_eq!(authorization.as_deref(), Some("Bearer secret-token"));
        assert_eq!(params["message"]["role"], "ROLE_USER");
        assert_eq!(params["message"]["parts"][0]["text"], "alice: hello");
        assert!(params["message"].get("contextId").is_none(), "the remote agent opens the conversation");
    }

    #[tokio::test]
    async fn events_split_anywhere_by_the_network_arrive_whole() {
        let result = |r: Value| format!("data: {}\r\n\r\n", json!({ "jsonrpc": "2.0", "id": "1", "result": r }));
        let body = [
            result(json!({ "task": { "id": "t-1", "contextId": "c-1", "status": { "state": "TASK_STATE_WORKING" } } })),
            result(json!({ "artifactUpdate": { "taskId": "t-1", "contextId": "c-1", "artifact": { "artifactId": "a", "parts": [{ "text": "Grüße" }] }, "lastChunk": true } })),
            result(json!({ "statusUpdate": { "taskId": "t-1", "contextId": "c-1", "status": { "state": "TASK_STATE_COMPLETED" } } })),
        ].concat();
        // Each byte a chunk of its own: an "ü" and every "\r\n" split.
        let bytes = futures::stream::iter(body.into_bytes().into_iter().map(|b| Ok::<_, std::io::Error>(bytes::Bytes::from(vec![b]))));

        let events: Vec<_> = events_of(bytes, "agent".to_string()).collect().await;
        let events: Vec<_> = events.into_iter().map(Result::unwrap).collect();
        assert_eq!(events, vec![
            ExternalEvent::Conversation(ConversationId("c-1".to_string())),
            ExternalEvent::Started(task("t-1")),
            ExternalEvent::Message("Grüße".to_string()),
            ExternalEvent::Completed,
        ]);
    }

    #[tokio::test]
    async fn without_streaming_the_answer_is_waited_for_and_a_reply_continues_the_task() {
        let (agent, stub) = agent("1.0", false, vec![("SendMessage", false, json!({ "jsonrpc": "2.0", "id": "1", "result": {
            "task": { "id": "t-1", "contextId": "c-1", "status": { "state": "TASK_STATE_INPUT_REQUIRED", "message": { "role": "ROLE_AGENT", "parts": [{ "text": "Which city?" }] } } }
        } }))]).await;
        let conversation = ConversationId("c-1".to_string());
        let reply = OutboundMessage { answers: Some(task("t-1")), ..hello() };

        let events = all(agent.send(&SecurityContext::default(), Some(&conversation), reply).await.unwrap()).await;
        assert_eq!(events, vec![ExternalEvent::Conversation(conversation), ExternalEvent::Started(task("t-1")), ExternalEvent::InputRequired("Which city?".to_string())]);
        let params = stub.lock().unwrap().asked[0].1.clone();
        assert_eq!((params["message"]["contextId"].as_str(), params["message"]["taskId"].as_str()), (Some("c-1"), Some("t-1")));
    }

    #[tokio::test]
    async fn a_streamed_request_its_server_never_answers_ends_after_the_timeout() {
        let (agent, _) = agent("1.0", true, vec![("SendStreamingMessage", true, Value::Null)]).await;
        let started = std::time::Instant::now();
        let error = agent.send(&SecurityContext::default(), None, hello()).await.err().expect("no answer").to_string();
        assert!(error.contains("no answer within 2 s"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn a_task_is_resumed_cancelled_and_a_task_past_cancelling_is_no_error() {
        let (agent, _) = agent("1.0", true, vec![
            ("SubscribeToTask", true, json!([{ "statusUpdate": { "taskId": "t-1", "status": { "state": "TASK_STATE_COMPLETED", "message": { "parts": [{ "text": "Done." }] } } } }])),
            ("CancelTask", false, json!({ "jsonrpc": "2.0", "id": "1", "error": { "code": TASK_NOT_CANCELABLE, "message": "it has ended" } })),
        ]).await;
        let security = SecurityContext::default();
        assert_eq!(all(agent.resume(&security, &task("t-1")).await.unwrap()).await, vec![ExternalEvent::Message("Done.".to_string()), ExternalEvent::Completed]);
        agent.cancel(&security, &task("t-1")).await.expect("past cancelling is no error");
    }

    #[tokio::test]
    async fn an_agent_that_cannot_be_reached_or_speaks_no_a2a_1_is_refused_when_asked() {
        // Nobody listens there: built all the same.
        let gone = component("http://127.0.0.1:9").await;
        let unreachable = gone.send(&SecurityContext::default(), None, hello()).await.err().expect("nobody there").to_string();
        assert!(unreachable.contains("cannot reach the external agent at http://127.0.0.1:9"), "{unreachable}");

        let (old, _) = agent("0.3", true, vec![]).await;
        let refused = old.send(&SecurityContext::default(), None, hello()).await.err().expect("not 1.x").to_string();
        assert!(refused.contains("no JSON-RPC interface of A2A 1.x, only JSONRPC 0.3"), "{refused}");
    }

    #[tokio::test]
    async fn a_refused_request_says_why() {
        let (agent, _) = agent("1.0", false, vec![("SendMessage", false, json!({ "jsonrpc": "2.0", "id": "1", "error": { "code": -32603, "message": "boom" } }))]).await;
        let refused = agent.send(&SecurityContext::default(), None, hello()).await.err().expect("refused").to_string();
        assert!(refused.contains("rejected the request: boom (-32603)"), "{refused}");
    }
}
