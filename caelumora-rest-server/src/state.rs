//! `AppState`: what every handler of the API shares.

use crate::auth::Auth;
use caelumora_domain::confidentiality::Compartments;
use caelumora_kernel::kernel::Kernel;
use caelumora_kernel::procedure::ProcedureExecutor;
use caelumora_kernel::project::ProjectManager;
use caelumora_kernel::session::SessionManager;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// What the handlers share. The stores are the `Kernel`'s - one
/// place to get them from.
pub struct AppState {
    pub auth: Arc<Auth>,
    pub kernel: Arc<Kernel>,
    /// The sessions that run, whose views the handlers read; held, so that the kernel, which knows
    /// it weakly, reaches it as its procedures are carried out.
    pub session_manager: Arc<SessionManager>,
    /// Makes, deletes, restores and purges shared projects (issue 467), made beside
    /// `session_manager`, which it holds.
    pub project_manager: Arc<ProjectManager>,
    /// What carries out the procedures the handlers make - every change of a lifecycle (issue
    /// 593): the kernel's (`Kernel::procedures`).
    pub procedures: Arc<ProcedureExecutor>,
    /// The configured compartments (issue 133), which a client offers for
    /// classifying a channel.
    pub compartments: Arc<Compartments>,
    /// Cancelled as the server shuts down (issue 297): every stream ends
    /// with it, so that the connections it holds close.
    pub shutdown: CancellationToken,
}
