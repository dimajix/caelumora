use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Json};
use crate::state::AppState;
use axum::extract::State;
use caelumora_rest_api::compartment::CompartmentResponse;
use std::sync::Arc;

/// The configured compartments, by name (issue 133) - what a client offers
/// for classifying a channel. Which one a channel may be given is decided
/// when it is created.
pub(super) async fn list_compartments(
    State(state): State<Arc<AppState>>,
    _user: AuthenticatedUser,
) -> Result<Json<Vec<CompartmentResponse>>, ApiError> {
    Ok(Json(state.compartments.names().map(CompartmentResponse::from).collect()))
}
