//! The `mcp_tool` component against an MCP server of the test's own, over streamable HTTP.
#![expect(clippy::unwrap_used, clippy::expect_used, reason = "test code: what fails here fails the test that runs it")]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use caelumora_mcp::plugin::McpPlugin;
use caelumora_mcp::tool::MCP_TOOL_KIND;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{AgentChannelState, AgentId, Personality};
use caelumora_model::component::{ComponentBundle, ComponentConfig, ComponentDescriptor, ComponentFailPolicy};
use caelumora_model::confidentiality::{Classification, Confidentiality};
use caelumora_model::scope::Scope;
use caelumora_model::security::SecurityContext;
use caelumora_model::channel::Channel;
use caelumora_model::session::SessionId;
use caelumora_model::store::Persistence;
use caelumora_model::tool::Tool;
use caelumora_model::type_map::TypeKey;
use caelumora_model::value::name::{Name, ScopedName};
use caelumora_runtime::agent::state::AgentStateHandle;
use caelumora_agents::turn::Turn;
use caelumora_runtime::component::{ComponentContainer, ComponentContext, BUNDLE_REF_KIND};
use caelumora_runtime::runtime::{ActorRuntime, RuntimeManager};
use caelumora_runtime::session::testing::{bare_session, detached_actor_runtime_in};
use caelumora_runtime::testing::{asked_for, TestRuntime, World};
use caelumora_agents::testing::{offered_anywhere, TestAgents};
use caelumora_agents::tool::{ToolError, ToolOutput, ToolProvider, ToolResult};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use axum::Router;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use caelumora_model::work::Work;

#[derive(Debug, Deserialize, JsonSchema)]
struct EchoArgs {
    message: String,
}

/// A tool server with two tools - or, once it was replaced, with `echo`
/// alone; `#[tool_handler]` routes to them through its `tool_router`.
#[derive(Clone)]
struct EchoServer {
    tool_router: ToolRouter<Self>,
}

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

impl EchoServer {
    /// The server with both its tools, or with `echo` alone once `replaced`.
    fn new(replaced: bool) -> Self {
        let mut tool_router = Self::tool_router();
        if replaced {
            tool_router.remove_route("forget");
        }
        Self { tool_router }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for EchoServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::default().with_instructions("  Echo messages back, for testing.  ")
    }
}

async fn start_test_server() -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (addr, serve(listener, Arc::new(AtomicBool::new(false))))
}

/// Serves the test server on `listener`, replaced - with `echo` alone - for
/// every connection made once `replaced` is set.
fn serve(listener: TcpListener, replaced: Arc<AtomicBool>) -> JoinHandle<()> {
    let service = StreamableHttpService::new(
        move || Ok(EchoServer::new(replaced.load(Ordering::SeqCst))),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let router = Router::new().route_service("/mcp", service);
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    })
}

/// The names of `tools`, sorted.
fn names(tools: &[Tool]) -> Vec<String> {
    let mut names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
    names.sort();
    names
}

/// A runtime over `store` with the plugin of MCP servers, as the programs
/// build theirs.
fn runtime(store: &Persistence) -> Arc<RuntimeManager> {
    TestRuntime { store: store.clone(), plugins: vec![&McpPlugin], ..Default::default() }.build()
}

/// The tool provider of the one MCP tool component configured with
/// `params`.
async fn provider(params: Value) -> Arc<dyn ToolProvider> {
    let container = build_container(params).await;
    Arc::clone(&container.get_all::<dyn ToolProvider>(None).unwrap()[0].facet)
}

/// What `provider` gives for the call of its tool `name` with `params`, in
/// a turn that reaches no channel.
async fn executed(provider: &dyn ToolProvider, name: &str, params: Value) -> ToolResult<ToolOutput> {
    let mut state = AgentChannelState::default();
    state.begin_turn(Work::new(Default::default(), None, Default::default(), Default::default(), Default::default()));
    let agent = AgentId::default();
    let mut turn = Turn::of(agent, "test-agent".to_string(), &mut state, AgentStateHandle::detached(), detached_actor_runtime()).expect("a turn was begun");
    provider.execute(name, &params, None, &mut turn).await
}

/// A container with one MCP tool component configured with `params`.
async fn build_container(params: Value) -> ComponentContainer {
    let runtime_manager = runtime(&Persistence::in_memory());
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

/// What a turn outside of a running session acts through: the runtime of
/// an actor in a channel that is attached to no session, so it reaches none.
fn detached_actor_runtime() -> ActorRuntime {
    let session = bare_session(SessionId::new());
    let channel = Channel::build(session.id, ActorId::None, Name::new("detached").unwrap(), Default::default(), Classification::CHANNEL_DEFAULT, None);
    detached_actor_runtime_in(runtime(&Persistence::in_memory()), session, channel)
}

#[tokio::test]
async fn discovers_and_calls_echo_tool_over_streamable_http() {
    let (addr, _server) = start_test_server().await;
    let url = format!("http://{}/mcp", addr);

    let container = build_container(json!({ "endpoint": { "url": url } })).await;
    let providers = container.get_all::<dyn ToolProvider>(None).unwrap();
    assert_eq!(providers.len(), 1);
    let provider = Arc::clone(&providers[0].facet);
    assert!(provider.tools().is_empty(), "not discovered while built (issue 126)");
    assert_eq!(provider.description(), "The tools of the MCP server 'test-echo'.");

    assert_eq!(offered_anywhere(provider.as_ref()).await.map(|tools| tools.len()), Ok(2), "all of them, wherever");
    let tools = provider.tools();
    assert_eq!(names(&tools), ["test-echo__echo", "test-echo__forget"], "as discovered when offered");
    assert!(tools.iter().all(|t| t.requires_approval), "approval unless configured otherwise, read-only or not");
    assert_eq!(provider.description(), "Echo messages back, for testing.", "what the server says of itself");

    let result = executed(provider.as_ref(), "test-echo__echo", json!({ "message": "hello mcp" })).await.unwrap();
    assert_eq!(result.value, json!("hello mcp"));
    assert!(result.classification.is_none(), "an MCP server does not classify its results");
}

#[tokio::test]
async fn a_configured_approval_and_classification_reach_every_discovered_tool() {
    let (addr, _server) = start_test_server().await;
    let url = format!("http://{}/mcp", addr);

    let provider = provider(json!({
        "endpoint": { "url": url },
        "requires_approval": false,
        "classification": { "level": "confidential" },
    })).await;

    let tools = offered_anywhere(provider.as_ref()).await.unwrap();
    assert!(!tools.is_empty(), "the server's tools are discovered");
    assert!(tools.iter().all(|t| !t.requires_approval), "no tool of this server needs approval: {tools:?}");
    let confidential = Classification::general(Confidentiality::Confidential);
    assert!(tools.iter().all(|t| t.classification.as_ref() == Some(&confidential)), "and so does the classification: {tools:?}");
}

#[tokio::test]
async fn unless_read_only_the_tools_the_server_says_only_read_need_no_approval() {
    let (addr, _server) = start_test_server().await;
    let url = format!("http://{}/mcp", addr);

    let provider = provider(json!({ "endpoint": { "url": url }, "requires_approval": "unless_read_only" })).await;

    let tools = offered_anywhere(provider.as_ref()).await.unwrap();
    let asks = |name: &str| tools.iter().find(|t| t.name == name).map(|t| t.requires_approval);
    assert_eq!(asks("test-echo__echo"), Some(false), "read-only, as the server says");
    assert_eq!(asks("test-echo__forget"), Some(true), "it changes something");
}

#[tokio::test]
async fn shared_component_via_ref_reuses_config_but_builds_independently_per_container() {
    let (addr, _server) = start_test_server().await;
    let url = format!("http://{}/mcp", addr);

    let store = Persistence::in_memory();

    // A bundle is an entity like any other, so it is put into the store directly.
    store
        .entity_stores()
        .unwrap()
        .component_bundles
        .create(
            &SecurityContext::default(),
            &ComponentBundle::new(ScopedName::new("shared-echo", Scope::GlobalScope).unwrap(), ComponentConfig {
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
            kind: BUNDLE_REF_KIND.to_string(),
            params: json!({ "name": "shared-echo" }),
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailOpen,
        }],
    };

    // Two independent containers (simulating two personalities, each with its own
    // SecurityContext) both referencing the same bundle by name.
    let container_a = ComponentContainer::from_config(&ref_config, &[TypeKey::of::<dyn ToolProvider>()], registry, &context).await.unwrap();
    let container_b = ComponentContainer::from_config(&ref_config, &[TypeKey::of::<dyn ToolProvider>()], registry, &context).await.unwrap();

    let provider_a = container_a.get_all::<dyn ToolProvider>(None).unwrap().remove(0).facet;
    let provider_b = container_b.get_all::<dyn ToolProvider>(None).unwrap().remove(0).facet;

    // Each container builds its own instance (using its own ComponentContext), rather than
    // sharing a single instance built once under a synthetic system context.
    assert!(!Arc::ptr_eq(&provider_a, &provider_b));

    for provider in [&provider_a, &provider_b] {
        let tools = offered_anywhere(provider.as_ref()).await.unwrap();
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
    assert_eq!(offered_anywhere(providers[0].facet.as_ref()).await, Err("its server cannot be reached".to_string()), "its tool set says so, without the error (issue 32)");
    assert_eq!(providers[0].tools().len(), 0);
    assert_eq!(providers[0].description(), "The tools of the MCP server 'test-echo'.");
}

#[tokio::test]
async fn a_configured_description_replaces_what_the_server_says_of_itself() {
    let (addr, _server) = start_test_server().await;
    let url = format!("http://{}/mcp", addr);

    let provider = provider(json!({ "endpoint": { "url": url }, "description": "Messages echoed by the test server." })).await;
    offered_anywhere(provider.as_ref()).await.unwrap();
    assert_eq!(provider.description(), "Messages echoed by the test server.");
}

/// Issue 126: the tools follow a server that was replaced once what was
/// discovered is older than the endpoint's `refresh_secs` - by default
/// five minutes.
#[tokio::test]
async fn the_tools_follow_a_replaced_server_once_those_discovered_expired() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let replaced = Arc::new(AtomicBool::new(false));
    let _server = serve(listener, Arc::clone(&replaced));
    let kept = provider(json!({ "endpoint": { "url": url } })).await;
    let refreshed = provider(json!({ "endpoint": { "url": url, "refresh_secs": 0 } })).await;
    for provider in [&kept, &refreshed] {
        assert_eq!(offered_anywhere(provider.as_ref()).await.map(|tools| names(&tools)), Ok(vec!["test-echo__echo".to_string(), "test-echo__forget".to_string()]));
    }

    replaced.store(true, Ordering::SeqCst);
    assert_eq!(offered_anywhere(refreshed.as_ref()).await.map(|tools| names(&tools)), Ok(vec!["test-echo__echo".to_string()]), "discovered again");
    assert_eq!(names(&refreshed.tools()), ["test-echo__echo"], "and so found by a call");
    assert_eq!(offered_anywhere(kept.as_ref()).await.map(|tools| tools.len()), Ok(2), "kept for five minutes");
}

/// Issue 126: a call of a tool the server no longer knows fails, and has
/// the tools discovered again at the next offer.
#[tokio::test]
async fn a_tool_the_server_no_longer_knows_has_the_tools_discovered_again() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let replaced = Arc::new(AtomicBool::new(false));
    let _server = serve(listener, Arc::clone(&replaced));
    let provider = provider(json!({ "endpoint": { "url": url } })).await;
    assert_eq!(offered_anywhere(provider.as_ref()).await.map(|tools| tools.len()), Ok(2));

    replaced.store(true, Ordering::SeqCst);
    let failed = executed(provider.as_ref(), "test-echo__forget", json!({})).await;
    assert!(matches!(&failed, Err(ToolError::ExecutionFailed(why)) if why.contains("tool not found")), "{failed:?}");
    assert_eq!(offered_anywhere(provider.as_ref()).await.map(|tools| names(&tools)), Ok(vec!["test-echo__echo".to_string()]));
}

/// Issue 126: a server that cannot be reached when its tools are first
/// offered is asked again the next time, and its tools are offered once it
/// answers.
#[tokio::test]
async fn a_server_unreachable_at_first_is_followed_once_it_answers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let provider = provider(json!({ "endpoint": { "url": format!("http://{address}/mcp") } })).await;
    assert_eq!(offered_anywhere(provider.as_ref()).await, Err("its server cannot be reached".to_string()));

    let _server = serve(TcpListener::bind(address).await.unwrap(), Arc::new(AtomicBool::new(false)));
    assert_eq!(offered_anywhere(provider.as_ref()).await.map(|tools| tools.len()), Ok(2), "asked again");
    assert_eq!(provider.description(), "Echo messages back, for testing.");
}

/// Issue 126: a server that cannot be asked again leaves the tools
/// discovered before.
#[tokio::test]
async fn a_server_gone_leaves_the_tools_discovered_before() {
    let (addr, server) = start_test_server().await;
    let provider = provider(json!({ "endpoint": { "url": format!("http://{addr}/mcp"), "refresh_secs": 0 } })).await;
    assert_eq!(offered_anywhere(provider.as_ref()).await.map(|tools| tools.len()), Ok(2));

    server.abort();
    let _ = server.await;
    assert_eq!(offered_anywhere(provider.as_ref()).await.map(|tools| tools.len()), Ok(2), "kept");
}

/// Issue 126: the tools are discovered in the name of whoever acts, as a
/// call is made (`ComponentContext` keeps no security context for later):
/// nothing is read while the agent is built, and its credential is read
/// for alice's agent, asked by bob, as its turn begins.
#[tokio::test]
async fn the_tools_are_discovered_in_the_name_of_whoever_acts() {
    let world = World::with(&[&TestAgents, &McpPlugin], |_| {}).await;
    let mcp = ComponentDescriptor {
        params: json!({ "endpoint": { "url": "http://127.0.0.1:1/mcp", "auth": { "type": "bearer", "secret": "MCP_TOKEN" } } }),
        ..ComponentDescriptor::new("tools", MCP_TOOL_KIND)
    };
    let personality = world.personality("working", None, &[]).await;
    let working = Personality { components: ComponentConfig { component_nodes: vec![mcp] }, ..personality };
    world.runtime.entity_stores().personalities.update(&World::system(), &working).await.unwrap();
    let helper = world.agent("helper", world.alice, &working).await;

    let (session, asked) = world.asked(world.shared_session(&[helper], &[])).await;
    assert!(!asked.iter().any(|line| line.contains("ReadSecret")), "nothing read while built: {asked:?}");

    let main = Arc::clone(&session.channels()[0]);
    let (_, asked) = world.asked(async {
        main.user_actor(world.bob).expect("he takes part").send_message("@helper do it".to_string(), None).await.expect("sent");
        asked_for(&world, "alice as helper, asked by bob: ReadSecret yes").await;
    }).await;
    let read_secret: Vec<&String> = asked.iter().filter(|line| line.contains("ReadSecret")).collect();
    assert_eq!(read_secret, ["alice as helper, asked by bob: ReadSecret yes"], "with alice's credential, asked by bob");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}
