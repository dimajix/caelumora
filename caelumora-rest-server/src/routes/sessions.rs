use std::sync::Arc;
use axum::{extract::{Path, State}, http::StatusCode, Json};
use uuid::Uuid;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::{SessionContext, SessionId};
use caelumora_model::template::SessionTemplateId;
use crate::state::AppState;
use crate::models::{CreateSessionRequest, SessionResponse, RenameSessionRequest};

pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateSessionRequest>,
) -> Result<Json<SessionResponse>, StatusCode> {
    let ctx = SecurityContext::default();
    let template_id = SessionTemplateId(payload.template_id);
    let template = state.entity_store.list_session_templates(&ctx).await
        .expect("failed to list templates")
        .into_iter()
        .find(|t| t.id == template_id)
        .ok_or(StatusCode::NOT_FOUND)?;

    let session_id = SessionId(Uuid::new_v4());
    let owner = state.user_id;
    let security = SecurityContext::default();
    let context = SessionContext {
        id: session_id,
        owner,
        security,
        entity_store: state.entity_store.clone(),
    };

    let name = payload.name.unwrap_or_else(|| template.metadata.name.clone());
    let session = template.instantiate(name, &context).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let session_loop = state.session_manager.create_session(session).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let session_loop_clone = session_loop.clone();
    let handle = tokio::spawn(async move {
        if let Err(e) = session_loop_clone.run().await {
            tracing::error!("Session loop for {} failed: {}", session_id, e);
        }
    });

    state.running_sessions.insert(session_id, handle);

    let meta =  session_loop.get_session().metadata.clone();
    Ok(Json(SessionResponse {
        id: session_id,
        name: meta.name.clone(),
        metadata: meta,
    }))
}

pub async fn list_sessions(State(state): State<Arc<AppState>>) -> Result<Json<Vec<SessionResponse>>, StatusCode> {
    let ctx = SecurityContext::default();
    let sessions = state.entity_store.list_sessions(&ctx).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

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

pub async fn terminate_session(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>) -> StatusCode {
    let session_id = SessionId(id);
    if let Some((_, handle)) = state.running_sessions.remove(&session_id) {
        handle.abort();
    }
    
    state.session_manager.sessions.remove(&session_id);
    StatusCode::NO_CONTENT
}

pub async fn rename_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(payload): Json<RenameSessionRequest>,
) -> Result<Json<SessionResponse>, StatusCode> {
    let session_id = SessionId(id);
    let session_loop = state.session_manager.get_session(session_id)
        .ok_or(StatusCode::NOT_FOUND)?;

    session_loop.update_name(payload.name).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let meta =  session_loop.get_session().metadata.clone();
    Ok(Json(SessionResponse {
        id: session_id,
        name: meta.name.clone(),
        metadata: meta,
    }))
}
