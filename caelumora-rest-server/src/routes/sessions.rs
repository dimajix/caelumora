use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Subject};
use crate::models::{CreateSessionRequest, DeletedResponse, ListOptions, RenameSessionRequest, SessionPrivacyRequest, SessionResponse, SessionSnapshot};
use crate::state::AppState;
use axum::response::{IntoResponse, Response};
use axum::{extract::{Query, State}, http::StatusCode};
use crate::error::{Json, Path};
use caelumora_model::snapshot::Detail;
use caelumora_model::session::SessionId;
use caelumora_model::template::SessionTemplateId;
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
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, &format!("creating a session from template {template_id}")))?;

    Ok(Json(SessionResponse::from(&**session_loop.session())))
}

/// The sessions the user may read - or, with `?deleted=true`, those they
/// deleted and may read as such, the trash (issue 164).
pub async fn list_sessions(State(state): State<Arc<AppState>>, Query(options): Query<ListOptions>, user: AuthenticatedUser) -> Result<Response, ApiError> {
    let views = crate::routes::views(&state, &user);
    if options.deleted {
        let deleted = views.deleted_sessions().await
            .map_err(|e| ApiError::internal("sessions_unavailable", "listing the deleted sessions", &e.into()))?;
        return Ok(Json(deleted.into_iter().map(DeletedResponse::from).collect::<Vec<_>>()).into_response());
    }
    let sessions = views.sessions().await
        .map_err(|e| ApiError::internal("sessions_unavailable", "listing the sessions", &e.into()))?;
    let mut listed = Vec::with_capacity(sessions.len());
    for session in sessions {
        listed.push(SessionResponse::from(session.snapshot(Detail::empty()).await));
    }
    Ok(Json(listed).into_response())
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
    let session = crate::routes::views(&state, &user).session(session_id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Session, &format!("loading session {session_id}")))?;
    Ok(Json(session.snapshot(Detail::CHANNELS | Detail::MEMBERS).await.into()))
}

/// Restores the deleted session `id` (issue 164) and answers it as `GET`
/// does. It is not started; opening it starts it.
pub async fn restore_session(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<SessionSnapshot>, ApiError> {
    let session_id = SessionId(id);
    state.session_manager.restore_session(&user.security_context(), session_id).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, &format!("restoring session {session_id}")))?;
    let session = crate::routes::views(&state, &user).session(session_id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Session, &format!("loading session {session_id}")))?;
    Ok(Json(session.snapshot(Detail::CHANNELS | Detail::MEMBERS).await.into()))
}

/// Removes the deleted session `id` for good, with its channels and their
/// state (issue 164) - as the policy allows, by default the system's alone.
pub async fn purge_session(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let session_id = SessionId(id);
    state.session_manager.purge_session(&user.security_context(), session_id).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, &format!("purging session {session_id}")))?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_session(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let session_id = SessionId(id);
    // `delete_session` stops the running loop, marks the session deleted -
    // kept with its channels until it is purged (issue 164) - and emits the
    // corresponding lifecycle events.
    match state.session_manager.delete_session(&user.security_context(), session_id).await {
        Ok(_) => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err(ApiError::from_runtime(e, Subject::Session, &format!("deleting session {session_id}"))),
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
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, &format!("renaming session {session_id}")))?;

    Ok(Json(SessionResponse::from(&**session_loop.session())))
}

/// Changes whom session `id` lets read what was said in it (issue 134);
/// only its owner may. Answers the session as the snapshot shows it.
pub async fn update_session_privacy(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    user: AuthenticatedUser,
    Json(payload): Json<SessionPrivacyRequest>,
) -> Result<Json<SessionSnapshot>, ApiError> {
    let session_id = SessionId(id);
    let session_loop = crate::routes::ensure_session(&state, &user, session_id).await?;

    session_loop.update_privacy(&user.security_context(), payload.privacy).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, &format!("changing the privacy of session {session_id}")))?;

    let session = crate::routes::views(&state, &user).session(session_id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Session, &format!("loading session {session_id}")))?;
    Ok(Json(session.snapshot(Detail::CHANNELS | Detail::MEMBERS).await.into()))
}
