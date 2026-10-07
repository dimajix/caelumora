//! The logged-in user's own profile (issue 176): how they introduce
//! themselves to the agents of every channel they take part in, and their
//! answers to tool calls given in advance (issue 13).

use std::sync::Arc;
use axum::extract::State;
use axum::http::StatusCode;
use caelumora_model::approval::{ApprovalRule, ApprovalRuleId};
use caelumora_model::store::{StoreError, StoreResultExt};
use caelumora_runtime::controller::UserController;
use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Json, Path};
use crate::models::{ApprovalRuleRequest, ProfileRequest, ProfileResponse};
use uuid::Uuid;
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
    let controller = controller_of(&state, &user).await?;
    let introduction = payload.introduction()?;
    let profile = controller.update_profile(&user.security_context(), introduction, payload.preferred_language).await
        .map_err(|e| ApiError::internal("profile_failed", "saving the profile", &e.into()))?;
    Ok(Json(ProfileResponse::of(Some(profile))))
}


/// Adds a rule to the user's answers to tool calls given in advance (issue
/// 13): `201` with the rule and its id. Answers `400 invalid_rule` for one
/// that asks - a person allows or denies - whose pattern cannot be matched,
/// or one too many.
pub async fn add_approval_rule(State(state): State<Arc<AppState>>, user: AuthenticatedUser, Json(payload): Json<ApprovalRuleRequest>) -> Result<(StatusCode, Json<ApprovalRule>), ApiError> {
    let invalid = |reason: String| ApiError::new(StatusCode::BAD_REQUEST, "invalid_rule", reason);
    let rule = ApprovalRule::new(payload.matcher, payload.decision, payload.reach, payload.expires_at).map_err(|e| invalid(e.to_string()))?;
    let controller = controller_of(&state, &user).await?;
    controller.add_approval_rule(&user.security_context(), rule.clone()).await
        .map_err(|e| match e {
            StoreError::Validation(reason) => invalid(reason),
            e => ApiError::internal("profile_failed", "saving the profile", &e.into()),
        })?;
    Ok((StatusCode::CREATED, Json(rule)))
}

/// Takes one of the user's rules back (issue 13): `204`, or `404
/// rule_not_found` if they have no such rule.
pub async fn remove_approval_rule(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let controller = controller_of(&state, &user).await?;
    controller.remove_approval_rule(&user.security_context(), ApprovalRuleId(id)).await
        .map_err(|e| match e {
            StoreError::NotFound { .. } => ApiError::not_found("rule_not_found", "You have no such rule"),
            e => ApiError::internal("profile_failed", "saving the profile", &e.into()),
        })?;
    Ok(StatusCode::NO_CONTENT)
}

/// The controller of the user's profile, its one writer (issue 13).
async fn controller_of(state: &AppState, user: &AuthenticatedUser) -> Result<Arc<UserController>, ApiError> {
    state.runtime_manager.controller_manager().user_controller(user.id).await
        .map_err(|e| ApiError::internal("profile_failed", "reading the profile", &e.into()))
}
