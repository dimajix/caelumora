use crate::auth::Auth;
use caelumora_model::store::{EntityStore, StateStore};
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_runtime::session::SessionManager;
use std::sync::Arc;

pub struct AppState {
    pub auth: Arc<Auth>,
    pub runtime_manager: Arc<RuntimeManager>,
    // Owns the lifecycle of running session loops (start/stop/create/delete).
    pub session_manager: Arc<SessionManager>,
    pub entity_store: Arc<dyn EntityStore>,
    pub state_store: Arc<dyn StateStore>,
}
