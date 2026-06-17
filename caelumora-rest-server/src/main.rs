use std::sync::Arc;
use dashmap::DashMap;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use caelumora_model::driver::memory::MemoryStore;
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_runtime::session::SessionManager;

mod data;
mod models;
mod routes;
mod state;

use crate::data::setup_initial_data;
use crate::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=debug,axum::rejection=trace".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let memory_store = Arc::new(MemoryStore::new());
    let runtime_manager = Arc::new(RuntimeManager::new(
        memory_store.clone(),
        memory_store.clone(),
    ));
    let session_manager = Arc::new(SessionManager::new(runtime_manager.clone()));

    setup_initial_data(&memory_store)?;

    let state = Arc::new(AppState {
        runtime_manager,
        session_manager,
        memory_store,
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
