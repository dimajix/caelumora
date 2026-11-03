//! The component kind `mcp_server` (issues 78, 126, 180, 534): what it is configured with - one
//! remote MCP server, how its tools are approved and classified, what it is cleared for -; the
//! tools of that server are its facet `ToolProvider` (`tool`).

mod tool;

use caelumora_agents::tool::{ApprovalMode, ToolProvider};
use caelumora_api::cache::ExpiringCache;
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents, Params};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::remote::RemoteEndpoint;
use caelumora_domain::tool::Tool;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

/// The kind of the component.
pub const MCP_SERVER_KIND: &str = "mcp_server";

/// How long a call or the discovery of the tools may take unless the
/// endpoint says otherwise.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the tools discovered are kept before the server is asked for
/// them again, unless the endpoint says otherwise (issue 126).
const DEFAULT_REFRESH: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpServerParams {
    /// The MCP server. Its timeout is how long a call of one of its tools,
    /// or the discovery of them, may take; its `refresh_secs` how long the
    /// tools discovered are kept.
    endpoint: RemoteEndpoint,
    /// Whether calls to this MCP server's tools require human approval.
    /// Left out, they do: a server may act outside, and what makes an agent
    /// call it may be a consent nobody gave (issue 150).
    #[serde(default)]
    requires_approval: ApprovalMode,
    /// How the results of this server's tools are classified (issue 133):
    /// a result that may not flow into the agent's channel is withheld
    /// from the agent. Left out, as the channel.
    #[serde(default)]
    classification: Option<Classification>,
    /// What this server's tools are for, written for a model: the
    /// description of their tool set (issue 194). Left out, what the server
    /// says of itself in its `instructions`, else a text naming the
    /// component.
    #[serde(default)]
    description: Option<String>,
    /// What the server is cleared for (issue 180): where the arguments of
    /// its tools go. Its tools are not offered in a channel that may not
    /// flow there. Left out, public: the server's word is trusted only
    /// where it is configured.
    #[serde(default)]
    destination: Option<Classification>,
}

impl Params for McpServerParams {}

impl McpServerParams {
    fn timeout(&self) -> Duration {
        self.endpoint.timeout_or(DEFAULT_TIMEOUT)
    }

    fn refresh(&self) -> Duration {
        self.endpoint.refresh_or(DEFAULT_REFRESH)
    }
}

/// A component of the kind `mcp_server`: the tools of one remote MCP server, reached over the
/// Streamable HTTP transport, as one tool set.
#[derive(Debug)]
struct McpServerComponent {
    descriptor_id: String,
    params: McpServerParams,
    /// The server's tools, and what it says of itself (`instructions`) if
    /// it does, as last discovered.
    discovered: ExpiringCache<(Vec<Tool>, Option<String>)>,
}

impl McpServerComponent {
    /// The component `descriptor_id` configured by `params`; its tools are not discovered yet.
    fn new(descriptor_id: String, params: McpServerParams) -> Self {
        let discovered = ExpiringCache::new(params.refresh());
        Self { descriptor_id, params, discovered }
    }
}

/// Registers the component kind `mcp_server`, built of its params alone.
pub(crate) fn register(registry: &mut LibraryComponents<'_>) {
    registry.register(
        ComponentKind::new(MCP_SERVER_KIND)
            .params::<McpServerParams>()
            .provides::<dyn ToolProvider>(),
        |descriptor, _context| {
            let params = McpServerParams::read(&descriptor.params)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(McpServerComponent::new(
                descriptor.id.clone(),
                params,
            )));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::remote::RemoteAuthConfig;
    use serde_json::{json, Value};

    #[test]
    fn parses_bearer_auth_config() {
        let value = json!({ "endpoint": {
            "url": "http://127.0.0.1:1234/mcp",
            "auth": { "type": "bearer", "secret": "MY_TOKEN" },
            "timeout_secs": 5
        } });
        let params: McpServerParams = serde_json::from_value(value).unwrap();
        assert_eq!(params.endpoint.url, "http://127.0.0.1:1234/mcp");
        assert_eq!(params.timeout(), Duration::from_secs(5));
        match params.endpoint.auth {
            Some(RemoteAuthConfig::Bearer { secret }) => assert_eq!(secret, "MY_TOKEN"),
            _ => panic!("Expected Bearer auth"),
        }
    }

    #[test]
    fn parses_config_without_auth_using_defaults() {
        let value = json!({ "endpoint": { "url": "http://127.0.0.1:1234/mcp" } });
        let params: McpServerParams = serde_json::from_value(value).unwrap();
        assert!(params.endpoint.auth.is_none());
        assert_eq!(params.timeout(), Duration::from_secs(10));
        assert_eq!(params.refresh(), Duration::from_secs(300));
        assert_eq!(
            params.requires_approval,
            ApprovalMode::Always,
            "approval unless configured otherwise"
        );
        assert!(
            serde_json::from_value::<McpServerParams>(
                json!({ "endpoint": { "url": "http://mcp.example" }, "requires_aproval": true })
            )
            .is_err(),
            "nothing unknown"
        );
    }

    #[test]
    fn approval_is_read_as_one_of_its_modes_and_nothing_else() {
        let configured = |approval: Value| {
            serde_json::from_value::<McpServerParams>(
                json!({ "endpoint": { "url": "http://mcp.example" }, "requires_approval": approval }),
            )
            .map(|c| c.requires_approval)
        };
        assert_eq!(configured(json!("unless_read_only")).unwrap(), ApprovalMode::UnlessReadOnly);
        let refused = configured(json!("hints")).unwrap_err().to_string();
        assert!(refused.contains("requires_approval must be"), "names the field: {refused}");
    }

    /// Issue 180: what the server is cleared for is read from the params.
    #[test]
    fn a_destination_is_read_from_the_params() {
        let configured: McpServerParams = serde_json::from_value(
            json!({ "endpoint": { "url": "http://mcp.example" }, "destination": { "level": "confidential" } }),
        )
        .unwrap();
        assert!(configured.destination.is_some(), "read from the params");
    }
}
