//! The logged-in user's own profile (issue 176): how they introduce
//! themselves to the agents of every channel they take part in.

use std::sync::Arc;
use axum::extract::State;
use axum::http::StatusCode;
use caelumora_model::store::{StoreError, StoreResultExt};
use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Json};
use crate::models::{ProfileRequest, ProfileResponse};
use crate::state::AppState;

/// The user's profile; one without an introduction if they never wrote one.
pub async fn get_profile(State(state): State<Arc<AppState>>, user: AuthenticatedUser) -> Result<Json<ProfileResponse>, ApiError> {
    let store = state.runtime_manager.entity_store();
    let profile = store.get_user_profile(&user.security_context(), user.id).await.optional()
        .map_err(|e| ApiError::internal("profile_failed", "reading the profile", &e.into()))?;
    Ok(Json(ProfileResponse::of(profile)))
}

/// Sets the user's profile: their introduction - `null` or blank removes
/// it - and the language they prefer (issue 56), `null` for none. Answers
/// `400 invalid_introduction` for an introduction that is too long or
/// contains control characters. Changed through the user's controller, the
/// profile's one writer (issue 13).
pub async fn update_profile(State(state): State<Arc<AppState>>, user: AuthenticatedUser, Json(payload): Json<ProfileRequest>) -> Result<Json<ProfileResponse>, ApiError> {
    let controller = state.runtime_manager.controller_manager().user_controller(user.id).await
        .map_err(|e| ApiError::internal("profile_failed", "reading the profile", &e.into()))?;
    let profile = controller.update_profile(&user.security_context(), payload.introduction.as_deref(), payload.preferred_language).await
        .map_err(|e| match e {
            StoreError::Validation(reason) => ApiError::new(StatusCode::BAD_REQUEST, "invalid_introduction", reason),
            e => ApiError::internal("profile_failed", "saving the profile", &e.into()),
        })?;
    Ok(Json(ProfileResponse::of(Some(profile))))
}

