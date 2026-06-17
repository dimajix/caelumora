use std::sync::Arc;
use dashmap::DashMap;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use clap::Parser;

use caelumora_model::store_impl::memory::MemoryStore;
use caelumora_model::store_impl::json::JsonFileStore;
use caelumora_model::store::Store;
use caelumora_model::config::{AppConfig, PersistenceBackend};
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

    let store: Arc<dyn Store> = if let Some(config_path) = args.config {
        let config = AppConfig::load(config_path)?;
        match config.persistence.backend {
            PersistenceBackend::Memory => Arc::new(MemoryStore::new()),
            PersistenceBackend::Json { path } => Arc::new(JsonFileStore::new(path)?),
        }
    } else {
        Arc::new(MemoryStore::new())
    };

    let runtime_manager = Arc::new(RuntimeManager::new(
        store.clone(),
        store.clone(),
    ));
    let session_manager = Arc::new(SessionManager::new(runtime_manager.clone()));

    setup_initial_data(&store)?;

    let state = Arc::new(AppState {
        runtime_manager,
        session_manager,
        store,
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
