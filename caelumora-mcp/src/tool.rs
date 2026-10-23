use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::question::Answers;
use caelumora_model::remote::RemoteEndpoint;
use caelumora_model::secret::{SecretError, SecretManager};
use caelumora_model::security::SecurityContext;
use caelumora_model::tool::{Tool, ToolParameters, ToolVisibility};
use caelumora_agents::turn::Turn;
use caelumora_agents::situation::Situation;
use caelumora_runtime::component::{config_of_params, ComponentFacets, ComponentKind, ComponentRegistry};
use caelumora_runtime::remote::ExpiringCache;
use caelumora_agents::tool::{ApprovalMode, ToolError, ToolOutput, ToolProvider, ToolResult};
use rmcp::model::{CallToolRequestParams, CallToolResult, ErrorCode, ToolAnnotations};
use rmcp::service::{RunningService, ServiceError};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{RoleClient, ServiceExt};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;
use tokio::time;

pub const MCP_TOOL_KIND: &str = "mcp_tool";

#[derive(Debug, thiserror::Error)]
enum McpError {
    #[error("failed to connect to MCP server at '{0}': {1}")]
    Connect(String, String),
    #[error("failed to list tools from MCP server at '{0}': {1}")]
    ListTools(String, String),
    #[error("MCP operation timed out: {0}")]
    Timeout(String),
    #[error("MCP tool call failed: {0}")]
    ToolCallFailed(String),
    #[error(transparent)]
    Secret(#[from] SecretError),
}

type McpResult<T> = Result<T, McpError>;

/// How long a call or the discovery of the tools may take unless the
/// endpoint says otherwise.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the tools discovered are kept before the server is asked for
/// them again, unless the endpoint says otherwise (issue 126).
const DEFAULT_REFRESH: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpToolConfig {
    /// The MCP server. Its timeout is how long a call of one of its tools,
    /// or the discovery of them, may take; its `refresh_secs` how long the
    /// tools discovered are kept.
    pub endpoint: RemoteEndpoint,
    /// Whether calls to this MCP server's tools require human approval.
    /// Left out, they do: a server may act outside, and what makes an agent
    /// call it may be a consent nobody gave (issue 150).
    #[serde(default)]
    pub requires_approval: ApprovalMode,
    /// How the results of this server's tools are classified (issue 133):
    /// a result that may not flow into the agent's channel is withheld
    /// from the agent. Left out, as the channel.
    #[serde(default)]
    pub classification: Option<Classification>,
    /// What this server's tools are for, written for a model: the
    /// description of their tool set (issue 194). Left out, what the server
    /// says of itself in its `instructions`, else a text naming the
    /// component.
    #[serde(default)]
    pub description: Option<String>,
    /// What the server is cleared for (issue 180): where the arguments of
    /// its tools go. Its tools are not offered in a channel that may not
    /// flow there. Left out, public: the server's word is trusted only
    /// where it is configured.
    #[serde(default)]
    pub destination: Option<Classification>,
}

impl McpError {
    /// Why the server's tools are unavailable after this error, in general
    /// terms for the model and the user (issue 32) - never the error, which
    /// may name hosts. That is in the log.
    fn unavailable(&self) -> String {
        match self {
            McpError::Connect(..) => "its server cannot be reached",
            McpError::ListTools(..) => "its server did not list its tools",
            McpError::Timeout(_) => "its server did not answer in time",
            McpError::ToolCallFailed(_) => "its server failed",
            McpError::Secret(_) => "its credentials are not available",
        }.to_string()
    }
}

/// Whether a tool the server annotated with `annotations` only reads, as
/// it says (issue 246): a tool without `readOnlyHint` does not, as MCP
/// says. That a server says so is trusted only where configured
/// (`ApprovalMode::UnlessReadOnly`): it may be wrong, or lie.
fn read_only(annotations: Option<&ToolAnnotations>) -> bool {
    annotations.and_then(|a| a.read_only_hint).unwrap_or(false)
}

impl McpToolConfig {
    fn timeout(&self) -> Duration {
        self.endpoint.timeout_or(DEFAULT_TIMEOUT)
    }

    fn refresh(&self) -> Duration {
        self.endpoint.refresh_or(DEFAULT_REFRESH)
    }
}

/// Exposes the tools of a single remote MCP server (reached over the
/// Streamable HTTP transport) as a `ToolProvider`: one tool set, described
/// as configured or as the server describes itself, and unavailable - with
/// why, in general terms - if the server could not be asked for its tools.
///
/// Tools are namespaced as `"{descriptor_id}__{remote_tool_name}"` to avoid
/// collisions with built-in tools or other MCP servers, since an agent's
/// tools are found by name across all its providers. No connection is held
/// open between calls: the tools are discovered when they are first offered,
/// in the name of whoever acts, and again once what was discovered is older
/// than the endpoint's `refresh_secs`, or after the server did not know a
/// tool that was called (issue 126); `execute()` opens a fresh connection
/// per call (the Streamable HTTP transport supports stateless operation),
/// which keeps this provider free of any stale-connection state.
#[derive(Debug)]
pub struct McpToolProvider {
    descriptor_id: String,
    config: McpToolConfig,
    /// The server's tools, and what it says of itself (`instructions`) if
    /// it does, as last discovered.
    discovered: ExpiringCache<(Vec<Tool>, Option<String>)>,
    secret_manager: Arc<SecretManager>,
}

/// Between the component id and the remote name of a tool: an MCP tool is
/// offered as `<component id>__<remote name>`.
const NAME_SEPARATOR: &str = "__";

impl McpToolProvider {
    /// The provider of the component `descriptor_id` with `config`; its
    /// tools are not discovered yet.
    fn new(descriptor_id: String, config: McpToolConfig, secret_manager: Arc<SecretManager>) -> Self {
        let discovered = ExpiringCache::new(config.refresh());
        Self { descriptor_id, config, discovered, secret_manager }
    }

    fn strip_prefix<'a>(&self, name: &'a str) -> ToolResult<&'a str> {
        name.strip_prefix(self.descriptor_id.as_str())
            .and_then(|rest| rest.strip_prefix(NAME_SEPARATOR))
            .ok_or_else(|| ToolError::UnknownTool(name.to_string()))
    }

    /// The server's tools and what it says of itself, discovered in
    /// `security` within the endpoint's timeout.
    async fn discover(&self, security: &SecurityContext) -> McpResult<(Vec<Tool>, Option<String>)> {
        let discover = async {
            let auth_header = self.config.endpoint.bearer_token(&self.secret_manager, security).await?;
            discover_tools(&self.descriptor_id, &self.config, auth_header).await
        };
        time::timeout(self.config.timeout(), discover).await
            .unwrap_or_else(|_| Err(McpError::Timeout(format!("Timed out discovering tools from MCP server at '{}'", self.config.endpoint.shown()))))
    }
}

/// A connection to the MCP server at `endpoint`. Its errors show the
/// endpoint without credentials, also where `rmcp` writes it into its own
/// messages (issue 188).
async fn connect(endpoint: &RemoteEndpoint, auth_header: Option<String>) -> McpResult<RunningService<RoleClient, ()>> {
    let mut transport_config = StreamableHttpClientTransportConfig::with_uri(endpoint.url.clone());
    if let Some(token) = auth_header {
        transport_config = transport_config.auth_header(token);
    }
    let transport = StreamableHttpClientTransport::from_config(transport_config);
    let service = ()
        .serve(transport)
        .await
        .map_err(|e| McpError::Connect(endpoint.shown(), endpoint.redact_in(&e.to_string())))?;
    Ok(service)
}

/// The tools of the MCP server `config` names, namespaced by `descriptor_id`
/// and requiring approval as `config` says (issue 78) - tool by tool, from
/// what the server says of each (issue 246) - with what the server
/// says of itself in its `instructions`, if anything (issue 194).
async fn discover_tools(descriptor_id: &str, config: &McpToolConfig, auth_header: Option<String>) -> McpResult<(Vec<Tool>, Option<String>)> {
    let endpoint = &config.endpoint;
    let service = connect(endpoint, auth_header).await?;
    let instructions = service.peer_info()
        .and_then(|info| info.instructions.clone())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    let remote_tools = service
        .list_all_tools()
        .await
        .map_err(|e| McpError::ListTools(endpoint.shown(), endpoint.redact_in(&e.to_string())));
    let _ = service.cancel().await;
    let remote_tools = remote_tools?;

    let tools = remote_tools
        .into_iter()
        .map(|t| {
            let description = t.description.map(|d| d.to_string()).unwrap_or_default();
            let schema = Value::Object((*t.input_schema).clone());
            Tool {
                name: format!("{}{}{}", descriptor_id, NAME_SEPARATOR, t.name),
                description,
                parameters: schema_to_tool_parameters(&schema),
                requires_approval: config.requires_approval.required(read_only(t.annotations.as_ref())),
                visibility: ToolVisibility::SHOWN,
                classification: config.classification.clone(),
                presentation: Default::default(),
            }
        })
        .collect();
    Ok((tools, instructions))
}

/// Best-effort conversion of a JSON-Schema fragment (as used by MCP tool
/// `input_schema`s) into `ToolParameters`. Recognizes object/string/number/
/// array/boolean structurally; anything else (enums, oneOf, tuples, ...)
/// falls back to a string parameter with a description hint, since
/// `ToolParameters` does not model full JSON Schema.
fn schema_to_tool_parameters(schema: &Value) -> ToolParameters {
    let Some(obj) = schema.as_object() else {
        return ToolParameters::string().described("Unrecognized parameter schema");
    };

    let description = obj.get("description").and_then(|v| v.as_str()).map(|s| s.to_string());

    match obj.get("type").and_then(|v| v.as_str()) {
        Some("object") => {
            let properties = obj
                .get("properties")
                .and_then(|v| v.as_object())
                .map(|props| {
                    props
                        .iter()
                        .map(|(k, v)| (k.clone(), schema_to_tool_parameters(v)))
                        .collect()
                })
                .unwrap_or_default();
            let required = obj
                .get("required")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();
            ToolParameters::Object { properties, required }
        }
        Some("array") => {
            let items = obj
                .get("items")
                .map(schema_to_tool_parameters)
                .unwrap_or_else(|| ToolParameters::string().described("Array item"));
            ToolParameters::Array {
                items: Box::new(items),
                description,
            }
        }
        Some("string") => ToolParameters::String { description, language: None },
        Some("number") | Some("integer") => ToolParameters::Number { description },
        Some("boolean") => ToolParameters::Boolean { description },
        _ => ToolParameters::String {
            description: Some(description.unwrap_or_else(|| "Unrecognized parameter schema".to_string())),
            language: None,
        },
    }
}

fn call_tool_result_to_value(name: &str, result: CallToolResult) -> McpResult<Value> {
    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n");

    if result.is_error == Some(true) {
        let message = if !text.is_empty() {
            text
        } else {
            serde_json::to_string(&result.content).unwrap_or_default()
        };
        return Err(McpError::ToolCallFailed(format!("MCP tool '{}' returned an error: {}", name, message)));
    }

    if let Some(structured) = result.structured_content {
        return Ok(structured);
    }

    if !text.is_empty() {
        Ok(Value::String(text))
    } else {
        Ok(serde_json::to_value(&result.content).unwrap_or(Value::Null))
    }
}

#[async_trait]
impl ToolProvider for McpToolProvider {
    fn description(&self) -> String {
        self.config.description.clone()
            .or_else(|| self.discovered.current().and_then(|discovered| discovered.1.clone()))
            .unwrap_or_else(|| format!("The tools of the MCP server '{}'.", self.descriptor_id))
    }

    /// Its tools as last discovered; none before they were.
    fn tools(&self) -> Vec<Tool> {
        self.discovered.current().map(|discovered| discovered.0.clone()).unwrap_or_default()
    }

    /// Its tools wherever it is asked, discovered again in the name of
    /// whoever acts there if what was discovered is outdated. A server that
    /// cannot be asked leaves the tools discovered before (issue 126), or,
    /// without any, its tool set unavailable (issue 32) - and is asked again
    /// the next time.
    async fn offered(&self, situation: &Situation<'_>) -> Result<Vec<Tool>, String> {
        match self.discovered.get(|| self.discover(situation.security_context())).await {
            Ok(discovered) => Ok(discovered.0.clone()),
            Err(e) => match self.discovered.current() {
                Some(discovered) => {
                    warn!("MCP tool component '{}' failed to discover its tools again ({}); keeping those discovered before", self.descriptor_id, e);
                    Ok(discovered.0.clone())
                }
                None => {
                    warn!("MCP tool component '{}' failed to connect/discover tools ({}); no tools available", self.descriptor_id, e);
                    Err(e.unavailable())
                }
            },
        }
    }

    fn destination(&self) -> Option<Classification> {
        Some(self.config.destination.clone().unwrap_or(Classification::PUBLIC))
    }

    async fn execute(&self, name: &str, params: &Value, _answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput> {
        let remote_name = self.strip_prefix(name)?.to_string();
        let security_context = turn.situation().security_context().clone();

        let call_result: McpResult<Value> = async {
            let auth_header = self.config.endpoint.bearer_token(&self.secret_manager, &security_context).await?;

            let service = connect(&self.config.endpoint, auth_header).await?;
            let mut request = CallToolRequestParams::new(remote_name.clone());
            if let Some(arguments) = params.as_object().cloned() {
                request = request.with_arguments(arguments);
            }
            // An interrupted call is dropped here (issue 161), and `service`
            // with it: rmcp then aborts the request and ends the session in
            // the background - this call's alone - which tells the server
            // as much as `notifications/cancelled` would.
            let call = service.call_tool(request);

            let result = time::timeout(self.config.timeout(), call).await;
            let _ = service.cancel().await;

            let result = result.map_err(|_| McpError::Timeout(format!("MCP tool call to '{}' timed out", name)))?
                .map_err(|e| {
                    // A server that does not know the tool answers so (MCP's
                    // "Unknown tool"): it has others now.
                    if matches!(&e, ServiceError::McpError(error) if error.code == ErrorCode::INVALID_PARAMS) {
                        self.discovered.invalidate();
                    }
                    McpError::ToolCallFailed(format!("MCP tool call to '{}' failed: {}", name, self.config.endpoint.redact_in(&e.to_string())))
                })?;

            call_tool_result_to_value(name, result)
        }.await;

        call_result.map(ToolOutput::from).map_err(|e| ToolError::ExecutionFailed(e.to_string()))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(MCP_TOOL_KIND).provides::<dyn ToolProvider>().uses::<SecretManager>(),
        |descriptor, context| {
            let config: McpToolConfig = config_of_params(&descriptor.params)?;
            let secret_manager = Arc::clone(context.service::<SecretManager>()?);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(McpToolProvider::new(descriptor.id.clone(), config, secret_manager)));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::confidentiality::Confidentiality;
    use caelumora_model::remote::RemoteAuthConfig;
    use caelumora_model::security::AllowAllPolicy;
    use serde_json::json;

    #[tokio::test]
    async fn an_unreachable_server_is_named_without_what_may_carry_a_credential() {
        let endpoint = RemoteEndpoint::new("http://user:secret@127.0.0.1:1/mcp?key=secret");
        let Err(error) = connect(&endpoint, None).await else { panic!("nobody there") };
        let error = error.to_string();
        assert!(error.contains("'http://127.0.0.1:1/mcp'") && !error.contains("secret"), "{error}");
    }

    #[test]
    fn parses_bearer_auth_config() {
        let value = json!({ "endpoint": {
            "url": "http://127.0.0.1:1234/mcp",
            "auth": { "type": "bearer", "secret": "MY_TOKEN" },
            "timeout_secs": 5
        } });
        let config: McpToolConfig = serde_json::from_value(value).unwrap();
        assert_eq!(config.endpoint.url, "http://127.0.0.1:1234/mcp");
        assert_eq!(config.timeout(), Duration::from_secs(5));
        match config.endpoint.auth {
            Some(RemoteAuthConfig::Bearer { secret }) => assert_eq!(secret, "MY_TOKEN"),
            _ => panic!("Expected Bearer auth"),
        }
    }

    #[test]
    fn parses_config_without_auth_using_defaults() {
        let value = json!({ "endpoint": { "url": "http://127.0.0.1:1234/mcp" } });
        let config: McpToolConfig = serde_json::from_value(value).unwrap();
        assert!(config.endpoint.auth.is_none());
        assert_eq!(config.timeout(), Duration::from_secs(10));
        assert_eq!(config.refresh(), Duration::from_secs(300));
        assert_eq!(config.requires_approval, ApprovalMode::Always, "approval unless configured otherwise");
        assert!(serde_json::from_value::<McpToolConfig>(json!({ "endpoint": { "url": "x" }, "requires_aproval": true })).is_err(), "nothing unknown");
    }

    #[test]
    fn approval_is_configured_for_all_none_or_all_but_the_read_only_tools() {
        let configured = |approval: Value| serde_json::from_value::<McpToolConfig>(json!({ "endpoint": { "url": "x" }, "requires_approval": approval })).map(|c| c.requires_approval);
        assert_eq!(configured(json!("unless_read_only")).unwrap(), ApprovalMode::UnlessReadOnly);
        let refused = configured(json!("hints")).unwrap_err().to_string();
        assert!(refused.contains("requires_approval must be"), "names the field: {refused}");

        let read_only = ToolAnnotations::new().read_only(true);
        let writes = ToolAnnotations::new().read_only(false);
        let unannotated = ToolAnnotations::new().destructive(false);
        for (approval, asks) in [(ApprovalMode::Always, [true, true, true, true]), (ApprovalMode::Never, [false; 4]), (ApprovalMode::UnlessReadOnly, [false, true, true, true])] {
            let required = [Some(&read_only), Some(&writes), Some(&unannotated), None].map(|a| approval.required(super::read_only(a)));
            assert_eq!(required, asks, "{approval:?}: read-only, writing, without readOnlyHint, without annotations");
        }
    }

    #[test]
    fn namespacing_round_trips_through_prefix_stripping() {
        let config = McpToolConfig {
            endpoint: RemoteEndpoint::new("http://example.invalid/mcp"),
            requires_approval: ApprovalMode::Never,
            classification: None,
            description: None,
            destination: None,
        };
        let secret_manager = Arc::new(SecretManager::new(vec![], Arc::new(AllowAllPolicy::new())));
        let provider = McpToolProvider::new("github-mcp".to_string(), config, Arc::clone(&secret_manager));

        let stripped = provider.strip_prefix("github-mcp__search").unwrap();
        assert_eq!(stripped, "search");
        assert!(provider.strip_prefix("unrelated_tool").is_err());

        // Issue 180: the server is cleared for what is configured, public
        // unless configured.
        assert_eq!(provider.destination(), Some(Classification::PUBLIC));
        let internal = Classification::general(Confidentiality::Internal);
        let cleared = McpToolProvider::new("github-mcp".to_string(), McpToolConfig { destination: Some(internal.clone()), ..provider.config.clone() }, secret_manager);
        assert_eq!(cleared.destination(), Some(internal));
        let configured: McpToolConfig = serde_json::from_value(json!({ "endpoint": { "url": "x" }, "destination": { "level": "confidential" } })).unwrap();
        assert!(configured.destination.is_some(), "read from the params");
    }

    #[test]
    fn schema_conversion_recognizes_basic_types() {
        let schema = json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "limit": { "type": "number" },
                "verbose": { "type": "boolean" },
                "tags": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["query"]
        });

        let params = schema_to_tool_parameters(&schema);
        match params {
            ToolParameters::Object { properties, required } => {
                assert_eq!(required, vec!["query".to_string()]);
                assert!(matches!(properties.get("query"), Some(ToolParameters::String { .. })));
                assert!(matches!(properties.get("limit"), Some(ToolParameters::Number { .. })));
                assert!(matches!(properties.get("verbose"), Some(ToolParameters::Boolean { .. })));
                assert!(matches!(properties.get("tags"), Some(ToolParameters::Array { .. })));
            }
            _ => panic!("Expected Object"),
        }
    }

    #[test]
    fn schema_conversion_falls_back_to_string_for_unrecognized_schema() {
        let schema = json!({ "oneOf": [{ "type": "string" }, { "type": "number" }] });
        let params = schema_to_tool_parameters(&schema);
        assert!(matches!(params, ToolParameters::String { .. }));
    }
}
