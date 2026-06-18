use std::sync::Arc;
use caelumora_model::store::{StateStore, EntityStore};
use caelumora_model::session::SessionId;
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_runtime::session::SessionManager;
use caelumora_model::user::UserId;
use dashmap::DashMap;

pub struct AppState {
    pub user_id: UserId,
    #[allow(dead_code)]
    pub runtime_manager: Arc<RuntimeManager>,
    pub session_manager: Arc<SessionManager>,
    pub entity_store: Arc<dyn EntityStore>,
    pub state_store: Arc<dyn StateStore>,
    // Map of session ID to join handles of running session loops
    pub running_sessions: DashMap<SessionId, tokio::task::JoinHandle<()>>,
}
