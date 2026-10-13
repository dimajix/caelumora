use std::net::SocketAddr;
use std::sync::Arc;

use caelumora_mcp::plugin::McpPlugin;
use caelumora_mcp::tool::MCP_TOOL_KIND;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::component::{ComponentConfig, ComponentDescriptor, ComponentFailPolicy, StoredComponents};
use caelumora_model::confidentiality::{Classification, Confidentiality, ConfidentialityRules};
use caelumora_model::config::AppConfig;
use caelumora_model::secret::SecretManager;
use caelumora_model::security::{DummySecurityManager, SecurityContext};
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::store::memory::MemoryStore;
use caelumora_model::store::ComponentStore;
use caelumora_model::type_map::TypeKey;
use caelumora_model::value::name::Name;
use caelumora_runtime::agent::state::AgentStates;
use caelumora_runtime::agent::turn::Turn;
use caelumora_runtime::backend::Backends;
use caelumora_runtime::command::CommandExecutor;
use caelumora_runtime::component::{ComponentContainer, ComponentContext, ComponentRegistry, PRECONFIGURED_REF_KIND};
use caelumora_runtime::runtime::{ChannelRuntime, RuntimeBuilder, RuntimeManager};
use caelumora_runtime::tool::ToolProvider;
use caelumora_runtime::view::ViewsFactory;
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

/// A tool server with two tools; `#[tool_handler]` routes to them through
/// `Self::tool_router()`.
#[derive(Clone)]
struct EchoServer;

#[tool_router]
impl EchoServer {
    #[tool(description = "Echoes the provided message back", annotations(read_only_hint = true))]
    async fn echo(&self, Parameters(EchoArgs { message }): Parameters<EchoArgs>) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text(message)]))
    }

    #[tool(description = "Forgets every message echoed so far", annotations(destructive_hint = true))]
    async fn forget(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("forgotten")]))
    }
}

#[tool_handler]
impl ServerHandler for EchoServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::default().with_instructions("  Echo messages back, for testing.  ")
    }
}

async fn start_test_server() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let service = StreamableHttpService::new(
        || Ok(EchoServer),
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

/// A runtime over `store` with the plugin of MCP servers, as the programs
/// build theirs.
fn runtime(store: &Arc<MemoryStore>) -> RuntimeManager {
    let security = Arc::new(DummySecurityManager::new());
    let secrets = Arc::new(SecretManager::new(vec![], Arc::<DummySecurityManager>::clone(&security)));
    let backends = Backends::with_plugins(&[&McpPlugin]);
    let mut builder = RuntimeBuilder::new(Arc::<MemoryStore>::clone(store), Arc::<MemoryStore>::clone(store), secrets, security, Arc::new(ConfidentialityRules::default()), &AppConfig::default(), ComponentRegistry::built_in(Arc::<MemoryStore>::clone(store)), backends).unwrap();
    builder.plugin(&McpPlugin).unwrap();
    builder.build().unwrap()
}

/// A container with one MCP tool component configured with `params`.
async fn build_container(params: serde_json::Value) -> ComponentContainer {
    let runtime_manager = runtime(&Arc::new(MemoryStore::new()));
    let registry = runtime_manager.component_registry();
    let context = ComponentContext::new(SecurityContext::default(), runtime_manager.services().clone());

    let config = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "test-echo".to_string(),
            kind: MCP_TOOL_KIND.to_string(),
            params,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailOpen,
        }],
    };

    ComponentContainer::from_config(&config, &[TypeKey::of::<dyn ToolProvider>()], registry, &context).await.unwrap()
}

/// What a turn outside of a running session navigates: the views of an
/// actor in a channel that is attached to no session, so they reach none.
fn detached_views() -> ViewsFactory {
    let runtime_manager = Arc::new(runtime(&Arc::new(MemoryStore::new())));
    let now = chrono::Utc::now();
    let session = Session {
        summary: None, briefing: None,
        approval_rules: Vec::new(),
        workspace: None,
        id: SessionId::new(),
        name: Name::new("detached").unwrap(),
        commands: Default::default(),
        privacy: Default::default(),
        participants: Vec::new(),
        channels: Vec::new(),
        created_at: now,
        updated_at: now,
        security_attributes: Default::default(),
        deleted: None,
    };
    let session_id = session.id;
    let commands = Arc::new(CommandExecutor::empty(Arc::clone(runtime_manager.security_manager())));
    let session = runtime_manager.controller_manager().new_session_controller(session);
    let channel = ChannelRuntime::new(runtime_manager, commands, session);
    channel.new_actor_runtime(ActorId::None, session_id, ChannelId::new()).views()
}

#[tokio::test]
async fn discovers_and_calls_echo_tool_over_streamable_http() {
    let (addr, _server) = start_test_server().await;
    let url = format!("http://{}/mcp", addr);

    let container = build_container(json!({ "endpoint": { "url": url } })).await;
    let providers = container.get_all::<dyn ToolProvider>(None).unwrap();
    assert_eq!(providers.len(), 1);
    let provider = Arc::clone(&providers[0].facet);

    let tools = provider.tools();
    let mut names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["test-echo__echo", "test-echo__forget"]);
    assert!(tools.iter().all(|t| t.requires_approval), "approval unless configured otherwise, read-only or not");
    assert_eq!(provider.description(), "Echo messages back, for testing.", "what the server says of itself");
    assert!(provider.unavailable().is_none());

    let mut agent_channel_state = AgentChannelState::default();
    agent_channel_state.begin_turn(Default::default(), Default::default(), Default::default(), Vec::new(), Default::default());
    let states = AgentStates::new(Arc::new(MemoryStore::new()), Arc::new(()));
    let agent_id = AgentId::default();
    let agent_state = states.get(agent_id, Default::default()).await.expect("the state of the agent was loaded");
    let mut turn = Turn::of(agent_id, "test-agent".to_string(), &mut agent_channel_state, agent_state, detached_views()).expect("a turn was begun");

    let result = provider
        .execute("test-echo__echo", &json!({ "message": "hello mcp" }), None, &mut turn)
        .await
        .unwrap();
    assert_eq!(result.value, json!("hello mcp"));
    assert!(result.classification.is_none(), "an MCP server does not classify its results");
}

#[tokio::test]
async fn a_configured_approval_and_classification_reach_every_discovered_tool() {
    let (addr, _server) = start_test_server().await;
    let url = format!("http://{}/mcp", addr);

    let container = build_container(json!({
        "endpoint": { "url": url },
        "requires_approval": false,
        "classification": { "level": "confidential" },
    })).await;
    let provider = Arc::clone(&container.get_all::<dyn ToolProvider>(None).unwrap()[0].facet);

    let tools = provider.tools();
    assert!(!tools.is_empty(), "the server's tools are discovered");
    assert!(tools.iter().all(|t| !t.requires_approval), "no tool of this server needs approval: {tools:?}");
    let confidential = Classification::general(Confidentiality::Confidential);
    assert!(tools.iter().all(|t| t.classification.as_ref() == Some(&confidential)), "and so does the classification: {tools:?}");
}

#[tokio::test]
async fn unless_read_only_the_tools_the_server_says_only_read_need_no_approval() {
    let (addr, _server) = start_test_server().await;
    let url = format!("http://{}/mcp", addr);

    let container = build_container(json!({ "endpoint": { "url": url }, "requires_approval": "unless_read_only" })).await;
    let provider = Arc::clone(&container.get_all::<dyn ToolProvider>(None).unwrap()[0].facet);

    let tools = provider.tools();
    let asks = |name: &str| tools.iter().find(|t| t.name == name).map(|t| t.requires_approval);
    assert_eq!(asks("test-echo__echo"), Some(false), "read-only, as the server says");
    assert_eq!(asks("test-echo__forget"), Some(true), "it changes something");
}

#[tokio::test]
async fn shared_component_via_ref_reuses_config_but_builds_independently_per_container() {
    let (addr, _server) = start_test_server().await;
    let url = format!("http://{}/mcp", addr);

    let store = Arc::new(MemoryStore::new());

    // Preconfigured components live in the ComponentStore rather than in application-level
    // config, so seed it directly.
    store
        .create_components(
            &SecurityContext::default(),
            &StoredComponents::new("shared-echo", ComponentConfig {
                component_nodes: vec![ComponentDescriptor {
                    id: "shared-echo".to_string(),
                    kind: MCP_TOOL_KIND.to_string(),
                    params: json!({ "endpoint": { "url": url } }),
                    tags: vec![],
                    fail_policy: ComponentFailPolicy::FailOpen,
                }],
            }, Default::default()),
        )
        .await
        .unwrap();

    let runtime_manager = runtime(&store);
    let registry = runtime_manager.component_registry();
    let context = ComponentContext::new(SecurityContext::default(), runtime_manager.services().clone());

    let ref_config = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "github-tool".to_string(),
            kind: PRECONFIGURED_REF_KIND.to_string(),
            params: json!({ "id": "shared-echo" }),
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailOpen,
        }],
    };

    // Two independent containers (simulating two personalities, each with its own
    // SecurityContext) both referencing the same shared component config by id.
    let container_a = ComponentContainer::from_config(&ref_config, &[TypeKey::of::<dyn ToolProvider>()], registry, &context).await.unwrap();
    let container_b = ComponentContainer::from_config(&ref_config, &[TypeKey::of::<dyn ToolProvider>()], registry, &context).await.unwrap();

    let provider_a = container_a.get_all::<dyn ToolProvider>(None).unwrap().remove(0).facet;
    let provider_b = container_b.get_all::<dyn ToolProvider>(None).unwrap().remove(0).facet;

    // Each container builds its own instance (using its own ComponentContext), rather than
    // sharing a single instance built once under a synthetic system context.
    assert!(!Arc::ptr_eq(&provider_a, &provider_b));

    for provider in [&provider_a, &provider_b] {
        let tools = provider.tools();
        assert_eq!(tools.len(), 2);
        assert!(tools.iter().all(|t| t.name.starts_with("shared-echo__")), "{tools:?}");
    }
}

#[tokio::test]
async fn unreachable_server_degrades_gracefully_instead_of_failing_construction() {
    // Port 1 is reserved and nothing will ever listen there, guaranteeing a
    // connection failure without depending on OS-specific "closed port"
    // behavior.
    let container = build_container(json!({ "endpoint": { "url": "http://127.0.0.1:1/mcp" } })).await;
    let providers = container.get_all::<dyn ToolProvider>(None).unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].tools().len(), 0);
    assert_eq!(providers[0].unavailable().as_deref(), Some("its server cannot be reached"), "its tool set says so, without the error (issue 32)");
    assert_eq!(providers[0].description(), "The tools of the MCP server 'test-echo'.");
}

#[tokio::test]
async fn a_configured_description_replaces_what_the_server_says_of_itself() {
    let (addr, _server) = start_test_server().await;
    let url = format!("http://{}/mcp", addr);

    let container = build_container(json!({ "endpoint": { "url": url }, "description": "Messages echoed by the test server." })).await;
    let provider = Arc::clone(&container.get_all::<dyn ToolProvider>(None).unwrap()[0].facet);
    assert_eq!(provider.description(), "Messages echoed by the test server.");
}
