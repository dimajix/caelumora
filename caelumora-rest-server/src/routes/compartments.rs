use std::sync::Arc;
use axum::extract::State;
use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Json};
use crate::models::CompartmentResponse;
use crate::state::AppState;

/// The configured compartments, by name (issue 133) - what a client offers
/// for classifying a channel. Which one a channel may be given is decided
/// when it is created.
pub async fn list_compartments(State(state): State<Arc<AppState>>, _user: AuthenticatedUser) -> Result<Json<Vec<CompartmentResponse>>, ApiError> {
    Ok(Json(state.compartments.names().map(CompartmentResponse::from).collect()))
}
