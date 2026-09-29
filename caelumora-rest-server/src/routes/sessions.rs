use caelumora_model::store::StoreError;
use crate::auth::AuthenticatedUser;
use crate::error::ApiError;
use crate::models::{CreateSessionRequest, RenameSessionRequest, SessionResponse, SessionSnapshot};
use crate::state::AppState;
use axum::{extract::State, http::StatusCode};
use crate::error::{Json, Path};
use caelumora_model::session::{SessionContext, SessionId};
use caelumora_model::template::{ParticipantId, SessionTemplateId};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

pub async fn create_session(
    State(state): State<Arc<AppState>>,
    user: AuthenticatedUser,
    Json(payload): Json<CreateSessionRequest>,
) -> Result<Json<SessionResponse>, ApiError> {
    let ctx = user.security_context();
    let template_id = SessionTemplateId(payload.template_id);
    let template = match state.entity_store.get_session_template(&ctx, template_id).await {
        Ok(template) => template,
        Err(StoreError::NotFound { .. }) => {
            return Err(ApiError::not_found("template_not_found", format!("Session template {} does not exist", template_id)));
        }
        Err(e) => return Err(ApiError::internal("templates_unavailable", "reading the session template", &e.into())),
    };

    let session_id = SessionId(Uuid::new_v4());
    // The logged-in user owns the session and fills its "user" participant.
    let owner = user.id;
    let security = user.security_context();
    let context = SessionContext {
        id: session_id,
        owner,
        security,
        entity_store: state.entity_store.clone(),
    };

    let name = payload.name.unwrap_or_else(|| template.metadata.name.clone());
    let mut mapping = HashMap::new();
    mapping.insert("user".to_string(), ParticipantId::UserId(owner));
    let (session, channels) = template.instantiate(name, &context, mapping).await
        .map_err(|e| ApiError::internal("template_invalid", &format!("instantiating template '{}'", template.metadata.name), &e.into()))?;

    // `create_session` persists the session, emits its lifecycle events, and starts it.
    let session_loop = state.session_manager.create_session(&ctx, session, channels).await
        .map_err(|e| ApiError::internal("session_start_failed", "starting the new session", &e))?;

    let meta =  session_loop.get_session().metadata.clone();
    Ok(Json(SessionResponse {
        id: session_id,
        name: meta.name.clone(),
        metadata: meta,
    }))
}

pub async fn list_sessions(State(state): State<Arc<AppState>>, user: AuthenticatedUser) -> Result<Json<Vec<SessionResponse>>, ApiError> {
    let ctx = user.security_context();
    let sessions = state.entity_store.list_sessions(&ctx).await
        .map_err(|e| ApiError::internal("sessions_unavailable", "listing the sessions", &e.into()))?;

    let response = sessions
        .into_iter()
        .map(|s| {
            // If the session is running, get the latest metadata from the session loop
            let (id, name, metadata) = if let Some(session_loop) = state.session_manager.get_session(s.id) {
                let meta = session_loop.get_session().metadata.clone();
                (session_loop.id(), meta.name.clone(), meta)
            } else {
                (s.id, s.metadata.name.clone(), s.metadata)
            };

            SessionResponse {
                id,
                name,
                metadata,
            }
        })
        .collect();

    Ok(Json(response))
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
    let session = state.entity_store.get_session(&user.security_context(), session_id).await
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

    let meta =  session_loop.get_session().metadata.clone();
    Ok(Json(SessionResponse {
        id: session_id,
        name: meta.name.clone(),
        metadata: meta,
    }))
}
