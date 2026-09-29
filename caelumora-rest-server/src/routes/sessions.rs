use crate::auth::AuthenticatedUser;
use crate::error::ApiError;
use crate::models::{CreateSessionRequest, RenameSessionRequest, SessionResponse, SessionSnapshot};
use crate::state::AppState;
use axum::{extract::State, http::StatusCode};
use crate::error::{Json, Path};
use caelumora_model::session::SessionId;
use caelumora_model::template::{SessionTemplateId, TemplateError};
use caelumora_runtime::error::is_not_found;
use std::sync::Arc;
use uuid::Uuid;

pub async fn create_session(
    State(state): State<Arc<AppState>>,
    user: AuthenticatedUser,
    Json(payload): Json<CreateSessionRequest>,
) -> Result<Json<SessionResponse>, ApiError> {
    // The logged-in user owns the session and is its template's "user".
    let template_id = SessionTemplateId(payload.template_id);
    let session_loop = state.session_manager.create_from_template(&user.security_context(), template_id, payload.name).await
        .map_err(|e| {
            if is_not_found(&e, "session_template") {
                ApiError::not_found("template_not_found", format!("Session template {} does not exist", template_id))
            } else if let Some(invalid) = e.downcast_ref::<TemplateError>() {
                ApiError::new(StatusCode::BAD_REQUEST, "template_invalid", invalid.to_string())
            } else {
                ApiError::internal("session_start_failed", "creating a session from a template", &e)
            }
        })?;

    Ok(Json(SessionResponse::from(&**session_loop.get_session())))
}

pub async fn list_sessions(State(state): State<Arc<AppState>>, user: AuthenticatedUser) -> Result<Json<Vec<SessionResponse>>, ApiError> {
    let ctx = user.security_context();
    let sessions = state.runtime_manager.entity_store().list_sessions(&ctx).await
        .map_err(|e| ApiError::internal("sessions_unavailable", "listing the sessions", &e.into()))?;

    // A running session changes nothing it has not stored first: the store
    // says what it is.
    Ok(Json(sessions.iter().map(SessionResponse::from).collect()))
}

/// One session with its channels, in the form the global snapshot uses - so
/// that a client can follow an event that names a session by id alone
/// (issue 35). Does not start a stopped session.
pub async fn get_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    user: AuthenticatedUser,
) -> Result<Json<SessionSnapshot>, ApiError> {
    let session_id = SessionId(id);
    let session = state.runtime_manager.entity_store().get_session(&user.security_context(), session_id).await
        .map_err(|e| {
            let error = anyhow::Error::new(e);
            if caelumora_runtime::error::is_not_found(&error, "session") {
                ApiError::not_found("session_not_found", format!("Session {} does not exist", session_id))
            } else {
                ApiError::internal("session_unavailable", &format!("loading session {}", session_id), &error)
            }
        })?;
    Ok(Json(crate::routes::events::describe_session(&state, session, &user.security_context()).await))
}

pub async fn delete_session(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let session_id = SessionId(id);
    let ctx = user.security_context();

    // `delete_session` stops the running loop, removes the persisted session (and all of its
    // channels), and emits the corresponding lifecycle events.
    match state.session_manager.delete_session(&ctx, session_id).await {
        Ok(_) => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err(crate::routes::session_error(e, session_id, "delete", "deleting")),
    }
}

pub async fn rename_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    user: AuthenticatedUser,
    Json(payload): Json<RenameSessionRequest>,
) -> Result<Json<SessionResponse>, ApiError> {
    let session_id = SessionId(id);
    let session_loop = crate::routes::ensure_session(&state, &user, session_id).await?;

    session_loop.update_name(&user.security_context(), payload.name).await
        .map_err(|e| crate::routes::session_error(e, session_id, "rename", "renaming"))?;

    Ok(Json(SessionResponse::from(&**session_loop.get_session())))
}
