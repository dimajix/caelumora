use crate::state::AppState;
use axum::{
    http::StatusCode,
    routing::{delete, get, post},
    Router,
};
use caelumora_model::actor::ActorId;
use caelumora_model::channel::ChannelId;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::SessionId;
use std::sync::Arc;

mod channels;
mod events;
mod sessions;
mod templates;

pub async fn ensure_session(
    state: &Arc<AppState>,
    session_id: SessionId,
) -> Result<Arc<caelumora_runtime::session::SessionLoop>, StatusCode> {
    if let Some(session_loop) = state.session_manager.get_session(session_id) {
        return Ok(session_loop);
    }

    let security_context = SecurityContext::new(ActorId::User(state.user_id), SessionId::default(), ChannelId::default());

    // Try to load the session
    let session_loop = state
        .session_manager
        .load_session(session_id, &security_context)
        .await
        .map_err(|e| {
            tracing::error!("Failed to load session {}: {}", session_id, e);
            StatusCode::NOT_FOUND
        })?;

    let session_loop_clone = session_loop.clone();
    let handle = tokio::spawn(async move {
        if let Err(e) = session_loop_clone.run().await {
            tracing::error!("Session loop for {} failed: {}", session_id, e);
        }
    });

    state.running_sessions.insert(session_id, handle);

    Ok(session_loop)
}

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/templates", get(templates::list_templates))
        .route("/sessions", get(sessions::list_sessions).post(sessions::create_session))
        .route(
            "/sessions/{id}",
            delete(sessions::terminate_session).patch(sessions::rename_session),
        )
        .route("/sessions/{id}/channels", get(channels::list_channels))
        .route("/sessions/{id}/channels/{cid}", get(channels::get_channel))
        .route(
            "/sessions/{id}/channels/{cid}/messages",
            get(channels::get_channel_messages).post(channels::send_message),
        )
        .route("/sessions/{id}/channels/{cid}/approvals", post(channels::send_approval))
        .route("/sessions/{id}/channels/{cid}/cancel", post(channels::cancel_channel))
        .route("/sessions/{id}/channels/{cid}/events", get(channels::join_channel))
        .route("/events", get(events::global_events))
        .with_state(state)
}
