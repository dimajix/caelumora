//! The tools of `openapi_server` (issues 32, 126, 161, 188, 252, 552): the operations chosen of the
//! component's server, which is their `ToolSource`, each an `OpenApiTool` made of what was read of
//! the document; the server describes itself by the document's `info.description`, and says in
//! general terms why its operations could not be read (`Undiscovered`).
//!
//! Tools are named `"{descriptor_id}__{operationId}"`, as the tools of an MCP server are, so that
//! servers cannot collide. The document is read when the tools are first offered, in the name of
//! whoever acts there: of the server with the endpoint's credential, again once what was read is
//! older than the endpoint's `refresh_secs`; of a file once. A call is one request, whose
//! credential is resolved of the secrets of whoever calls (`Situation::secrets`, issue 516) for the
//! URL it goes to; an interrupted call drops it (issue 161). What it answers is the tool's result -
//! JSON as JSON, text as text, nothing as its status -, kept whole beside a preview above the
//! component's bound (issue 552); a status of failure is the call's failure, with what the server
//! said, cut to a preview's size.

use super::document::{self, DocumentError, Offered, Operation, Place, BODY};
use super::{Discovered, DocumentSource, OpenApiServer};
use async_trait::async_trait;
use caelumora_agents::tool::{
    cut_to_preview, Tool, ToolError, ToolOutput, ToolResult, ToolSource, Undiscovered,
};
use caelumora_agents::turn::Turn;
use caelumora_api::cache::ExpiringCache;
use caelumora_api::error::describe_request_error;
use caelumora_domain::question::Answers;
use caelumora_domain::remote::RemoteEndpoint;
use caelumora_domain::secret::{SecretError, Secrets};
use caelumora_domain::tool::{ToolContent, ToolDescription, ToolVisibility};
use reqwest::header::CONTENT_TYPE;
use reqwest::{Method, RequestBuilder, Response, StatusCode, Url};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use tokio::fs;

/// Why the document of a server could not be read.
#[derive(Debug, thiserror::Error)]
enum DiscoveryError {
    #[error("failed to reach the OpenAPI server at '{0}': {1}")]
    Unreachable(String, String),
    #[error("the OpenAPI server at '{0}' did not answer in time")]
    TimedOut(String),
    #[error("the OpenAPI server at '{0}' answered {1} when asked for its document")]
    Refused(String, StatusCode),
    /// What fails names no content of the document, and of a file not its path.
    #[error("the document cannot be read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Document(#[from] DocumentError),
    #[error(transparent)]
    Secret(#[from] SecretError),
}

impl From<DiscoveryError> for Undiscovered {
    /// Why the server's operations could not be read after `error`: in general terms for the
    /// model and the user (issue 32) - never the error, which may name hosts -, and the error for
    /// the log.
    fn from(error: DiscoveryError) -> Self {
        let reason = match &error {
            DiscoveryError::Unreachable(..) => "its server cannot be reached",
            DiscoveryError::TimedOut(_) => "its server did not answer in time",
            DiscoveryError::Refused(..) => "its server did not give its document",
            DiscoveryError::Unreadable(_) | DiscoveryError::Document(DocumentError::NotOpenApi) => {
                "its document cannot be read"
            }
            DiscoveryError::Document(_) => {
                "its document does not offer the operations it is configured with"
            }
            DiscoveryError::Secret(_) => "its credentials are not available",
        };
        Self { reason: reason.to_string(), cause: error.to_string() }
    }
}

/// Between the component id and the `operationId` of a tool's name.
const NAME_SEPARATOR: &str = "__";

#[async_trait]
impl ToolSource for OpenApiServer {
    type Discovery = Discovered;

    /// The operations chosen of the server's document, and what it says of the server, read in
    /// the name `secrets` are bound to.
    async fn discover(&self, secrets: &dyn Secrets) -> Result<Discovered, Undiscovered> {
        let document = match self.params.document() {
            DocumentSource::Served(path) => self.served_document(path, secrets).await?,
            DocumentSource::File(file) => file_document(file).await?,
        };
        let operations = document::operations(&document, self.params.operations.keys())
            .map_err(DiscoveryError::from)?;
        let described = document.pointer("/info/description").and_then(Value::as_str);
        let described = described.map(str::trim).filter(|text| !text.is_empty());
        let operations = operations.into_iter().map(|offered| self.offering(offered)).collect();
        Ok(Discovered { operations, described: described.map(str::to_string) })
    }

    fn discovered(&self) -> &ExpiringCache<Discovered> {
        &self.discovered
    }

    /// What the document says of the server in its `info.description`, if it does.
    fn described(discovery: &Discovered) -> Option<&str> {
        discovery.described.as_deref()
    }

    fn tools(self: &Arc<Self>, discovery: &Discovered) -> Vec<Arc<dyn Tool>> {
        discovery
            .operations
            .iter()
            .map(|(operation, description)| {
                Arc::new(OpenApiTool {
                    description: description.clone(),
                    operation: Arc::clone(operation),
                    server: Arc::clone(self),
                }) as Arc<dyn Tool>
            })
            .collect()
    }
}

/// The document in `file`, read as JSON.
///
/// # Errors
/// `Unreadable` for a file that cannot be read or holds no JSON, naming neither its path nor what
/// it holds.
async fn file_document(file: &Path) -> Result<Value, DiscoveryError> {
    let bytes =
        fs::read(file).await.map_err(|e| DiscoveryError::Unreadable(e.kind().to_string()))?;
    serde_json::from_slice(&bytes).map_err(|_| DiscoveryError::Unreadable("it is no JSON".into()))
}

impl OpenApiServer {
    /// The operation `offered` and how its tool is offered: named after the component, described
    /// as the document says, asking for approval as the component says of it alone, else of all -
    /// trusting `GET` and `HEAD` only to read where configured -, its results classified as
    /// configured.
    fn offering(&self, offered: Offered) -> (Arc<Operation>, ToolDescription) {
        let Offered { operation, text, arguments } = offered;
        let own = self.params.operations.get(&operation.id).and_then(|own| own.requires_approval);
        let configured = || self.params.requires_approval.required(operation.read_only());
        let description = ToolDescription {
            name: format!("{}{NAME_SEPARATOR}{}", self.descriptor_id, operation.id),
            description: text,
            parameters: arguments,
            requires_approval: own.unwrap_or_else(configured),
            visibility: ToolVisibility::SHOWN,
            classification: self.params.classification.clone(),
            presentation: Default::default(),
        };
        (Arc::new(operation), description)
    }

    /// A request of `method` to `url`, bounded by the endpoint's timeout, with the endpoint's
    /// credential, resolved of `secrets` for `url`.
    ///
    /// # Errors
    /// Whatever resolving the credential reports.
    async fn request(
        &self,
        method: Method,
        url: Url,
        secrets: &dyn Secrets,
    ) -> Result<RequestBuilder, SecretError> {
        let token = self.params.endpoint.bearer_token(secrets, url.as_str()).await?;
        let request = self.http.request(method, url).timeout(self.params.timeout());
        Ok(match token {
            Some(token) => request.bearer_auth(token),
            None => request,
        })
    }

    /// The document at `path` below the endpoint, read as JSON with the endpoint's credential.
    ///
    /// # Errors
    /// `Unreachable`, `TimedOut` or `Refused` for a server that does not give it, `Unreadable`
    /// for one that is no JSON, `Secret` for a credential that cannot be had.
    async fn served_document(
        &self,
        path: &str,
        secrets: &dyn Secrets,
    ) -> Result<Value, DiscoveryError> {
        let endpoint = &self.params.endpoint;
        let segments: Vec<&str> = path.split('/').filter(|segment| !segment.is_empty()).collect();
        let Some(url) = endpoint.url_of(&segments) else {
            let why = "its endpoint is no URL to go below".to_string();
            return Err(DiscoveryError::Unreachable(endpoint.shown(), why));
        };
        let unreached = |e: reqwest::Error| match e.is_timeout() {
            true => DiscoveryError::TimedOut(endpoint.shown()),
            false => DiscoveryError::Unreachable(endpoint.shown(), describe_request_error(e)),
        };
        let answer =
            self.request(Method::GET, url, secrets).await?.send().await.map_err(unreached)?;
        if !answer.status().is_success() {
            return Err(DiscoveryError::Refused(endpoint.shown(), answer.status()));
        }
        let bytes = answer.bytes().await.map_err(unreached)?;
        serde_json::from_slice(&bytes)
            .map_err(|_| DiscoveryError::Unreadable("it is no JSON".into()))
    }

    /// What the model of the agent is given of what the tool `name` was answered (issue 552): JSON
    /// as JSON, text as text, an answer without a body as its status. Above the component's bound,
    /// the text is kept whole as `<component id>.txt` beside a preview, JSON as
    /// `<component id>.json` without one, which would cut it in the middle.
    ///
    /// # Errors
    /// `ExecutionFailed` for a status of failure - with what the server said, cut to the size of
    /// a preview whether a bound is configured or not, unlike what an MCP tool's failure says: an
    /// HTTP server's failure is often a whole page of HTML -, for an answer that is neither JSON
    /// nor text, or one that broke off.
    async fn output_of(&self, name: &str, answer: Response) -> ToolResult<ToolOutput> {
        let failed = |why: String| ToolError::ExecutionFailed(self.params.endpoint.redact_in(&why));
        let status = answer.status();
        let media_type = answer
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(|media_type| media_type.trim().to_ascii_lowercase())
            .unwrap_or_default();
        let body = answer.bytes().await.map_err(|e| {
            failed(format!("the answer to '{name}' broke off: {}", describe_request_error(e)))
        })?;
        if !status.is_success() {
            let said = cut_to_preview(String::from_utf8_lossy(&body).into_owned());
            return Err(failed(format!("'{name}' failed with status {status}: {said}")));
        }
        if body.is_empty() {
            return Ok(json!({ "status": status.as_u16() }).into());
        }
        let json = document::is_json(&media_type);
        let text = match String::from_utf8(body.to_vec()) {
            Ok(text) if json || media_type.is_empty() || media_type.starts_with("text/") => text,
            _ => {
                let why = format!("'{name}' answered neither JSON nor text ('{media_type}')");
                return Err(failed(why));
            }
        };
        let value = match json || media_type.is_empty() {
            true => serde_json::from_str(&text).unwrap_or_else(|_| Value::String(text.clone())),
            false => Value::String(text.clone()),
        };
        let keep_whole_above = self.params.keep_whole_above_bytes;
        if !keep_whole_above.is_some_and(|above| above.keeps(&text)) {
            return Ok(value.into());
        }
        let is_text = value.is_string();
        let extension = if is_text { "txt" } else { "json" };
        let name = format!("{}.{extension}", self.descriptor_id);
        let kept = ToolContent { name, bytes: text.clone().into() };
        Ok(ToolOutput::kept(is_text.then_some(text.as_str()), vec![kept], &self.prompts))
    }
}

/// The URL a call of `operation` with `arguments` goes to below `endpoint`: its path with the
/// arguments of its parameters in the path, each as one segment, and those of the query, an array
/// as the parameter repeated.
///
/// # Errors
/// `InvalidParameter` for an argument of the path that is missing, `ExecutionFailed` for an
/// endpoint with no path to go below.
fn url_of(endpoint: &RemoteEndpoint, operation: &Operation, arguments: &Value) -> ToolResult<Url> {
    let given = |name: &str| arguments.get(name).filter(|value| !value.is_null());
    let mut segments: Vec<String> = Vec::new();
    for segment in operation.path.split('/').filter(|segment| !segment.is_empty()) {
        let mut segment = segment.to_string();
        for parameter in operation.parameters.iter().filter(|p| p.place == Place::Path) {
            let placeholder = format!("{{{}}}", parameter.sent_as);
            if segment.contains(&placeholder) {
                let Some(value) = given(&parameter.name) else {
                    let missing = format!("'{}' is required", parameter.name);
                    return Err(ToolError::InvalidParameter(missing));
                };
                segment = segment.replace(&placeholder, &argument_text(value));
            }
        }
        // A segment of nothing or of dots would be dropped or climb up as the URL is made, and
        // the call reach another path than its operation's.
        if segment.is_empty() || segment == "." || segment == ".." {
            let why = format!("a value of the path of '{}' is empty, '.' or '..'", operation.id);
            return Err(ToolError::InvalidParameter(why));
        }
        segments.push(segment);
    }
    let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
    let Some(mut url) = endpoint.url_of(&segments) else {
        let why = format!("the endpoint of '{}' is no URL to go below", operation.id);
        return Err(ToolError::ExecutionFailed(why));
    };
    let query: Vec<(&str, String)> = operation
        .parameters
        .iter()
        .filter(|parameter| parameter.place == Place::Query)
        .flat_map(|parameter| {
            let values = match given(&parameter.name) {
                Some(Value::Array(values)) => values.iter().map(argument_text).collect(),
                Some(value) => vec![argument_text(value)],
                None => Vec::new(),
            };
            values.into_iter().map(|value| (parameter.sent_as.as_str(), value))
        })
        .collect();
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    Ok(url)
}

/// `value` as a parameter sends it: a string as it is, anything else as JSON.
fn argument_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        value => value.to_string(),
    }
}

/// An operation of the server, offered as `<component id>__<operationId>`.
#[derive(Debug)]
struct OpenApiTool {
    description: ToolDescription,
    operation: Arc<Operation>,
    server: Arc<OpenApiServer>,
}

#[async_trait]
impl Tool for OpenApiTool {
    fn description(&self) -> &ToolDescription {
        &self.description
    }

    async fn execute(
        &self,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn<'_>,
    ) -> ToolResult<ToolOutput> {
        let server = &self.server;
        let name = &self.description.name;
        let secrets = turn.situation().secrets();
        let url = url_of(&server.params.endpoint, &self.operation, params)?;
        let request = server
            .request(self.operation.method.clone(), url, &*secrets)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        let body = params.get(BODY).filter(|body| self.operation.body && !body.is_null());
        let request = match body {
            Some(body) => request.json(body),
            None => request,
        };
        // An interrupted call is dropped here (issue 161), and its request with it.
        let answer = request.send().await.map_err(|e| {
            let why = describe_request_error(e);
            ToolError::ExecutionFailed(format!("'{name}' did not reach its server: {why}"))
        })?;
        server.output_of(name, answer).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::document::Parameter;
    use crate::component::OpenApiServerParams;
    use caelumora_agents::testing::prompts;
    use caelumora_api::component::Params;
    use caelumora_domain::secret::SecretResult;
    use caelumora_domain::tool::ToolParameters;
    use caelumora_security::secret::{SecretManager, SecretProvider};
    use caelumora_security::{AllowAllPolicy, SecurityContext};
    use std::time::Duration;

    /// An operation of `method` at `path` with `parameters`, each `(name as sent, place)`.
    fn operation(method: Method, path: &str, parameters: &[(&str, Place)]) -> Operation {
        let parameters = parameters
            .iter()
            .map(|(sent_as, place)| Parameter {
                name: sent_as.strip_suffix("[]").unwrap_or(sent_as).to_string(),
                sent_as: sent_as.to_string(),
                place: *place,
            })
            .collect();
        Operation { id: "op".to_string(), method, path: path.to_string(), parameters, body: false }
    }

    /// `operation`, named `id`, as it was read, described by `text` and without arguments.
    fn offered(id: &str, operation: Operation) -> Offered {
        let operation = Operation { id: id.to_string(), ..operation };
        Offered { operation, text: format!("does {id}"), arguments: ToolParameters::none() }
    }

    fn kimai() -> RemoteEndpoint {
        RemoteEndpoint::new("https://kimai.example")
    }

    #[test]
    fn a_call_goes_to_its_path_with_its_arguments_in_the_path_and_the_query() {
        let timesheets = operation(
            Method::GET,
            "/api/projects/{project}/timesheets",
            &[("project", Place::Path), ("users[]", Place::Query), ("order", Place::Query)],
        );
        let arguments = json!({ "project": "a/b c", "users": [1, 2], "order": "ASC" });
        let url = url_of(&kimai(), &timesheets, &arguments).unwrap();
        assert_eq!(
            url.as_str(),
            "https://kimai.example/api/projects/a%2Fb%20c/timesheets?users%5B%5D=1&users%5B%5D=2&order=ASC",
            "a value of the path one segment, an array the name repeated as written"
        );
        let unasked = json!({ "project": 7, "order": null });
        let url = url_of(&kimai(), &timesheets, &unasked).unwrap();
        assert_eq!(url.as_str(), "https://kimai.example/api/projects/7/timesheets");
        let below = RemoteEndpoint::new("https://example.org/kimai/");
        let url = url_of(&below, &timesheets, &unasked).unwrap();
        assert_eq!(url.as_str(), "https://example.org/kimai/api/projects/7/timesheets");
    }

    #[test]
    fn a_call_without_an_argument_of_its_path_or_one_that_leaves_it_is_the_models_to_correct() {
        let stop = operation(Method::PATCH, "/api/timesheets/{id}/stop", &[("id", Place::Path)]);
        let refused = |arguments: Value| match url_of(&kimai(), &stop, &arguments) {
            Err(ToolError::InvalidParameter(why)) => why,
            other => panic!("refused: {other:?}"),
        };
        assert!(refused(json!({})).contains("'id'"));
        for leaving in ["..", ".", ""] {
            let why = refused(json!({ "id": leaving }));
            assert!(why.contains("empty, '.' or '..'"), "{leaving:?}: {why}");
        }
        let url = url_of(&kimai(), &stop, &json!({ "id": "..." })).unwrap();
        assert_eq!(url.as_str(), "https://kimai.example/api/timesheets/.../stop", "a name");
    }

    /// The server of the component `kimai` configured with `requires_approval` for all and the
    /// operations `own` says of each.
    fn server(requires_approval: Value, own: Value) -> OpenApiServer {
        let params = json!({
            "endpoint": { "url": "https://kimai.example" },
            "operations": own,
            "requires_approval": requires_approval,
        });
        let params = OpenApiServerParams::read(&params).unwrap();
        OpenApiServer::new("kimai".to_string(), params, prompts()).unwrap()
    }

    #[test]
    fn an_operation_asks_for_approval_as_configured_for_it_else_for_all() {
        let own = json!({ "get_users": { "requires_approval": true }, "op": {} });
        for (approval, asks) in [
            (json!(true), [true, true]),
            (json!(false), [false, false]),
            (json!("unless_read_only"), [false, true]),
        ] {
            let server = server(approval.clone(), own.clone());
            let asks_for = |method: Method| {
                let (_, description) = server.offering(offered("op", operation(method, "/", &[])));
                description.requires_approval
            };
            assert_eq!([asks_for(Method::GET), asks_for(Method::DELETE)], asks, "{approval}");
            let users = offered("get_users", operation(Method::GET, "/", &[]));
            assert!(server.offering(users).1.requires_approval, "its own, over {approval}");
        }
        let server = server(json!(true), json!({ "stop": { "requires_approval": false } }));
        let (operation, description) =
            server.offering(offered("stop", operation(Method::PATCH, "/", &[])));
        assert!(!description.requires_approval, "its own, over all");
        assert_eq!(description.name, "kimai__stop");
        assert_eq!(description.description, "does stop");
        assert_eq!(operation.id, "stop");
    }

    /// The one secret of the tests: the tracker's token.
    #[derive(Debug)]
    struct Token;

    #[async_trait]
    impl SecretProvider for Token {
        async fn get_secret(&self, key: &str) -> SecretResult<Option<String>> {
            Ok((key == "TRACKER_TOKEN").then(|| "t0ken".to_string()))
        }
    }

    /// The server of the component `kimai` at `url`, its credential `secret`.
    fn authenticated(url: &str, secret: &str) -> OpenApiServer {
        let auth = json!({ "type": "bearer", "secret": secret });
        let params =
            json!({ "endpoint": { "url": url, "auth": auth }, "operations": { "op": {} } });
        let params = OpenApiServerParams::read(&params).unwrap();
        OpenApiServer::new("kimai".to_string(), params, prompts()).unwrap()
    }

    #[tokio::test]
    async fn a_request_carries_the_endpoints_credential_and_the_endpoints_timeout() {
        let server = authenticated("https://kimai.example", "test/TRACKER_TOKEN");
        let policy = Arc::new(AllowAllPolicy::new());
        let manager = Arc::new(SecretManager::of_test_vault(Arc::new(Token), policy));
        let secrets = manager.for_context(SecurityContext::default());
        let url = Url::parse("https://kimai.example/api/version").unwrap();
        let request = server.request(Method::GET, url, &secrets).await.unwrap().build().unwrap();
        let authorization = request.headers().get("authorization").unwrap();
        assert_eq!(authorization, "Bearer t0ken");
        assert_eq!(request.timeout(), Some(&Duration::from_secs(10)), "the endpoint's timeout");
    }

    /// Issue 310: the credential is resolved for the URL the request goes to, which its vault's
    /// origins have to hold, not for the endpoint's.
    #[tokio::test]
    async fn a_credential_is_resolved_for_where_its_request_goes() {
        let server = authenticated("https://bound.example", "bound/TRACKER_TOKEN");
        let manager = Arc::new(SecretManager::of_test_vaults(Arc::new(AllowAllPolicy::new())));
        let secrets = manager.for_context(SecurityContext::default());
        let elsewhere = Url::parse("https://elsewhere.example/api/version").unwrap();
        let refused = server.request(Method::GET, elsewhere, &secrets).await.unwrap_err();
        assert!(matches!(refused, SecretError::NotSentTo { .. }), "{refused}");
        let bound = Url::parse("https://bound.example/api/version").unwrap();
        let unkept = server.request(Method::GET, bound, &secrets).await.unwrap_err();
        assert!(matches!(unkept, SecretError::NotFound(_)), "may go there, has no key: {unkept}");
    }
}
