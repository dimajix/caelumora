use caelumora_model::config::AppConfig;
use caelumora_model::secret::{SecretError, SecretManager};
use caelumora_model::security::SecurityContext;
use caelumora_runtime::component::ComponentRegistry;
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_runtime::session::SessionManager;
use clap::Parser;
use std::sync::Arc;
use caelumora_runtime::logging::{self, LogTarget};
use caelumora_runtime::event::EventBus;
use tower_http::trace::TraceLayer;

mod auth;
mod error;
mod models;
mod routes;
mod sse;
mod state;

use crate::auth::{Auth, TokenService};
use crate::state::AppState;
use caelumora_model::store::memberships::StoreMemberships;

#[derive(Parser, Debug)]
#[command(author, version, about = "Caelumora REST Server")]
struct Args {
    /// Path to a specific config file
    #[arg(short, long)]
    config: Option<std::path::PathBuf>,
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

    logging::init(&config.logging, LogTarget::Stderr, "info,tower_http=debug,axum::rejection=trace")?;

    // The policy learns who takes part where from the store it guards
    // (issue 23): made before the store, bound to it once it exists.
    let memberships = Arc::new(StoreMemberships::new());
    let security_manager = config.create_security_manager(Arc::<StoreMemberships>::clone(&memberships));
    let state_store = config.create_state_store(Arc::clone(&security_manager))?;
    let entity_store = config.create_entity_store(Arc::clone(&security_manager))?;
    memberships.bind(&entity_store);
    let component_store = config.create_component_store(Arc::clone(&security_manager))?;
    let secret_manager = config.create_secret_manager(Arc::clone(&security_manager))?;
    let runtime_manager = Arc::new(RuntimeManager::new(
        state_store,
        Arc::clone(&entity_store),
        Arc::clone(&secret_manager),
        Arc::clone(&security_manager),
        &config,
        ComponentRegistry::built_in(Arc::clone(&component_store)),
    )?);
    let session_manager = Arc::new(SessionManager::new(Arc::clone(&runtime_manager)));
    // Users are recorded and their changes announced on the runtime's event
    // bus (issue 306), so the user manager comes after the runtime.
    let user_manager = config.create_user_manager(Arc::clone(&entity_store), Arc::<EventBus>::clone(runtime_manager.event_bus()))?;
    if user_manager.is_empty() {
        tracing::warn!("No user providers configured (`users` section): nobody can log in. \
            See examples/config.yaml.");
    }
    let signing_key = jwt_signing_key(&secret_manager, &config.auth.jwt_secret).await?;
    let auth = Arc::new(Auth {
        tokens: TokenService::new(&signing_key, &config.auth)?,
        users: user_manager,
        controllers: Arc::clone(runtime_manager.controller_manager()),
    });

    caelumora_runtime::seed::load(entity_store.as_ref(), component_store.as_ref(), security_manager.as_ref(), &config.seed).await?;

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
    let app = routes::create_router(state)
        .layer(axum::middleware::from_fn(error::request_id))
        .layer(trace_layer);

    let listener = tokio::net::TcpListener::bind(config.server.address).await?;
    tracing::info!(
        "Caelumora REST server listening on {}",
        listener.local_addr()?
    );
    axum::serve(listener, app).await?;

    Ok(())
}

/// Resolves the key for signing session tokens through the secret chain. If
/// the secret does not exist, a random key is generated - sessions then do not
/// survive a restart, which is fine for development and loudly announced.
async fn jwt_signing_key(secrets: &SecretManager, name: &str) -> anyhow::Result<Vec<u8>> {
    match secrets.resolve(&SecurityContext::system(), name).await {
        Ok(key) => Ok(key.into_bytes()),
        Err(SecretError::NotFound(_)) => {
            tracing::warn!("Secret '{}' not found: signing session tokens with a random key. \
                Every restart logs all users out.", name);
            let key: [u8; 32] = rand::random();
            Ok(key.to_vec())
        }
        Err(e) => Err(e.into()),
    }
}
