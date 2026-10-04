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
//! The protocol's types are in `wire`; between them and `crate::external`
//! translates `translation`; `client` reaches the agent and hands back
//! what it answered, as wire types. What is left here is the component:
//! which method the agent is asked with, and what of its answers is
//! translated how (issue 210).

mod client;
mod translation;
mod wire;

use self::client::{A2aClient, Responses};
use self::translation::{wire_message, Translation, ACCEPTED_OUTPUT_MODES};
use self::wire::{method, SendConfiguration, SendMessageParams, StreamResponse, TaskParams, WireTask, TASK_NOT_CANCELABLE};
use crate::component::{ComponentError, ComponentFacets, ComponentKind, ComponentRegistry};
use crate::external::{ExternalAgent, ExternalAgentResult, ExternalStream, OutboundMessage};
use crate::remote::RemoteEndpoint;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::{ModelProfile, SenderFormat, SenderStyle};
use caelumora_model::external_agent::{ConversationId, TaskId};
use caelumora_model::secret::SecretManager;
use caelumora_model::security::SecurityContext;
use futures::StreamExt;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

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
    /// How the agent is told who said what (issue 135): A2A has no sender
    /// of a message, so it is marked in the content - `prefix` unless
    /// named, by `handle` unless named.
    #[serde(default = "SenderFormat::configured_default")]
    sender_format: SenderFormat,
    #[serde(default)]
    sender_style: SenderStyle,
}

impl A2aAgentConfig {
    fn timeout(&self) -> Duration {
        self.endpoint.timeout_or(DEFAULT_TIMEOUT)
    }
}

/// An agent reached over A2A, as the `ExternalAgent` of an external
/// personality: which method it is asked with, and what its answers tell.
#[derive(Debug)]
pub struct A2aAgentComponent {
    client: Arc<A2aClient>,
    classification: Option<Classification>,
    profile: ModelProfile,
}

impl A2aAgentComponent {
    /// The component `id` with `config`, its card read if it can be, in
    /// `security`: an agent that cannot be reached yet does not keep its
    /// personality from being built.
    async fn build(id: String, config: A2aAgentConfig, secret_manager: Arc<SecretManager>, security: &SecurityContext) -> Result<Self, ComponentError> {
        let timeout = config.timeout();
        let client = A2aClient::new(id.clone(), config.endpoint, timeout, secret_manager)?;
        if let Err(e) = client.streaming(security).await {
            warn!("A2A agent component '{}' cannot read its agent card yet: {}", id, e);
        }
        let profile = ModelProfile { sender_format: config.sender_format, sender_style: config.sender_style, ..ModelProfile::default() };
        Ok(Self { client: Arc::new(client), classification: config.classification, profile })
    }

    /// The events of `task` on an agent without streaming, asked for with
    /// `GetTask` - after `first`, where it stands already - until it ends or
    /// asks back (issue 186): while it works, only that it was started is
    /// told, and it is asked again a little later each time. Its own
    /// translation: every answer repeats what the task produced so far.
    fn polled(&self, security: &SecurityContext, mut first: Option<WireTask>, task: TaskId) -> ExternalStream {
        let client = self.client.clone();
        let security = security.clone();
        Box::pin(async_stream::stream! {
            let mut translation = Translation::default();
            let mut wait = POLL_FIRST;
            let mut told = false;
            loop {
                let found = match first.take() {
                    Some(found) => found,
                    None => match client.call::<WireTask>(&security, method::GET_TASK, TaskParams { id: task.0.clone() }).await {
                        Ok(Ok(found)) => found,
                        Ok(Err(error)) => {
                            yield Err(client.rejected(error));
                            return;
                        }
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

/// The events of `responses`, through one translation: what the agent
/// streamed, or the one answer it gave. Ends with them, or after an error.
fn translated(mut responses: Responses) -> ExternalStream {
    Box::pin(async_stream::stream! {
        let mut translation = Translation::default();
        while let Some(response) = responses.next().await {
            match response {
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
    })
}

#[async_trait]
impl ExternalAgent for A2aAgentComponent {
    async fn send(&self, security: &SecurityContext, conversation: Option<&ConversationId>, message: OutboundMessage) -> ExternalAgentResult<ExternalStream> {
        let streaming = self.client.streaming(security).await?;
        let params = SendMessageParams {
            message: wire_message(&message, conversation),
            configuration: SendConfiguration {
                accepted_output_modes: ACCEPTED_OUTPUT_MODES.map(str::to_string).to_vec(),
                // Without streaming answered at once with the task, which is
                // followed until it ends: its id is known however long it
                // runs (issue 186).
                return_immediately: !streaming,
            },
        };
        if streaming {
            return Ok(translated(self.client.stream(security, method::SEND_STREAMING_MESSAGE, params).await?));
        }
        let mut response: StreamResponse = self.client.call(security, method::SEND_MESSAGE, params).await?
            .map_err(|error| self.client.rejected(error))?;
        match response.task.take_if(|task| task.status.state.works()) {
            Some(task) => {
                let id = TaskId(task.id.clone());
                Ok(self.polled(security, Some(task), id))
            }
            None => Ok(translated(Box::pin(futures::stream::once(async move { Ok(response) })))),
        }
    }

    async fn resume(&self, security: &SecurityContext, task: &TaskId) -> ExternalAgentResult<ExternalStream> {
        if self.client.streaming(security).await? {
            Ok(translated(self.client.stream(security, method::SUBSCRIBE_TO_TASK, TaskParams { id: task.0.clone() }).await?))
        } else {
            Ok(self.polled(security, None, task.clone()))
        }
    }

    async fn cancel(&self, security: &SecurityContext, task: &TaskId) -> ExternalAgentResult<()> {
        let answer = self.client.call::<serde::de::IgnoredAny>(security, method::CANCEL_TASK, TaskParams { id: task.0.clone() }).await?;
        match answer {
            Err(error) if error.code == TASK_NOT_CANCELABLE => Ok(()),
            Err(error) => Err(self.client.rejected(error)),
            Ok(_) => Ok(()),
        }
    }

    fn classification(&self) -> Option<Classification> {
        self.classification.clone()
    }

    fn profile(&self) -> ModelProfile {
        self.profile
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
    use crate::external::{ExternalEvent, Said};
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
        OutboundMessage { prompts: Vec::new(), said: vec![Said { sender: "alice".to_string(), text: "alice: hello".to_string() }], answers: None }
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
        assert_eq!(config.timeout(), Duration::from_secs(30));
        assert_eq!((config.sender_format, config.sender_style), (SenderFormat::Prefix, SenderStyle::Handle), "told who said what (issue 135)");
        let config: A2aAgentConfig = serde_json::from_value(json!({ "endpoint": { "url": "x" }, "sender_format": "none", "sender_style": "name" })).unwrap();
        assert_eq!((config.sender_format, config.sender_style), (SenderFormat::None, SenderStyle::Name));
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
