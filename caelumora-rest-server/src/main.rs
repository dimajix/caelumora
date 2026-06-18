use std::sync::Arc;
use dashmap::DashMap;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use clap::Parser;

use caelumora_model::config::{AppConfig};
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_runtime::session::SessionManager;

mod data;
mod models;
mod routes;
mod state;

use crate::data::setup_initial_data;
use crate::state::AppState;

#[derive(Parser, Debug)]
#[command(author, version, about = "Caelumora REST Server")]
struct Args {
    /// Path to a specific config file
    #[arg(short, long)]
    config: Option<std::path::PathBuf>,

    /// Username to use
    #[arg(short, long, default_value = "user")]
    user: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=debug,axum::rejection=trace".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = if let Some(config_path) = args.config {
        AppConfig::load(config_path)?
    }
    else {
        AppConfig::default()
    };

    let security_manager = Arc::new(caelumora_model::security::DummySecurityManager::new());
    let state_store = config.create_state_store(security_manager.clone())?;
    let entity_store = config.create_entity_store(security_manager.clone())?;

    let runtime_manager = Arc::new(RuntimeManager::new(
        state_store.clone(),
        entity_store.clone(),
    ));
    let session_manager = Arc::new(SessionManager::new(runtime_manager.clone()));

    setup_initial_data(&entity_store, &args.user).await?;

    let state = Arc::new(AppState {
        user_id: caelumora_model::user::UserId::default(),
        runtime_manager,
        session_manager,
        entity_store,
        state_store,
        running_sessions: DashMap::new(),
    });

    let app = routes::create_router(state).layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    tracing::info!(
        "Caelumora REST server listening on {}",
        listener.local_addr()?
    );
    axum::serve(listener, app).await?;

    Ok(())
}
