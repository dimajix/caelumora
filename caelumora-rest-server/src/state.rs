//! `AppState`: what every handler of the API shares.

use crate::auth::Auth;
use caelumora_domain::confidentiality::Compartments;
use caelumora_kernel::kernel::Kernel;
use caelumora_kernel::project::ProjectManager;
use caelumora_kernel::session::SessionManager;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// What the handlers share. The stores are the `Kernel`'s - one
/// place to get them from.
pub struct AppState {
    pub auth: Arc<Auth>,
    pub kernel: Arc<Kernel>,
    // Owns the lifecycle of running session loops (start/stop/create/delete).
    pub session_manager: Arc<SessionManager>,
    /// Makes, deletes, restores and purges shared projects (issue 467), made beside
    /// `session_manager`, which it holds.
    pub project_manager: Arc<ProjectManager>,
    /// The configured compartments (issue 133), which a client offers for
    /// classifying a channel.
    pub compartments: Arc<Compartments>,
    /// Cancelled as the server shuts down (issue 297): every stream ends
    /// with it, so that the connections it holds close.
    pub shutdown: CancellationToken,
}
