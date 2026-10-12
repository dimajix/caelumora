//! Caelumora as one program (issue 323): the composition root that lists
//! the plugins, builds the runtime with them, serves the REST API with
//! their routes under `/api` and - with the feature `web-ui` - the web UI at
//! `/`, from the binary.

use axum::Router;
use caelumora_logging::LogTarget;
use caelumora_model::config::AppConfig;
use caelumora_rest_server::auth::{Auth, TokenService};
use caelumora_rest_server::error::{self, ApiError};
use caelumora_rest_server::plugin::RestPlugin;
use caelumora_rest_server::routes;
use caelumora_rest_server::state::AppState;
use caelumora_runtime::event::EventBus;
use caelumora_runtime::runtime::RuntimeBuilder;
use caelumora_runtime::session::SessionManager;
use caelumora_skills::plugin::SkillsPlugin;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::trace::TraceLayer;

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
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let config = if let Some(config_path) = args.config {
        AppConfig::load(config_path)?
    }
    else {
        AppConfig::default()
    };

    caelumora_logging::init(&config.logging, LogTarget::Stderr, "info,tower_http=debug,axum::rejection=trace")?;

    let plugins = plugins();
    let mut builder = RuntimeBuilder::from_config(&config)?;
    for plugin in &plugins {
        builder.plugin(plugin.as_ref())?;
    }
    let runtime_manager = Arc::new(builder.build()?);
    let session_manager = Arc::new(SessionManager::new(Arc::clone(&runtime_manager)));
    // Users are recorded and their changes announced on the runtime's event
    // bus (issue 306), so the user manager comes after the runtime.
    let user_manager = config.create_user_manager(Arc::clone(&runtime_manager.entity_stores().users), Arc::<EventBus>::clone(runtime_manager.event_bus()))?;
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

    let state = Arc::new(AppState {
        auth,
        runtime_manager,
        session_manager,
        compartments: Arc::new(config.compartments.clone()),
    });

    // One span per request. `request_id` is filled in by `error::request_id`,
    // `user_id` by `auth::require_auth`,
    // `span_id` by handlers that start an envelope chain in a channel.
    let trace_layer = TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
        tracing::info_span!(
            "request",
            method = %request.method(),
            uri = %request.uri(),
            request_id = tracing::field::Empty,
            user_id = tracing::field::Empty,
            span_id = tracing::field::Empty,
        )
    });
    // The request id is set inside the request span, which is the outer layer.
    let app = app(routes::api(state, &plugins))
        .layer(axum::middleware::from_fn(error::request_id))
        .layer(trace_layer);

    let listener = tokio::net::TcpListener::bind(config.server.address).await?;
    tracing::info!("Caelumora listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;

    Ok(())
}

/// The plugins of the program (issue 323), which it builds its runtime and
/// its API with.
fn plugins() -> Vec<Box<dyn RestPlugin>> {
    vec![Box::new(SkillsPlugin)]
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
    use caelumora_model::actor::ActorId;
    use caelumora_model::security::{DummySecurityManager, SecurityContext};
    use caelumora_model::secret::SecretManager;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::store::query::Query;
    use caelumora_model::store::{ComponentStore, EntityStoreFactory};
    use caelumora_model::template::ParticipantTemplate;
    use caelumora_model::value::text::Description;
    use caelumora_runtime::component::{ComponentContainer, ComponentContext, ComponentRegistry};
    use caelumora_runtime::runtime::RuntimeManager;
    use caelumora_runtime::seed::Seed;
    use caelumora_skills::skill::Skill;
    use std::path::Path;
    use tower::ServiceExt;

    /// The file of the examples `name`.
    fn example(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples").join(name)
    }

    /// The runtime of the program, with its plugins, over `store`, with the
    /// policy of the example configuration, which knows the roles the
    /// example seed names.
    fn runtime(store: &Arc<MemoryStore>) -> RuntimeManager {
        let config = AppConfig::load(example("config.yaml")).unwrap();
        let security = Arc::new(DummySecurityManager::new());
        let secrets = Arc::new(SecretManager::new(vec![], Arc::<DummySecurityManager>::clone(&security)));
        let mut builder = RuntimeBuilder::new(Arc::<MemoryStore>::clone(store), Arc::<MemoryStore>::clone(store), secrets, security, config.create_confidentiality_policy(), &AppConfig::default(), ComponentRegistry::built_in(Arc::<MemoryStore>::clone(store))).unwrap();
        for plugin in plugins() {
            builder.plugin(plugin.as_ref()).unwrap();
        }
        builder.build().unwrap()
    }

    #[tokio::test]
    async fn the_example_seed_is_complete_and_seeding_twice_changes_nothing() {
        let seed = Seed::read(&example("seed.yaml")).expect("examples/seed.yaml is a seed");
        assert!(!seed.personalities.is_empty() && !seed.session_templates.is_empty());
        let store = Arc::new(MemoryStore::new());
        let runtime = runtime(&store);
        let stores = store.entity_stores();
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
        let components = store.list_components(&system, &Query::all()).await.map(|page| page.items).unwrap();
        assert!(!components.is_empty() && components.len() == seed.components.len(), "the preconfigured components, once (issue 270)");
    }

    /// Every personality of the example seed can be built with what the
    /// seed and the program's plugins bring: the components it includes
    /// are there, and none of them is written twice (issue 270).
    #[tokio::test]
    async fn every_personality_of_the_example_seed_can_be_built() {
        let seed = Seed::read(&example("seed.yaml")).expect("examples/seed.yaml is a seed");
        let store = Arc::new(MemoryStore::new());
        let runtime = runtime(&store);
        runtime.seed(&[example("seed.yaml")]).await.unwrap();
        // The services a runtime gives them (issue 296).
        let context = ComponentContext::new(SecurityContext::system(), runtime.services().clone());
        for personality in seed.personalities.iter().map(|seeded| &seeded.personality) {
            ComponentContainer::from_config(&personality.components, runtime.component_registry(), &context).await
                .unwrap_or_else(|e| panic!("personality {} cannot be built: {e}", personality.name));
        }
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
