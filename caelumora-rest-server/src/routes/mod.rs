use crate::auth::{require_auth, AuthenticatedUser};
use crate::error::ApiError;
use crate::state::AppState;
use axum::{
    routing::{delete, get, post},
    Router,
};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_runtime::channel::ChannelLoop;
use caelumora_runtime::session::SessionLoop;
use std::sync::Arc;

mod channels;
mod events;
mod sessions;
mod templates;

pub async fn ensure_session(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    session_id: SessionId,
) -> Result<Arc<SessionLoop>, ApiError> {
    if let Some(session_loop) = state.session_manager.get_session(session_id) {
        return Ok(session_loop);
    }

    let security_context = user.security_context();

    // Not currently running: load it from the store and start it. `load_session` owns
    // spawning the run loop and emitting the `SessionStarted` event.
    state
        .session_manager
        .load_session(session_id, &security_context)
        .await
        .map_err(|e| {
            // Only a missing session is "not found"; a session that exists but
            // cannot start (say, a broken component) is a server error.
            if caelumora_runtime::error::is_not_found(&e, "session") {
                ApiError::not_found("session_not_found", format!("Session {} does not exist", session_id))
            } else {
                ApiError::internal("session_load_failed", &format!("loading session {}", session_id), &e)
            }
        })
}

/// The channel `channel_id` of `session`, or a `404`.
pub fn channel_of(session: &SessionLoop, channel_id: ChannelId) -> Result<Arc<ChannelLoop>, ApiError> {
    session.channel_by_id(channel_id)
        .ok_or_else(|| ApiError::not_found("channel_not_found", format!("Channel {} does not exist in this session", channel_id)))
}

/// All routes. Everything except `/auth/login` and `/auth/logout` requires a
/// session (see `crate::auth`).
pub fn create_router(state: Arc<AppState>) -> Router {
    let protected = Router::new()
        .route("/templates", get(templates::list_templates))
        .route("/sessions", get(sessions::list_sessions).post(sessions::create_session))
        .route(
            "/sessions/{id}",
            delete(sessions::delete_session).patch(sessions::rename_session),
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
        .route_layer(axum::middleware::from_fn_with_state(state.auth.clone(), require_auth))
        .with_state(state.clone());

    crate::auth::router(state.auth.clone()).merge(protected)
}
