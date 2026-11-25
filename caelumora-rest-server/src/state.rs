//! `AppState`: what every handler of the API shares.

use crate::auth::Auth;
use caelumora_domain::confidentiality::Compartments;
use caelumora_kernel::kernel::Kernel;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// What the handlers share. The stores are the `Kernel`'s - one
/// place to get them from.
pub struct AppState {
    pub auth: Arc<Auth>,
    pub kernel: Arc<Kernel>,
    /// The configured compartments (issue 133), which a client offers for
    /// classifying a channel.
    pub compartments: Arc<Compartments>,
    /// Cancelled as the server shuts down (issue 297): every stream ends
    /// with it, so that the connections it holds close.
    pub shutdown: CancellationToken,
}
