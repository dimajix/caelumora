//! The transport of the A2A agent (issue 210): one A2A agent reached over
//! JSON-RPC, its answers handed back as they are on the wire - the result
//! of a request, or the responses of a stream of server-sent events. It
//! knows nothing of `ExternalEvent`s.

use super::wire::{is_version_1, AgentCard, RpcError, RpcRequest, RpcResponse, StreamResponse, A2A_VERSION, CARD_PATH, JSONRPC_BINDING};
use bytes::Buf;
use caelumora_model::remote::{redact_endpoint, RemoteEndpoint};
use caelumora_model::secret::SecretManager;
use caelumora_model::security::SecurityContext;
use caelumora_runtime::component::ComponentError;
use caelumora_agents::external::{ExternalAgentError, ExternalAgentResult};
use caelumora_runtime::remote::{describe_chain, describe_request_error, ExpiringCache};
use futures::stream::{once, BoxStream};
use futures::{Stream, StreamExt};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::{Client, Response, Error};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sse_stream::SseStream;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{debug, trace};

/// The responses of a streamed request, as they come; it ends after an
/// error.
pub(super) type Responses = BoxStream<'static, ExternalAgentResult<StreamResponse>>;

/// What of the agent card is used: where to send requests, and whether
/// they are streamed.
#[derive(Debug, Clone)]
struct Interface {
    url: String,
    streaming: bool,
}

/// One A2A agent, reached over JSON-RPC as its agent card says - read the
/// first time it is needed, in the name of whoever needs it, and again once
/// what was read is older than its time to keep it, or the agent could not
/// be reached where its card said (issue 126).
#[derive(Debug)]
pub(super) struct A2aClient {
    /// The component's, for the log.
    id: String,
    endpoint: RemoteEndpoint,
    /// How long one request may take to be answered - a streamed one until
    /// its stream begins.
    timeout: Duration,
    http: Client,
    secret_manager: Arc<SecretManager>,
    interface: ExpiringCache<Interface>,
}

impl A2aClient {
    /// The client of the component `id` for the agent at `endpoint`, whose
    /// card is kept for `refresh` once read; it is not read yet.
    ///
    /// # Errors
    /// If no HTTP client can be made.
    pub(super) fn new(id: String, endpoint: RemoteEndpoint, timeout: Duration, refresh: Duration, secret_manager: Arc<SecretManager>) -> Result<Self, ComponentError> {
        let http = Client::builder()
            .connect_timeout(timeout)
            .build()
            .map_err(|e| ComponentError::InvalidConfig(format!("no HTTP client for '{}': {e}", endpoint.shown())))?;
        Ok(Self { id, endpoint, timeout, http, secret_manager, interface: ExpiringCache::new(refresh) })
    }

    /// Whether the agent streams its answers.
    ///
    /// # Errors
    /// If its card cannot be read, or offers no JSON-RPC interface of A2A
    /// 1.x.
    pub(super) async fn streaming(&self, security: &SecurityContext) -> ExternalAgentResult<bool> {
        Ok(self.interface(security).await?.streaming)
    }

    /// The result of the JSON-RPC request `method` with `params` - or the
    /// JSON-RPC error the agent answered with, for the caller to tell
    /// apart; `rejected` makes it the error of a refused request.
    ///
    /// # Errors
    /// If the agent cannot be reached, or does not answer as JSON-RPC.
    pub(super) async fn call<T: DeserializeOwned>(&self, security: &SecurityContext, method: &str, params: impl Serialize) -> ExternalAgentResult<Result<T, RpcError>> {
        let response = self.post(security, method, params, false).await?;
        let answer: RpcResponse<T> = self.json_of(response).await?;
        answer.into_result(&self.shown())
    }

    /// The responses of the streamed JSON-RPC request `method` with
    /// `params`. An agent that refuses it before it begins answers with a
    /// JSON-RPC error instead of a stream.
    ///
    /// # Errors
    /// If the agent cannot be reached, or refuses the request.
    pub(super) async fn stream(&self, security: &SecurityContext, method: &str, params: impl Serialize) -> ExternalAgentResult<Responses> {
        let response = self.post(security, method, params, true).await?;
        self.require_success(&response)?;
        let is_stream = response.headers().get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        if !is_stream {
            // Refused before it began: a JSON-RPC error, not a stream.
            let answer: RpcResponse<StreamResponse> = self.json_of(response).await?;
            let response = answer.into_result(&self.shown())?.map_err(|error| self.rejected(error))?;
            return Ok(Box::pin(once(async move { Ok(response) })));
        }
        let bytes = response.bytes_stream().map(|chunk| chunk.map_err(Error::without_url));
        Ok(Box::pin(responses_of(bytes, self.shown())))
    }

    /// The error for a request the agent refused with `error`.
    pub(super) fn rejected(&self, error: RpcError) -> ExternalAgentError {
        error.rejected(&self.shown())
    }

    /// The URL of the agent, for messages: without credentials.
    fn shown(&self) -> String {
        self.endpoint.shown()
    }

    /// Where the agent card is: below the agent's base URL, or at the URL
    /// configured if that is a card's own, ending in `.json`.
    fn card_url(&self) -> String {
        let url = &self.endpoint.url;
        if url.ends_with(".json") {
            url.clone()
        } else {
            format!("{}/{CARD_PATH}", url.trim_end_matches('/'))
        }
    }

    /// Where and how to reach the agent, as its card says: read in
    /// `security` unless what was read is kept still.
    async fn interface(&self, security: &SecurityContext) -> ExternalAgentResult<Arc<Interface>> {
        self.interface.get(|| self.discover(security)).await
    }

    /// Reads the agent card and chooses its JSON-RPC interface of A2A 1.x.
    async fn discover(&self, security: &SecurityContext) -> ExternalAgentResult<Interface> {
        let mut request = self.http.get(self.card_url()).timeout(self.timeout);
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
        Ok(Interface { url: interface.url.clone(), streaming: card.capabilities.streaming.unwrap_or(false) })
    }

    async fn token(&self, security: &SecurityContext) -> ExternalAgentResult<Option<String>> {
        self.endpoint.bearer_token(&self.secret_manager, security).await
            .map_err(|source| ExternalAgentError::Credential { endpoint: self.shown(), source })
    }

    fn connection(&self, e: Error) -> ExternalAgentError {
        ExternalAgentError::Connection { endpoint: self.shown(), reason: describe_request_error(e) }
    }

    /// The body of `response` as `T`, if it answered successfully.
    async fn json_of<T: DeserializeOwned>(&self, response: Response) -> ExternalAgentResult<T> {
        self.require_success(&response)?;
        let text = response.text().await.map_err(|e| self.connection(e))?;
        serde_json::from_str(&text).map_err(|e| ExternalAgentError::InvalidResponse { endpoint: self.shown(), reason: e.to_string() })
    }

    /// Refuses `response` unless it answered successfully.
    fn require_success(&self, response: &Response) -> ExternalAgentResult<()> {
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
    async fn post(&self, security: &SecurityContext, method: &str, params: impl Serialize, streamed: bool) -> ExternalAgentResult<Response> {
        let interface = self.interface(security).await?;
        let body = RpcRequest::new(method, params);
        trace!("A2A agent component '{}' sends {}: {}", self.id, method, serde_json::to_string(&body).unwrap_or_default());
        let mut request = self.http.post(&interface.url).header("A2A-Version", A2A_VERSION).json(&body);
        request = if streamed {
            request.header(ACCEPT, "text/event-stream")
        } else {
            request.timeout(self.timeout)
        };
        if let Some(token) = self.token(security).await? {
            request = request.bearer_auth(token);
        }
        match timeout(self.timeout, request.send()).await {
            Ok(Ok(response)) => Ok(response),
            // It may have moved: its card is read again at the next request.
            Ok(Err(e)) => {
                self.interface.invalidate();
                Err(self.connection(e))
            }
            Err(_) => Err(ExternalAgentError::Connection { endpoint: self.shown(), reason: format!("no answer within {} s", self.timeout.as_secs()) }),
        }
    }
}

/// The responses of a stream of server-sent events, each a JSON-RPC
/// response carrying a `StreamResponse`, from the agent shown as
/// `endpoint`. Ends with the stream, or after an error - a JSON-RPC error
/// among them. The events are framed in bytes, so a character or a line
/// end the network splits arrives whole (issue 185).
fn responses_of<S, D, E>(bytes: S, endpoint: String) -> impl Stream<Item = ExternalAgentResult<StreamResponse>> + Send + 'static
where
    S: Stream<Item = Result<D, E>> + Send + 'static,
    D: Buf + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    async_stream::stream! {
        let mut sse = pin!(SseStream::from_bytes_stream(bytes));
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
            let response = serde_json::from_str::<RpcResponse<StreamResponse>>(&data)
                .map_err(|e| ExternalAgentError::InvalidResponse { endpoint: endpoint.clone(), reason: e.to_string() })
                .and_then(|answer| answer.into_result(&endpoint))
                .and_then(|answered| answered.map_err(|error| error.rejected(&endpoint)));
            let failed = response.is_err();
            yield response;
            if failed {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::wire::WireTaskState;
    use bytes::Bytes;
    use futures::stream::iter;
    use caelumora_model::security::AllowAllPolicy;
    use serde_json::{json, Value};
    use std::io::Error;

    fn client(url: &str) -> A2aClient {
        let endpoint: RemoteEndpoint = serde_json::from_value(json!({ "url": url })).unwrap();
        let secrets = Arc::new(SecretManager::new(vec![], Arc::new(AllowAllPolicy::new())));
        A2aClient::new("hermes".to_string(), endpoint, Duration::from_secs(1), Duration::from_secs(300), secrets).unwrap()
    }

    fn event(result: Value) -> String {
        format!("data: {}\r\n\r\n", json!({ "jsonrpc": "2.0", "id": "1", "result": result }))
    }

    /// `body` as the network may hand it over: each byte a chunk of its own.
    fn bytewise(body: String) -> impl Stream<Item = Result<Bytes, Error>> + Send + 'static {
        iter(body.into_bytes().into_iter().map(|b| Ok(Bytes::from(vec![b]))))
    }

    #[test]
    fn the_card_is_found_below_the_agent_or_where_its_own_url_says() {
        assert_eq!(client("https://hermes.example/").card_url(), "https://hermes.example/.well-known/agent-card.json");
        assert_eq!(client("https://hermes.example").card_url(), "https://hermes.example/.well-known/agent-card.json");
        assert_eq!(client("https://hermes.example/card.json").card_url(), "https://hermes.example/card.json", "a card's own URL");
    }

    #[tokio::test]
    async fn responses_split_anywhere_by_the_network_arrive_whole() {
        let body = [
            event(json!({ "task": { "id": "t-1", "contextId": "c-1", "status": { "state": "TASK_STATE_WORKING" } } })),
            event(json!({ "artifactUpdate": { "taskId": "t-1", "contextId": "c-1", "artifact": { "artifactId": "a", "parts": [{ "text": "Grüße" }] }, "lastChunk": true } })),
            event(json!({ "statusUpdate": { "taskId": "t-1", "contextId": "c-1", "status": { "state": "TASK_STATE_COMPLETED" } } })),
        ].concat();

        let responses: Vec<_> = responses_of(bytewise(body), "agent".to_string()).collect().await;
        let responses: Vec<_> = responses.into_iter().map(Result::unwrap).collect();
        assert_eq!(responses.len(), 3);
        assert!(responses[0].task.as_ref().is_some_and(|t| t.id == "t-1" && t.status.state.works()));
        let artifact = responses[1].artifact_update.as_ref().expect("an artifact");
        assert_eq!(artifact.artifact.parts[0].text.as_deref(), Some("Grüße"), "an \"ü\" split in two");
        assert!(artifact.last_chunk);
        assert!(responses[2].status_update.as_ref().is_some_and(|u| matches!(u.status.state, WireTaskState::Completed)));
    }

    #[tokio::test]
    async fn a_stream_ends_after_it_breaks_off_or_is_answered_with_an_error() {
        let working = event(json!({ "task": { "id": "t-1", "status": { "state": "TASK_STATE_WORKING" } } }));
        let broken = iter(vec![
            Ok(Bytes::from(working.clone())),
            Err(Error::other("connection reset")),
            Ok(Bytes::from(working.clone())),
        ]);
        let responses: Vec<_> = responses_of(broken, "agent".to_string()).collect().await;
        assert_eq!(responses.len(), 2, "what came before, then the error, then nothing");
        assert!(responses[0].is_ok());
        let error = responses[1].as_ref().expect_err("broken off").to_string();
        assert!(error.contains("cannot reach the external agent at agent") && error.contains("connection reset"), "{error}");

        let refused = format!("data: {}\r\n\r\n", json!({ "jsonrpc": "2.0", "id": "1", "error": { "code": -32603, "message": "boom" } }));
        let responses: Vec<_> = responses_of(bytewise([working.clone(), refused, working].concat()), "agent".to_string()).collect().await;
        assert_eq!(responses.len(), 2);
        let error = responses[1].as_ref().expect_err("refused").to_string();
        assert!(error.contains("rejected the request: boom (-32603)"), "{error}");
    }
}
