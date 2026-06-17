use std::sync::Arc;
use caelumora_model::store::Store;
use caelumora_model::session::SessionId;
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_runtime::session::SessionManager;
use dashmap::DashMap;

pub struct AppState {
    #[allow(dead_code)]
    pub runtime_manager: Arc<RuntimeManager>,
    pub session_manager: Arc<SessionManager>,
    pub store: Arc<dyn Store>,
    // Map of session ID to join handles of running session loops
    pub running_sessions: DashMap<SessionId, tokio::task::JoinHandle<()>>,
}
