use std::sync::Arc;
use axum::{extract::{Path, State}, http::StatusCode, Json};
use uuid::Uuid;
use caelumora_model::session::{SessionContext, SessionId};
use caelumora_model::template::SessionTemplateId;
use caelumora_model::user::UserId;
use crate::state::AppState;
use crate::models::{CreateSessionRequest, SessionResponse, RenameSessionRequest};

pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateSessionRequest>,
) -> Result<Json<SessionResponse>, StatusCode> {
    let template_id = SessionTemplateId(payload.template_id);
    let template = state.template_store.list_session_templates()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .into_iter()
        .find(|t| t.id == template_id)
        .ok_or(StatusCode::NOT_FOUND)?;

    let session_id = SessionId(Uuid::new_v4());
    let owner = UserId::default();
    let context = SessionContext {
        id: session_id,
        owner,
        state_store: state.state_store.clone(),
        template_store: state.template_store.clone(),
    };

    let name = payload.name.unwrap_or_else(|| template.metadata.name.clone());
    let session = template.instantiate(name, &context)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let session_loop = state.session_manager.create_session(session)
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

pub async fn list_sessions(State(state): State<Arc<AppState>>) -> Json<Vec<SessionResponse>> {
    Json(
        state.session_manager.sessions
            .iter()
            .map(|s| {
                let meta = s.get_session().metadata.clone();
                SessionResponse {
                    id: s.id(),
                    name: meta.name.clone(),
                    metadata: meta,
                }
            })
            .collect(),
    )
}

pub async fn terminate_session(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>) -> StatusCode {
    let session_id = SessionId(id);
    if let Some((_, handle)) = state.running_sessions.remove(&session_id) {
        handle.abort();
    }
    
    if state.session_manager.sessions.remove(&session_id).is_some() {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
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
