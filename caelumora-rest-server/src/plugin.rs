//! A plugin's routes (issue 323).

use crate::state::AppState;
use axum::Router;
use caelumora_runtime::plugin::Plugin;
use std::sync::Arc;

/// A plugin that serves routes of its own in the API. They are built with
/// the state every handler shares, from which the plugin takes what its
/// handlers need - its manager among the runtime's services - into a state
/// of their own, and served behind the same authentication as the core's
/// routes (`routes::api`).
pub trait RestPlugin: Plugin {
    /// Its routes, their state applied, below the path the API is served
    /// at.
    fn routes(&self, state: Arc<AppState>) -> Router;
}
