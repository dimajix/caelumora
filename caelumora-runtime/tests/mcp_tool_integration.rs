use std::net::SocketAddr;
use std::sync::Arc;

use caelumora_model::command::default_commands;
use caelumora_model::component::{ComponentConfig, ComponentDescriptor, ComponentFailPolicy};
use caelumora_model::secret::SecretManager;
use caelumora_runtime::llm_impl::ollama::OllamaClient;
use caelumora_model::security::{DummySecurityManager, SecurityContext};
use caelumora_model::store::EntityStore;
use caelumora_model::store_impl::memory::MemoryStore;
use caelumora_runtime::component::{ComponentContainer, ComponentContext};
use caelumora_runtime::component_impl::mcp_tool;
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_runtime::tool::ToolProvider;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize, JsonSchema)]
struct EchoArgs {
    message: String,
}

#[derive(Clone)]
struct EchoServer {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl EchoServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Echoes the provided message back")]
    async fn echo(&self, Parameters(EchoArgs { message }): Parameters<EchoArgs>) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text(message)]))
    }
}

#[tool_handler]
impl ServerHandler for EchoServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::default()
    }
}

async fn start_test_server() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let service = StreamableHttpService::new(
        || Ok(EchoServer::new()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new().route_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (addr, handle)
}

/// A container with one MCP tool component configured with `params`.
async fn build_container(params: serde_json::Value) -> ComponentContainer {
    let store = Arc::new(MemoryStore::new());
    let secret_manager = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
    let runtime_manager = RuntimeManager::new(store.clone(), store.clone(), secret_manager, Arc::new(DummySecurityManager::new()), default_commands(), Default::default(), Arc::new(OllamaClient::default())).unwrap();
    let registry = runtime_manager.component_registry();
    let context = ComponentContext::new(SecurityContext::default(), runtime_manager.secret_manager(), None);

    let config = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            security_attributes: Default::default(),
            id: "test-echo".to_string(),
            kind: mcp_tool::MCP_TOOL_PROVIDER_KIND.to_string(),
            params,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailOpen,
        }],
    };

    ComponentContainer::from_config(&config, &registry, &context).await.unwrap()
}

#[tokio::test]
async fn discovers_and_calls_echo_tool_over_streamable_http() {
    let (addr, _server) = start_test_server().await;
    let url = format!("http://{}/mcp", addr);

    let container = build_container(json!({ "url": url })).await;
    let providers = container.get_all::<dyn ToolProvider>(None);
    assert_eq!(providers.len(), 1);
    let provider = providers[0].facet.clone();

    let tools = provider.tools();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "test-echo__echo");
    assert!(!tools[0].requires_approval, "no approval unless configured");

    let mut agent_channel_state = caelumora_model::agent::AgentChannelState::default();
    let mut turn = caelumora_runtime::agent_turn::Turn::new(
        caelumora_model::agent::AgentId::default(),
        "test-agent".to_string(),
        &mut agent_channel_state, caelumora_runtime::agent_state::AgentStateHandle::detached());

    let result = provider
        .execute("test-echo__echo", &json!({ "message": "hello mcp" }), &mut turn)
        .await
        .unwrap();
    assert_eq!(result, json!("hello mcp"));
}

#[tokio::test]
async fn a_configured_approval_reaches_every_discovered_tool() {
    let (addr, _server) = start_test_server().await;
    let url = format!("http://{}/mcp", addr);

    let container = build_container(json!({ "url": url, "requires_approval": true })).await;
    let provider = container.get_all::<dyn ToolProvider>(None)[0].facet.clone();

    let tools = provider.tools();
    assert!(!tools.is_empty(), "the server's tools are discovered");
    assert!(tools.iter().all(|t| t.requires_approval), "every tool of this server needs approval: {tools:?}");
}

#[tokio::test]
async fn shared_component_via_ref_reuses_config_but_builds_independently_per_container() {
    let (addr, _server) = start_test_server().await;
    let url = format!("http://{}/mcp", addr);

    let store = Arc::new(MemoryStore::new());
    let secret_manager = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));

    // Preconfigured components now live in the EntityStore rather than in application-level
    // config, so seed it directly.
    store
        .create_component(
            &SecurityContext::default(),
            &ComponentDescriptor {
                security_attributes: Default::default(),
                id: "shared-echo".to_string(),
                kind: mcp_tool::MCP_TOOL_PROVIDER_KIND.to_string(),
                params: json!({ "url": url }),
                tags: vec![],
                fail_policy: ComponentFailPolicy::FailOpen,
            },
        )
        .await
        .unwrap();

    let runtime_manager = RuntimeManager::new(store.clone(), store.clone(), secret_manager, Arc::new(DummySecurityManager::new()), default_commands(), Default::default(), Arc::new(OllamaClient::default())).unwrap();
    let registry = runtime_manager.component_registry();
    let context = ComponentContext::new(SecurityContext::default(), runtime_manager.secret_manager(), None);

    let ref_config = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            security_attributes: Default::default(),
            id: "github-tool".to_string(),
            kind: caelumora_runtime::component::PRECONFIGURED_REF_KIND.to_string(),
            params: json!({ "id": "shared-echo" }),
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailOpen,
        }],
    };

    // Two independent containers (simulating two personalities, each with its own
    // SecurityContext) both referencing the same shared component config by id.
    let container_a = ComponentContainer::from_config(&ref_config, &registry, &context).await.unwrap();
    let container_b = ComponentContainer::from_config(&ref_config, &registry, &context).await.unwrap();

    let provider_a = container_a.get_all::<dyn ToolProvider>(None).remove(0).facet;
    let provider_b = container_b.get_all::<dyn ToolProvider>(None).remove(0).facet;

    // Each container builds its own instance (using its own ComponentContext), rather than
    // sharing a single instance built once under a synthetic system context.
    assert!(!Arc::ptr_eq(&provider_a, &provider_b));

    for provider in [&provider_a, &provider_b] {
        let tools = provider.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "shared-echo__echo");
    }
}

#[tokio::test]
async fn unreachable_server_degrades_gracefully_instead_of_failing_construction() {
    // Port 1 is reserved and nothing will ever listen there, guaranteeing a
    // connection failure without depending on OS-specific "closed port"
    // behavior.
    let container = build_container(json!({ "url": "http://127.0.0.1:1/mcp" })).await;
    let providers = container.get_all::<dyn ToolProvider>(None);
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].facet.tools().len(), 0);
}
