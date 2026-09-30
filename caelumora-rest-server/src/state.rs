use crate::auth::Auth;
use caelumora_model::confidentiality::Compartments;
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_runtime::session::SessionManager;
use std::sync::Arc;

/// What the handlers share. The stores are the `RuntimeManager`'s - one
/// place to get them from.
pub struct AppState {
    pub auth: Arc<Auth>,
    pub runtime_manager: Arc<RuntimeManager>,
    // Owns the lifecycle of running session loops (start/stop/create/delete).
    pub session_manager: Arc<SessionManager>,
    /// The configured compartments (issue 133), which a client offers for
    /// classifying a channel.
    pub compartments: Arc<Compartments>,
}
