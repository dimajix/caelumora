//! Caelumora as one program (issue 323): the composition root that lists
//! the plugins, builds the runtime with them, serves the REST API with
//! their routes under `/api` and - with the feature `web-ui` - the web UI at
//! `/`, from the binary.

use axum::Router;
use caelumora_logging::{LogTarget, init};
use caelumora_a2a::plugin::A2aPlugin;
use caelumora_mcp::plugin::McpPlugin;
use caelumora_model::config::AppConfig;
use caelumora_ollama::client::OLLAMA_CLIENT_KIND;
use caelumora_ollama::plugin::OllamaPlugin;
use caelumora_decision::plugin::DecisionPlugin;
use caelumora_open_terminal::plugin::OpenTerminalPlugin;
use caelumora_store_json::plugin::JsonStorePlugin;
use caelumora_workspace_tools::plugin::WorkspaceToolsPlugin;
use caelumora_rest_server::auth::{Auth, TokenService};
use caelumora_rest_server::error::{self, ApiError};
use caelumora_rest_server::plugin::RestPlugin;
use caelumora_rest_server::routes;
use caelumora_rest_server::state::AppState;
use caelumora_runtime::component::{ComponentError, ComponentRegistry};
use caelumora_agents::prompt::ModelProfileProvider;
use caelumora_agents::llm::LLMClient;
use caelumora_agents::plugin::AgentsPlugin;
use caelumora_runtime::plugin::Plugin;
use caelumora_runtime::runtime::RuntimeBuilder;
use caelumora_runtime::session::SessionManager;
use caelumora_artifacts::plugin::ArtifactsPlugin;
use caelumora_skills::plugin::SkillsPlugin;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use anyhow::Result;
use axum::http::Request;
use std::future::IntoFuture;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::signal::unix::{signal, SignalKind};
use tokio_util::sync::CancellationToken;
use tracing::field::Empty;

#[cfg(feature = "web-ui")]
mod web_ui;

/// Where the REST API is served: the web UI speaks to it there.
const API_PATH: &str = "/api";

#[derive(Parser, Debug)]
#[command(author, version, about = "Caelumora")]
struct Args {
    /// Path to a specific config file
    #[arg(short, long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let config = if let Some(config_path) = args.config {
        AppConfig::load(config_path)?
    }
    else {
        AppConfig::default()
    };

    init(&config.logging, LogTarget::Stderr, "info,tower_http=debug,axum::rejection=trace")?;

    let rest_plugins = rest_plugins();
    let plugins = plugins();
    let mut builder = RuntimeBuilder::from_config(&config, &all(&rest_plugins, &plugins)).await?;
    default_kinds(builder.components())?;
    let runtime_manager = Arc::new(builder.build()?);
    let session_manager = Arc::new(SessionManager::new(Arc::clone(&runtime_manager)));
    let user_manager = Arc::clone(runtime_manager.user_manager());
    if user_manager.is_empty() {
        tracing::warn!("No user providers configured (`users` section): nobody can log in. \
            See examples/config.yaml.");
    }
    let auth = Arc::new(Auth {
        tokens: TokenService::from_secret(runtime_manager.secret_manager(), &config.auth).await?,
        users: user_manager,
        controllers: Arc::clone(runtime_manager.controller_manager()),
    });

    runtime_manager.seed(&config.seed).await?;
    // What was under way when the process last ended goes on (issue 388),
    // beside the requests rather than before them.
    tokio::spawn({
        let session_manager = Arc::clone(&session_manager);
        async move { session_manager.resume_interrupted().await }
    });

    let shutdown = CancellationToken::new();
    let state = Arc::new(AppState {
        auth,
        runtime_manager,
        session_manager: Arc::clone(&session_manager),
        compartments: Arc::new(config.compartments.clone()),
        shutdown: shutdown.clone(),
    });

    // One span per request. `request_id` is filled in by `error::request_id`,
    // `user_id` by `auth::require_auth`,
    // `span_id` by handlers that start an envelope chain in a channel.
    let trace_layer = TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
        tracing::info_span!(
            "request",
            method = %request.method(),
            uri = %request.uri(),
            request_id = Empty,
            user_id = Empty,
            span_id = Empty,
        )
    });
    // The request id is set inside the request span, which is the outer layer.
    let app = app(routes::api(state, &rest_plugins))
        .layer(axum::middleware::from_fn(error::request_id))
        .layer(trace_layer);

    let listener = TcpListener::bind(config.server.address).await?;
    tracing::info!("Caelumora listening on {}", listener.local_addr()?);
    let serving = axum::serve(listener, app).with_graceful_shutdown(shutdown_on_signal(session_manager, shutdown.clone()));
    tokio::select! {
        served = serving.into_future() => served?,
        () = async { shutdown.cancelled().await; tokio::time::sleep(CONNECTIONS_CLOSE).await } => {
            tracing::warn!("Requests were still under way {:?} after the shutdown; they end with the process", CONNECTIONS_CLOSE);
        }
    }
    tracing::info!("Caelumora has ended");
    Ok(())
}

/// How long the requests under way are waited for once the sessions are
/// shut down and the streams ended (issue 297).
const CONNECTIONS_CLOSE: Duration = Duration::from_secs(1);

/// Waits for SIGINT or SIGTERM, then shuts down in order (issue 297): the
/// sessions are stopped, their actors keeping what they are in the middle
/// of, and the `local` shell's commands ended, within `SHUTDOWN_BOUND` -
/// meanwhile requests are still answered, a start of a session refused;
/// then `shutdown` ends every stream, and the server takes no new request.
async fn shutdown_on_signal(session_manager: Arc<SessionManager>, shutdown: CancellationToken) {
    signalled().await;
    tracing::info!("Caelumora is shutting down");
    session_manager.shutdown().await;
    shutdown.cancel();
}

/// Completes on SIGINT or SIGTERM - or never, should neither be caught.
async fn signalled() {
    let terminated = async {
        match signal(SignalKind::terminate()) {
            Ok(mut terminated) => {
                terminated.recv().await;
            }
            Err(e) => {
                tracing::warn!("SIGTERM cannot be caught, and ends Caelumora without a shutdown: {e}");
                std::future::pending::<()>().await;
            }
        }
    };
    let interrupted = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!("SIGINT cannot be caught, and ends Caelumora without a shutdown: {e}");
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        () = terminated => {}
        () = interrupted => {}
    }
}

/// The plugins of the program that serve routes (issue 323), which it
/// builds its runtime and its API with.
fn rest_plugins() -> Vec<Box<dyn RestPlugin>> {
    vec![Box::new(SkillsPlugin), Box::new(ArtifactsPlugin)]
}

/// The plugins of the program that bring no routes (issues 325, 327, 329,
/// 428), which only its runtime is built with - first the essential one,
/// the agents' (issue 370).
fn plugins() -> Vec<Box<dyn Plugin>> {
    vec![Box::new(AgentsPlugin), Box::new(OllamaPlugin), Box::new(McpPlugin), Box::new(A2aPlugin), Box::new(OpenTerminalPlugin), Box::new(WorkspaceToolsPlugin), Box::new(DecisionPlugin), Box::new(JsonStorePlugin)]
}

/// Names the defaults of the facets of which an agent has one that the core
/// leaves to the program (issue 329) among the kinds of `components`: the
/// model is served by Ollama. `component_defaults` of the configuration
/// still replaces them.
///
/// # Errors
/// If a plugin that brings a default kind is not among `plugins`.
fn default_kinds(components: &mut ComponentRegistry) -> Result<(), ComponentError> {
    components.set_default_kind::<dyn LLMClient>(OLLAMA_CLIENT_KIND)?;
    components.set_default_kind::<dyn ModelProfileProvider>(OLLAMA_CLIENT_KIND)
}

/// Every plugin of the program, which it builds its runtime with:
/// `rest_plugins` and `plugins`, those without routes.
fn all<'a>(rest_plugins: &'a [Box<dyn RestPlugin>], plugins: &'a [Box<dyn Plugin>]) -> Vec<&'a dyn Plugin> {
    rest_plugins.iter().map(|plugin| plugin.as_ref() as &dyn Plugin).chain(plugins.iter().map(Box::as_ref)).collect()
}

/// What the program serves: `api` under `/api`, where a path no route
/// takes is answered as a problem like every other error of the API, and
/// the web UI everywhere else if it is built in.
fn app(api: Router) -> Router {
    let api = api.fallback(|| async { ApiError::not_found("not_found", "There is nothing at this path of the API") });
    let app = Router::new().nest(API_PATH, api);
    #[cfg(feature = "web-ui")]
    let app = app.fallback_service(axum::routing::get(web_ui::serve));
    app
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use caelumora_mcp::tool::MCP_TOOL_KIND;
    use caelumora_model::actor::ActorId;
    use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_model::security::{AllowAllPolicy, SecurityContext};
    use caelumora_model::secret::SecretManager;
    use caelumora_model::store::Persistence;
    use caelumora_model::store::query::Query;
    use caelumora_model::store::EntityStoreFactory;
    use caelumora_model::template::ParticipantTemplate;
    use caelumora_model::type_map::TypeKey;
    use caelumora_model::value::text::Description;
    use caelumora_runtime::backend::Backends;
    use caelumora_runtime::channel::ChannelLoop;
    use caelumora_runtime::component::{ComponentContainer, ComponentContext};
    use caelumora_model::config::BackendConfig;
    use caelumora_runtime::runtime::{RuntimeManager, Services};
    use caelumora_runtime::terminal::TerminalProvider;
    use caelumora_runtime::seed::Seed;
    use caelumora_agents::testing::offered_anywhere;
    use caelumora_agents::tool::ToolProvider;
    use caelumora_skills::skill::Skill;
    use serde_json::json;
    use std::path::Path;
    use tower::ServiceExt;

    /// The file of the examples `name`.
    fn example(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples").join(name)
    }

    /// The runtime of the program, with its plugins, over `store`, with the
    /// policy of the example configuration, which knows the roles the
    /// example seed names, its component defaults, on which the example
    /// seed's models build (issue 445), and its variants of the texts, which
    /// they are given (issue 57) - read from where the program runs, the
    /// root of the sources.
    fn runtime(store: &Persistence) -> RuntimeManager {
        let config = AppConfig::load(example("config.yaml")).unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let prompts = config.prompts.iter().map(|(variant, dir)| (variant.clone(), root.join(dir))).collect();
        let AppConfig { compartments, roles, component_defaults, .. } = config;
        runtime_of(store, &AppConfig { compartments, roles, component_defaults, prompts, ..AppConfig::default() })
    }

    /// The runtime of the program, with its plugins, over `store`, as
    /// `config` configures it.
    fn runtime_of(store: &Persistence, config: &AppConfig) -> RuntimeManager {
        let access_policy = Arc::new(AllowAllPolicy::new());
        let secrets = Arc::new(SecretManager::new(vec![], Arc::<AllowAllPolicy>::clone(&access_policy)));
        let (rest_plugins, plugins) = (rest_plugins(), plugins());
        let plugins = all(&rest_plugins, &plugins);
        let mut builder = RuntimeBuilder::new(store.clone(), secrets, access_policy, config.create_confidentiality_policy(), config, ComponentRegistry::built_in(store.component_store().unwrap()), Backends::with_plugins(&plugins)).unwrap();
        for plugin in plugins {
            builder.plugin(plugin).unwrap();
        }
        default_kinds(builder.components()).unwrap();
        builder.build().unwrap()
    }

    /// An agent whose personality names no model, in a configuration that
    /// names none either, talks to Ollama as before the kind moved into its
    /// plugin (issue 329).
    #[test]
    fn the_default_model_is_served_by_ollama() {
        let runtime = runtime_of(&Persistence::in_memory(), &AppConfig::default());
        assert_eq!(runtime.component_registry().kind_of::<dyn LLMClient>(&ComponentConfig::default()).as_deref(), Some(OLLAMA_CLIENT_KIND));
    }

    #[tokio::test]
    async fn the_example_seed_is_complete_and_seeding_twice_changes_nothing() {
        let seed = Seed::read(&example("seed.yaml")).expect("examples/seed.yaml is a seed");
        assert!(!seed.personalities.is_empty() && !seed.session_templates.is_empty());
        let store = Persistence::in_memory();
        let runtime = runtime(&store);
        let stores = store.entity_stores().unwrap();
        let system = SecurityContext::system();

        runtime.seed(&[example("seed.yaml")]).await.unwrap();
        // Every personality a template names is part of the seed.
        for template in &seed.session_templates {
            for participant in &template.participants {
                if let ParticipantTemplate::Agent { personality_id, .. } = participant {
                    stores.personalities.get(&system, *personality_id).await
                        .unwrap_or_else(|_| panic!("template {} names a personality the seed has", template.name));
                }
            }
        }

        // A document changed in the store stays as it is.
        let mut changed = seed.personalities[0].personality.clone();
        changed.description = Some(Description::new("changed").unwrap());
        stores.personalities.update(&system, &changed).await.unwrap();
        runtime.seed(&[example("seed.yaml")]).await.expect("seeding again is no conflict");
        assert_eq!(stores.personalities.list(&system, &Query::all()).await.map(|page| page.items).unwrap().len(), seed.personalities.len(), "nothing twice");
        assert_eq!(stores.session_templates.list(&system, &Query::all()).await.map(|page| page.items).unwrap().len(), seed.session_templates.len());
        assert_eq!(stores.personalities.get(&system, changed.id).await.unwrap().description, changed.description);
        let skills = store.entity_store::<Skill>().unwrap().list(&system, &Query::all()).await.map(|page| page.items).unwrap();
        let seeded = seed.plugins.get("skills").and_then(|section| section.as_array()).map_or(0, Vec::len);
        assert!(!skills.is_empty() && skills.len() == seeded, "the skills, once, by their plugin (issues 22, 323)");
        assert!(skills.iter().all(|s| s.security_attributes.owner_id == ActorId::System), "the system's");
        let components = store.component_store().unwrap().list_components(&system, &Query::all()).await.map(|page| page.items).unwrap();
        assert!(!components.is_empty() && components.len() == seed.components.len(), "the preconfigured components, once (issue 270)");
    }

    /// Every personality of the example seed can be built with what the
    /// seed and the program's plugins bring: the components it includes
    /// are there, and none of them is written twice (issue 270).
    #[tokio::test]
    async fn every_personality_of_the_example_seed_can_be_built() {
        let seed = Seed::read(&example("seed.yaml")).expect("examples/seed.yaml is a seed");
        let store = Persistence::in_memory();
        let runtime = runtime(&store);
        runtime.seed(&[example("seed.yaml")]).await.unwrap();
        // The services a runtime gives them (issue 296).
        let context = ComponentContext::new(SecurityContext::system(), runtime.services().clone());
        for personality in seed.personalities.iter().map(|seeded| &seeded.personality) {
            // Asked for what its actor asks for (issue 337).
            let asked = runtime.agent_kinds().facets(personality.kind).expect("a kind of agent the program runs");
            ComponentContainer::from_config(&personality.components, asked, runtime.component_registry(), &context).await
                .unwrap_or_else(|e| panic!("personality {} cannot be built: {e}", personality.name));
        }
    }

    /// Every channel of the example seed's templates can be built with
    /// what the program's plugins bring: its moderator and the policy in its
    /// slot, `decision_policy` among them (issue 403).
    #[tokio::test]
    async fn every_channel_of_the_example_seed_can_be_built() {
        let seed = Seed::read(&example("seed.yaml")).expect("examples/seed.yaml is a seed");
        let store = Persistence::in_memory();
        let runtime = runtime(&store);
        runtime.seed(&[example("seed.yaml")]).await.unwrap();
        let context = ComponentContext::new(SecurityContext::system(), runtime.services().clone());
        for (template, channel) in seed.session_templates.iter().flat_map(|template| template.channels.iter().map(move |channel| (template, channel))) {
            ComponentContainer::from_config(&channel.components, ChannelLoop::COMPONENT_FACETS, runtime.component_registry(), &context).await
                .unwrap_or_else(|e| panic!("channel {} of template {} cannot be built: {e}", channel.name, template.name));
        }
    }

    /// A personality that names `mcp_tool` is built as before the kind
    /// moved into its plugin (issue 325): its tool set, unavailable while
    /// nobody answers at the endpoint.
    #[tokio::test]
    async fn the_tools_of_an_mcp_server_are_a_component_of_the_program() {
        let runtime = runtime(&Persistence::in_memory());
        let context = ComponentContext::new(SecurityContext::system(), runtime.services().clone());
        let config = ComponentConfig { component_nodes: vec![ComponentDescriptor { params: json!({ "endpoint": { "url": "http://127.0.0.1:1/mcp" } }), ..ComponentDescriptor::new("tracker", MCP_TOOL_KIND) }] };
        let container = ComponentContainer::from_config(&config, &[TypeKey::of::<dyn ToolProvider>()], runtime.component_registry(), &context).await.expect("the kind is known");
        let tools = container.get_all::<dyn ToolProvider>(None).unwrap();
        assert_eq!(tools.len(), 1);
        assert!(offered_anywhere(tools[0].facet.as_ref()).await.is_err());
    }

    /// The storages of workspaces the example configuration names are made
    /// with the program's plugins, `open_terminal_shared` as before it moved
    /// into its plugin (issue 327); `open_terminal_docker` is known too,
    /// checked without a Docker daemon by parameters it refuses.
    #[tokio::test]
    async fn the_storages_of_workspaces_are_made_with_the_program_s_plugins() {
        let config = AppConfig::load(example("config.yaml")).unwrap();
        let (rest_plugins, plugins) = (rest_plugins(), plugins());
        let backends = Backends::with_plugins(&all(&rest_plugins, &plugins));
        let secrets = Arc::new(SecretManager::new(vec![], Arc::new(AllowAllPolicy::new())));
        let services = Services::empty().with(secrets);
        let storages = backends.make_named::<dyn TerminalProvider>("storage of workspaces", &config.workspaces.storage, &services).expect("every type is known");
        assert!(storages.contains_key("sandbox"), "the shared Open Terminal of the example");
        let docker = [("isolated".to_string(), BackendConfig::new("open_terminal_docker", json!({ "image": "x", "max_run_secs": 0 })))].into();
        let refused = backends.make_named::<dyn TerminalProvider>("storage of workspaces", &docker, &services).map(|_| ()).unwrap_err().to_string();
        assert!(refused.contains("max_run_secs") && !refused.contains("no backend of this type"), "{refused}");
    }

    async fn status_of(app: &Router, uri: &str) -> (StatusCode, Option<String>) {
        let response = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
        let content_type = response.headers().get("content-type").map(|v| v.to_str().unwrap().to_string());
        (response.status(), content_type)
    }

    #[tokio::test]
    async fn the_api_is_served_under_its_path_and_answers_a_wrong_path_as_a_problem() {
        let app = app(Router::new().route("/sessions", get(|| async { "listed" })));
        assert_eq!(status_of(&app, "/api/sessions").await.0, StatusCode::OK);
        assert_eq!(status_of(&app, "/api/nothing").await, (StatusCode::NOT_FOUND, Some("application/problem+json".to_string())), "not the web UI");
    }
}
