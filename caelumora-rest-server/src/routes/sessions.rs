use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Subject};
use crate::models::{CreateSessionRequest, DeletedResponse, DescriptionRequest, ListOptions, PrivateNotesResponse, RenameSessionRequest, SessionPrivacyRequest, SessionResponse, SessionSnapshot};
use crate::state::AppState;
use axum::response::{IntoResponse, Response};
use axum::{extract::{Query, State}, http::StatusCode};
use crate::error::{Json, Path};
use caelumora_model::agent::AgentId;
use caelumora_model::approval::ApprovalRule;
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
    let session_loop = state.session_manager.create_from_template(&user.security_context(), template_id, payload.name.as_deref(), payload.workspace.map(Into::into)).await
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
    Ok(Json(crate::routes::session_view(&state, &user, session_id).await?))
}

/// The private notes agent `aid` wrote in the channels of session `id` the
/// user may read (issue 233), sorted by the channel's name, for whoever may
/// read what it saw (`ReadAgentContext`).
pub async fn agent_private_notes(
    State(state): State<Arc<AppState>>,
    Path((id, aid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<PrivateNotesResponse>>, ApiError> {
    let session_id = SessionId(id);
    let session = crate::routes::views(&state, &user).session(session_id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Session, &format!("loading session {session_id}")))?;
    let found = session.private_notes_of(AgentId(aid)).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Context, "reading the private notes"))?;
    Ok(Json(found.into_iter().map(|(channel, notes, digest)| PrivateNotesResponse {
        channel_id: channel.id(),
        channel_name: channel.name().to_string(),
        notes,
        digest,
    }).collect()))
}

/// Restores the deleted session `id` (issue 164) and answers it as `GET`
/// does. It is not started; opening it starts it.
pub async fn restore_session(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<SessionSnapshot>, ApiError> {
    let session_id = SessionId(id);
    state.session_manager.restore_session(&user.security_context(), session_id).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, &format!("restoring session {session_id}")))?;
    Ok(Json(crate::routes::session_view(&state, &user, session_id).await?))
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

    session_loop.update_name(&user.security_context(), &payload.name).await
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

    Ok(Json(crate::routes::session_view(&state, &user, session_id).await?))
}

/// Sets what the session restricts of the tool calls made in any of its
/// channels (issue 13): rules that ask or deny, replacing those it had -
/// whoever may change the session. Answers `400 invalid_rule` for a rule
/// that would allow, is narrower than the session or cannot be matched, and
/// otherwise the session as the user sees it now.
pub async fn update_session_approval_rules(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    user: AuthenticatedUser,
    Json(rules): Json<Vec<ApprovalRule>>,
) -> Result<Json<SessionSnapshot>, ApiError> {
    let session_id = SessionId(id);
    let session_loop = crate::routes::ensure_session(&state, &user, session_id).await?;

    session_loop.update_approval_rules(&user.security_context(), rules).await
        .map_err(|e| crate::routes::change_error(e, "invalid_rule", Subject::Session, &format!("changing the approval rules of session {session_id}")))?;

    Ok(Json(crate::routes::session_view(&state, &user, session_id).await?))
}

/// Says what session `id` is for (issue 245): its summary and briefing,
/// `null` or empty to remove one - whoever may change the session. They are
/// public to everybody in it. Answers `400 invalid_value` for one that does
/// not fit, and otherwise the session as the user sees it now.
pub async fn update_session_description(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    user: AuthenticatedUser,
    Json(payload): Json<DescriptionRequest>,
) -> Result<Json<SessionSnapshot>, ApiError> {
    let session_id = SessionId(id);
    let session_loop = crate::routes::ensure_session(&state, &user, session_id).await?;

    session_loop.update_description(&user.security_context(), payload.summary.as_deref(), payload.briefing.as_deref()).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, &format!("changing the description of session {session_id}")))?;

    Ok(Json(crate::routes::session_view(&state, &user, session_id).await?))
}
