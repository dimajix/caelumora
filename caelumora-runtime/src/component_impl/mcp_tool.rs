use crate::agent_turn::Turn;
use crate::component::{ComponentError, ComponentFacets, ComponentRegistry, ComponentKind};
use crate::tool::{ToolError, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::secret::{SecretError, SecretManager};
use caelumora_model::security::SecurityContext;
use caelumora_model::tool::{Tool, ToolConfidentiality, ToolParameters};
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{RoleClient, ServiceExt};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

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

fn default_timeout_secs() -> u64 {
    10
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum McpAuthConfig {
    Bearer { secret: String },
}

#[derive(Debug, Clone, Deserialize)]
struct McpToolConfig {
    pub url: String,
    #[serde(default)]
    pub auth: Option<McpAuthConfig>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Whether calls to this MCP server's tools require human approval.
    #[serde(default)]
    pub requires_approval: bool,
}

/// Exposes the tools of a single remote MCP server (reached over the
/// Streamable HTTP transport) as a `ToolProvider`.
///
/// Tools are namespaced as `"{descriptor_id}__{remote_tool_name}"` to avoid
/// collisions with built-in tools or other MCP servers, since
/// `DefaultToolExecutor` dispatches by first matching name across all
/// providers. No connection is held open between calls: the tool list is
/// discovered once at construction and cached, and `execute()` opens a fresh
/// connection per call (the Streamable HTTP transport supports stateless
/// operation), which keeps this provider free of any stale-connection state.
#[derive(Debug)]
pub struct McpToolProvider {
    descriptor_id: String,
    config: McpToolConfig,
    cached_tools: Vec<Tool>,
    secret_manager: Arc<SecretManager>,
}

/// Between the component id and the remote name of a tool: an MCP tool is
/// offered as `<component id>__<remote name>`.
const NAME_SEPARATOR: &str = "__";

impl McpToolProvider {
    fn strip_prefix<'a>(&self, name: &'a str) -> ToolResult<&'a str> {
        name.strip_prefix(self.descriptor_id.as_str())
            .and_then(|rest| rest.strip_prefix(NAME_SEPARATOR))
            .ok_or_else(|| ToolError::UnknownTool(name.to_string()))
    }
}

async fn resolve_auth_header(
    auth: &Option<McpAuthConfig>,
    secret_manager: &SecretManager,
    security_context: &SecurityContext,
) -> McpResult<Option<String>> {
    match auth {
        None => Ok(None),
        Some(McpAuthConfig::Bearer { secret }) => {
            let token = secret_manager.resolve(security_context, secret).await?;
            Ok(Some(token))
        }
    }
}

async fn connect(url: &str, auth_header: Option<String>) -> McpResult<RunningService<RoleClient, ()>> {
    let mut transport_config = StreamableHttpClientTransportConfig::with_uri(url.to_string());
    if let Some(token) = auth_header {
        transport_config = transport_config.auth_header(token);
    }
    let transport = StreamableHttpClientTransport::from_config(transport_config);
    let service = ()
        .serve(transport)
        .await
        .map_err(|e| McpError::Connect(url.to_string(), e.to_string()))?;
    Ok(service)
}

/// The tools of the MCP server `config` names, namespaced by `descriptor_id`
/// and requiring approval as `config` says (issue 78).
async fn discover_tools(descriptor_id: &str, config: &McpToolConfig, auth_header: Option<String>) -> McpResult<Vec<Tool>> {
    let url = config.url.as_str();
    let service = connect(url, auth_header).await?;
    let remote_tools = service
        .list_all_tools()
        .await
        .map_err(|e| McpError::ListTools(url.to_string(), e.to_string()));
    let _ = service.cancel().await;
    let remote_tools = remote_tools?;

    Ok(remote_tools
        .into_iter()
        .map(|t| {
            let description = t.description.map(|d| d.to_string()).unwrap_or_default();
            let schema = Value::Object((*t.input_schema).clone());
            Tool {
                name: format!("{}{}{}", descriptor_id, NAME_SEPARATOR, t.name),
                description,
                parameters: schema_to_tool_parameters(&schema),
                requires_approval: config.requires_approval,
                confidentiality: ToolConfidentiality::PUBLIC,
            }
        })
        .collect())
}

/// Best-effort conversion of a JSON-Schema fragment (as used by MCP tool
/// `input_schema`s) into `ToolParameters`. Recognizes object/string/number/
/// array/boolean structurally; anything else (enums, oneOf, tuples, ...)
/// falls back to a string parameter with a description hint, since
/// `ToolParameters` does not model full JSON Schema.
fn schema_to_tool_parameters(schema: &Value) -> ToolParameters {
    let Some(obj) = schema.as_object() else {
        return ToolParameters::string("Unrecognized parameter schema");
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
                .unwrap_or_else(|| ToolParameters::string("Array item"));
            ToolParameters::Array {
                items: Box::new(items),
                description,
            }
        }
        Some("string") => ToolParameters::String { description },
        Some("number") | Some("integer") => ToolParameters::Number { description },
        Some("boolean") => ToolParameters::Boolean { description },
        _ => ToolParameters::String {
            description: Some(description.unwrap_or_else(|| "Unrecognized parameter schema".to_string())),
        },
    }
}

fn call_tool_result_to_value(name: &str, result: rmcp::model::CallToolResult) -> McpResult<Value> {
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
    fn tools(&self) -> Vec<Tool> {
        self.cached_tools.clone()
    }

    async fn execute(&self, name: &str, params: &Value, turn: &mut Turn) -> ToolResult<Value> {
        let remote_name = self.strip_prefix(name)?.to_string();
        let security_context = turn.security_context().clone();

        let call_result: McpResult<Value> = async {
            let auth_header = resolve_auth_header(&self.config.auth, &self.secret_manager, &security_context).await?;

            let service = connect(&self.config.url, auth_header).await?;
            let mut request = CallToolRequestParams::new(remote_name.clone());
            if let Some(arguments) = params.as_object().cloned() {
                request = request.with_arguments(arguments);
            }
            let call = service.call_tool(request);

            let result = tokio::time::timeout(Duration::from_secs(self.config.timeout_secs), call).await;
            let _ = service.cancel().await;

            let result = result.map_err(|_| McpError::Timeout(format!("MCP tool call to '{}' timed out", name)))?
                .map_err(|e| McpError::ToolCallFailed(format!("MCP tool call to '{}' failed: {}", name, e)))?;

            call_tool_result_to_value(name, result)
        }.await;

        call_result.map_err(|e| ToolError::ExecutionFailed(e.to_string()))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        ComponentKind::new(MCP_TOOL_KIND).provides::<dyn ToolProvider>(),
        |descriptor, context| async move {
            let config: McpToolConfig = serde_json::from_value(descriptor.params.clone())
                .map_err(|e| ComponentError::InvalidConfig(format!("invalid parameters: {}", e)))?;

            let secret_manager = context.secret_manager.clone();
            let discover = async {
                let auth_header = resolve_auth_header(&config.auth, &secret_manager, &context.security_context).await?;
                discover_tools(&descriptor.id, &config, auth_header).await
            };
            let discovery_result: McpResult<Vec<Tool>> = match tokio::time::timeout(Duration::from_secs(config.timeout_secs), discover).await {
                Ok(result) => result,
                Err(_) => Err(McpError::Timeout(format!("Timed out discovering tools from MCP server at '{}'", config.url))),
            };

            let cached_tools = match discovery_result {
                Ok(tools) => tools,
                Err(e) => {
                    warn!(
                        "MCP tool component '{}' failed to connect/discover tools ({}); starting with no tools available",
                        descriptor.id, e
                    );
                    Vec::new()
                }
            };

            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(McpToolProvider {
                descriptor_id: descriptor.id.clone(),
                config,
                cached_tools,
                secret_manager,
            }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::security::DummySecurityManager;
    use serde_json::json;

    #[test]
    fn parses_bearer_auth_config() {
        let value = json!({
            "url": "http://127.0.0.1:1234/mcp",
            "auth": { "type": "bearer", "secret": "MY_TOKEN" },
            "timeout_secs": 5
        });
        let config: McpToolConfig = serde_json::from_value(value).unwrap();
        assert_eq!(config.url, "http://127.0.0.1:1234/mcp");
        assert_eq!(config.timeout_secs, 5);
        match config.auth {
            Some(McpAuthConfig::Bearer { secret }) => assert_eq!(secret, "MY_TOKEN"),
            _ => panic!("Expected Bearer auth"),
        }
    }

    #[test]
    fn parses_config_without_auth_using_defaults() {
        let value = json!({ "url": "http://127.0.0.1:1234/mcp" });
        let config: McpToolConfig = serde_json::from_value(value).unwrap();
        assert!(config.auth.is_none());
        assert_eq!(config.timeout_secs, 10);
        assert!(!config.requires_approval);
    }

    #[test]
    fn namespacing_round_trips_through_prefix_stripping() {
        let provider = McpToolProvider {
            descriptor_id: "github-mcp".to_string(),
            config: McpToolConfig {
                url: "http://example.invalid/mcp".to_string(),
                auth: None,
                timeout_secs: 10,
                requires_approval: false,
            },
            cached_tools: vec![],
            secret_manager: Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new()))),
        };

        let stripped = provider.strip_prefix("github-mcp__search").unwrap();
        assert_eq!(stripped, "search");
        assert!(provider.strip_prefix("unrelated_tool").is_err());
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
