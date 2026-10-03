//! An external agent reached over the Agent2Agent protocol, A2A v1.0
//! (issue 179): its agent card says where and how to reach it; what is
//! said is sent as a message over JSON-RPC, and what comes back - a task,
//! its status, its artifacts, or a message of its own - is translated to
//! `ExternalEvent`s. Streams over server-sent events where the card offers
//! streaming; where it does not, asks for the task until it ends or asks
//! back.
//! Only A2A 1.x over JSON-RPC: a card that offers nothing else is refused
//! when the agent is asked, not when the component is built.
//!
//! The protocol's types are in `wire`, what they tell as `ExternalEvent`s
//! in `translation` (issue 210).

mod translation;
mod wire;

use self::translation::{wire_message, Translation, ACCEPTED_OUTPUT_MODES};
use self::wire::{is_version_1, method, AgentCard, RpcRequest, RpcResponse, SendConfiguration, SendMessageParams, StreamResponse, TaskParams, WireTask, A2A_VERSION, CARD_PATH, JSONRPC_BINDING, TASK_NOT_CANCELABLE};
use crate::component::{ComponentError, ComponentFacets, ComponentKind, ComponentRegistry};
use crate::external::{ExternalAgent, ExternalAgentError, ExternalAgentResult, ExternalEvent, ExternalStream, OutboundMessage};
use crate::remote::{describe_chain, describe_request_error, redact_endpoint, RemoteEndpoint};
use arc_swap::ArcSwapOption;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::external_agent::{ConversationId, TaskId};
use caelumora_model::secret::SecretManager;
use caelumora_model::security::SecurityContext;
use futures::StreamExt;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sse_stream::SseStream;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, trace, warn};

pub const A2A_AGENT_KIND: &str = "a2a_agent";

/// How long a task that works is left before it is asked again, at first
/// and at the most: twice as long each time (issue 186).
const POLL_FIRST: Duration = Duration::from_secs(1);
const POLL_LONGEST: Duration = Duration::from_secs(30);

/// How long one request may take unless the endpoint says otherwise.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The parameters of an `a2a_agent`, written by hand.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct A2aAgentConfig {
    /// The agent: its base URL, below which its card is published - or the
    /// card's own URL, ending in `.json`. Its timeout is how long one
    /// request may take to be answered - a streamed one until its stream
    /// begins; not how long a task may run.
    endpoint: RemoteEndpoint,
    /// How the agent's answers are classified (issue 133); the channel's
    /// unless named.
    #[serde(default)]
    classification: Option<Classification>,
}

impl A2aAgentConfig {
    fn card_url(&self) -> String {
        let url = &self.endpoint.url;
        if url.ends_with(".json") {
            url.clone()
        } else {
            format!("{}/{CARD_PATH}", url.trim_end_matches('/'))
        }
    }

    fn timeout(&self) -> Duration {
        self.endpoint.timeout_or(DEFAULT_TIMEOUT)
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
#[derive(Debug, Clone)]
pub struct A2aAgentComponent {
    id: String,
    config: A2aAgentConfig,
    /// Read from the card when the component is built, or when the agent is
    /// first asked if that failed.
    endpoint: Arc<ArcSwapOption<Endpoint>>,
    http: reqwest::Client,
    secret_manager: Arc<SecretManager>,
}

impl A2aAgentComponent {
    /// The component `id` with `config`, its card read if it can be, in
    /// `security`: an agent that cannot be reached yet does not keep its
    /// personality from being built.
    async fn build(id: String, config: A2aAgentConfig, secret_manager: Arc<SecretManager>, security: &SecurityContext) -> Result<Self, ComponentError> {
        let http = reqwest::Client::builder()
            .connect_timeout(config.timeout())
            .build()
            .map_err(|e| ComponentError::InvalidConfig(format!("no HTTP client for '{}': {e}", config.endpoint.shown())))?;
        let component = Self { id, config, endpoint: Arc::new(ArcSwapOption::empty()), http, secret_manager };
        if let Err(e) = component.endpoint(security).await {
            warn!("A2A agent component '{}' cannot read its agent card yet: {}", component.id, e);
        }
        Ok(component)
    }

    /// The URL of the agent, for messages: without credentials.
    fn shown(&self) -> String {
        self.config.endpoint.shown()
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
        let mut request = self.http.get(self.config.card_url()).timeout(self.config.timeout());
        if let Some(token) = self.token(security).await? {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|e| self.connection(e))?;
        let card: AgentCard = self.json_of(response).await?;
        let interface = card.supported_interfaces.iter()
            .find(|i| i.protocol_binding == JSONRPC_BINDING && is_version_1(&i.protocol_version))
            .ok_or_else(|| ExternalAgentError::Rejected {
                endpoint: self.shown(),
                message: format!("its agent card offers no JSON-RPC interface of A2A 1.x, only {}", card.offered()),
            })?;
        debug!("A2A agent component '{}' reaches its agent at {}", self.id, redact_endpoint(&interface.url));
        Ok(Endpoint { url: interface.url.clone(), streaming: card.capabilities.streaming.unwrap_or(false) })
    }

    async fn token(&self, security: &SecurityContext) -> ExternalAgentResult<Option<String>> {
        self.config.endpoint.bearer_token(&self.secret_manager, security).await
            .map_err(|source| ExternalAgentError::Credential { endpoint: self.shown(), source })
    }

    fn connection(&self, e: reqwest::Error) -> ExternalAgentError {
        ExternalAgentError::Connection { endpoint: self.shown(), reason: describe_request_error(e) }
    }

    /// The body of `response` as `T`, if it answered successfully.
    async fn json_of<T: DeserializeOwned>(&self, response: reqwest::Response) -> ExternalAgentResult<T> {
        self.require_success(&response)?;
        let text = response.text().await.map_err(|e| self.connection(e))?;
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
    async fn post(&self, security: &SecurityContext, endpoint: &Endpoint, method: &str, params: impl Serialize, streamed: bool) -> ExternalAgentResult<reqwest::Response> {
        let body = RpcRequest::new(method, params);
        trace!("A2A agent component '{}' sends {}: {}", self.id, method, serde_json::to_string(&body).unwrap_or_default());
        let mut request = self.http.post(&endpoint.url).header("A2A-Version", A2A_VERSION).json(&body);
        request = if streamed {
            request.header(reqwest::header::ACCEPT, "text/event-stream")
        } else {
            request.timeout(self.config.timeout())
        };
        if let Some(token) = self.token(security).await? {
            request = request.bearer_auth(token);
        }
        let timeout = self.config.timeout();
        match tokio::time::timeout(timeout, request.send()).await {
            Ok(response) => response.map_err(|e| self.connection(e)),
            Err(_) => Err(ExternalAgentError::Connection { endpoint: self.shown(), reason: format!("no answer within {} s", timeout.as_secs()) }),
        }
    }

    /// The result of the JSON-RPC request `method`.
    async fn call<T: DeserializeOwned>(&self, security: &SecurityContext, endpoint: &Endpoint, method: &str, params: impl Serialize) -> ExternalAgentResult<T> {
        let response = self.post(security, endpoint, method, params, false).await?;
        let answer: RpcResponse<T> = self.json_of(response).await?;
        answer.into_result(&self.shown())
    }

    /// The events of the streamed JSON-RPC request `method`.
    async fn stream(&self, security: &SecurityContext, endpoint: &Endpoint, method: &str, params: impl Serialize) -> ExternalAgentResult<ExternalStream> {
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
        let bytes = response.bytes_stream().map(|chunk| chunk.map_err(reqwest::Error::without_url));
        Ok(Box::pin(events_of(bytes, self.shown())))
    }

    /// The events of `task` on an agent without streaming, asked for with
    /// `GetTask` - after `first`, where it stands already - until it ends or
    /// asks back (issue 186): while it works, only that it was started is
    /// told, and it is asked again a little later each time.
    fn polled(&self, security: &SecurityContext, endpoint: Endpoint, mut first: Option<WireTask>, task: TaskId) -> ExternalStream {
        let agent = self.clone();
        let security = security.clone();
        Box::pin(async_stream::stream! {
            let mut translation = Translation::default();
            let mut wait = POLL_FIRST;
            let mut told = false;
            loop {
                let found = match first.take() {
                    Some(found) => found,
                    None => match agent.call::<WireTask>(&security, &endpoint, method::GET_TASK, TaskParams { id: task.0.clone() }).await {
                        Ok(found) => found,
                        Err(e) => {
                            yield Err(e);
                            return;
                        }
                    },
                };
                if !found.status.state.works() {
                    for event in translation.events(StreamResponse { task: Some(found), ..StreamResponse::default() }) {
                        yield Ok(event);
                    }
                    return;
                }
                if !told {
                    told = true;
                    for event in translation.started(&found) {
                        yield Ok(event);
                    }
                }
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(POLL_LONGEST);
            }
        })
    }
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
                    yield Err(ExternalAgentError::Connection { endpoint: endpoint.clone(), reason: describe_chain(e.as_ref()) });
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
        let params = SendMessageParams {
            message: wire_message(&message, conversation),
            configuration: SendConfiguration {
                accepted_output_modes: ACCEPTED_OUTPUT_MODES.map(str::to_string).to_vec(),
                // Without streaming answered at once with the task, which is
                // followed until it ends: its id is known however long it
                // runs (issue 186).
                return_immediately: !endpoint.streaming,
            },
        };
        if endpoint.streaming {
            self.stream(security, &endpoint, method::SEND_STREAMING_MESSAGE, params).await
        } else {
            let mut response: StreamResponse = self.call(security, &endpoint, method::SEND_MESSAGE, params).await?;
            match response.task.take_if(|task| task.status.state.works()) {
                Some(task) => {
                    let id = TaskId(task.id.clone());
                    Ok(self.polled(security, endpoint, Some(task), id))
                }
                None => Ok(answered(response)),
            }
        }
    }

    async fn resume(&self, security: &SecurityContext, task: &TaskId) -> ExternalAgentResult<ExternalStream> {
        let endpoint = self.endpoint(security).await?;
        if endpoint.streaming {
            self.stream(security, &endpoint, method::SUBSCRIBE_TO_TASK, TaskParams { id: task.0.clone() }).await
        } else {
            Ok(self.polled(security, endpoint, None, task.clone()))
        }
    }

    async fn cancel(&self, security: &SecurityContext, task: &TaskId) -> ExternalAgentResult<()> {
        let endpoint = self.endpoint(security).await?;
        let response = self.post(security, &endpoint, method::CANCEL_TASK, TaskParams { id: task.0.clone() }, false).await?;
        let answer: RpcResponse<serde::de::IgnoredAny> = self.json_of(response).await?;
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
    use serde_json::{json, Value};
    use std::collections::HashMap;
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
        /// By method: a JSON answer - an array of them in turn, the last
        /// kept - or with `sse` the results streamed.
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
            stub.answers.get_mut(&method).map(|(sse, answer)| match answer {
                Value::Array(turns) if !*sse && turns.len() > 1 => (false, turns.remove(0)),
                Value::Array(turns) if !*sse => (false, turns[0].clone()),
                _ => (*sse, answer.clone()),
            })
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
        let config: A2aAgentConfig = serde_json::from_value(json!({ "endpoint": { "url": url, "auth": { "type": "bearer", "secret": "HERMES_TOKEN" }, "timeout_secs": 2 } })).unwrap();
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
        let config: A2aAgentConfig = serde_json::from_value(json!({ "endpoint": { "url": "https://hermes.example/" } })).unwrap();
        assert_eq!(config.card_url(), "https://hermes.example/.well-known/agent-card.json");
        assert_eq!(config.timeout(), Duration::from_secs(30));
        let own: A2aAgentConfig = serde_json::from_value(json!({ "endpoint": { "url": "https://hermes.example/card.json" } })).unwrap();
        assert_eq!(own.card_url(), "https://hermes.example/card.json", "a card's own URL");
        assert!(serde_json::from_value::<A2aAgentConfig>(json!({ "endpoint": { "url": "x" }, "roles": [] })).is_err(), "nothing unknown");
        assert!(serde_json::from_value::<A2aAgentConfig>(json!({ "endpoint": { "url": "x", "timeout": 3 } })).is_err(), "nothing unknown in the endpoint");
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
    async fn without_streaming_a_long_task_is_answered_at_once_and_followed_until_it_ends() {
        let task_in = |state: &str, artifacts: Value| json!({ "jsonrpc": "2.0", "id": "1", "result": { "task": { "id": "t-1", "contextId": "c-1", "status": { "state": state }, "artifacts": artifacts } } });
        let ended = json!({ "jsonrpc": "2.0", "id": "1", "result": { "id": "t-1", "contextId": "c-1", "status": { "state": "TASK_STATE_COMPLETED" }, "artifacts": [{ "artifactId": "a", "parts": [{ "text": "Done." }] }] } });
        let (agent, stub) = agent("1.0", false, vec![
            ("SendMessage", false, task_in("TASK_STATE_SUBMITTED", json!([]))),
            ("GetTask", false, ended),
        ]).await;

        let events = all(agent.send(&SecurityContext::default(), None, hello()).await.unwrap()).await;
        assert_eq!(events, vec![
            ExternalEvent::Conversation(ConversationId("c-1".to_string())),
            ExternalEvent::Started(task("t-1")),
            ExternalEvent::Message("Done.".to_string()),
            ExternalEvent::Completed,
        ]);
        let asked = stub.lock().unwrap().asked.clone();
        assert_eq!(asked[0].1["configuration"]["returnImmediately"], true, "the request is not held open while the task runs");
        assert_eq!(asked.iter().map(|(method, ..)| method.as_str()).collect::<Vec<_>>(), vec!["SendMessage", "GetTask"]);
    }

    #[tokio::test]
    async fn without_streaming_a_task_is_resumed_by_asking_until_it_ends() {
        let task_in = |state: &str| json!({ "jsonrpc": "2.0", "id": "1", "result": { "id": "t-1", "contextId": "c-1", "status": { "state": state }, "artifacts": [{ "artifactId": "a", "parts": [{ "text": "Done." }] }] } });
        let (agent, stub) = agent("1.0", false, vec![("GetTask", false, json!([task_in("TASK_STATE_WORKING"), task_in("TASK_STATE_WORKING"), task_in("TASK_STATE_COMPLETED")]))]).await;

        let events = all(agent.resume(&SecurityContext::default(), &task("t-1")).await.unwrap()).await;
        assert_eq!(events, vec![
            ExternalEvent::Conversation(ConversationId("c-1".to_string())),
            ExternalEvent::Started(task("t-1")),
            ExternalEvent::Message("Done.".to_string()),
            ExternalEvent::Completed,
        ], "started once while it works, what came of it once it ends");
        assert_eq!(stub.lock().unwrap().asked.iter().filter(|(method, ..)| method == "GetTask").count(), 3);
    }

    #[tokio::test]
    async fn an_agent_that_cannot_be_reached_or_speaks_no_a2a_1_is_refused_when_asked() {
        // Nobody listens there: built all the same.
        let gone = component("http://127.0.0.1:9").await;
        let unreachable = gone.send(&SecurityContext::default(), None, hello()).await.err().expect("nobody there").to_string();
        assert!(unreachable.contains("cannot reach the external agent at http://127.0.0.1:9"), "{unreachable}");
        let secret = component("http://user:secret@127.0.0.1:9/a2a.json?key=secret").await;
        let unreachable = secret.send(&SecurityContext::default(), None, hello()).await.err().expect("nobody there").to_string();
        assert!(unreachable.contains("at http://127.0.0.1:9/a2a.json") && !unreachable.contains("secret"), "{unreachable}");

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
