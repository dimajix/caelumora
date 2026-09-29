use crate::auth::{require_auth, require_stream_auth, AuthenticatedUser};
use crate::error::{ApiError, Subject};
use crate::state::AppState;
use caelumora_model::security::AccessQuestions;
use axum::{
    routing::{get, post},
    Router,
};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_runtime::channel::ChannelLoop;
use caelumora_runtime::session::SessionLoop;
use std::sync::Arc;

pub mod channels;
mod events;
mod sessions;
mod templates;

pub async fn ensure_session(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    session_id: SessionId,
) -> Result<Arc<SessionLoop>, ApiError> {
    // The running session, or loaded from the store and started - either way
    // only if this user may read it (issue 11).
    state
        .session_manager
        .open_session(&user.security_context(), session_id)
        .await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, &format!("opening session {session_id}")))
}

/// The channel `channel_id` of `session`, if `viewer` may read it (issue 70):
/// one they may not read is reported as absent, like one that does not
/// exist, so that channels cannot be probed.
pub async fn channel_of(state: &AppState, session: &SessionLoop, channel_id: ChannelId, viewer: &caelumora_model::security::SecurityContext) -> Result<Arc<ChannelLoop>, ApiError> {
    let not_found = || ApiError::not_found("channel_not_found", format!("Channel {} does not exist in this session", channel_id));
    let channel = session.channel_by_id(channel_id).ok_or_else(not_found)?;
    if may_read_channel(state, &channel, viewer).await { Ok(channel) } else { Err(not_found()) }
}

/// Whether `viewer` may read the running `channel` (issue 70).
pub async fn may_read_channel(state: &AppState, channel: &ChannelLoop, viewer: &caelumora_model::security::SecurityContext) -> bool {
    state.runtime_manager.security_manager().may_read_channel(viewer, &channel.security_attributes()).await
}

/// All routes. Everything except `/auth/login` and `/auth/logout` requires a
/// session (see `crate::auth`).
pub fn create_router(state: Arc<AppState>) -> Router {
    let protected = Router::new()
        .route("/templates", get(templates::list_templates))
        .route("/sessions", get(sessions::list_sessions).post(sessions::create_session))
        .route(
            "/sessions/{id}",
            get(sessions::get_session).delete(sessions::delete_session).patch(sessions::rename_session),
        )
        .route("/sessions/{id}/channels", get(channels::list_channels).post(channels::create_channel))
        .route("/sessions/{id}/channels/{cid}", get(channels::get_channel).delete(channels::delete_channel))
        .route(
            "/sessions/{id}/channels/{cid}/messages",
            get(channels::get_channel_messages).post(channels::send_message),
        )
        .route("/sessions/{id}/channels/{cid}/approvals", post(channels::send_approval))
        .route("/sessions/{id}/channels/{cid}/cancel", post(channels::cancel_channel))
        .route("/sessions/{id}/channels/{cid}/notes", get(channels::get_notes))
        .route("/sessions/{id}/channels/{cid}/participants", post(channels::add_participant))
        .route("/sessions/{id}/channels/{cid}/participants/{aid}", axum::routing::delete(channels::remove_participant))
        .route(
            "/sessions/{id}/channels/{cid}/actors/{aid}/messages/{mid}/context",
            get(channels::message_context),
        )
        .route(
            "/sessions/{id}/channels/{cid}/commands",
            get(channels::list_commands).post(channels::execute_command),
        )
        .route_layer(axum::middleware::from_fn_with_state(state.auth.clone(), require_auth))
        .with_state(state.clone());

    // The event streams authenticate like everything else, but do not renew
    // the session they run on - see `require_stream_auth` (issue 33).
    let streams = Router::new()
        .route("/sessions/{id}/channels/{cid}/events", get(channels::join_channel))
        .route("/events", get(events::global_events))
        .route_layer(axum::middleware::from_fn_with_state(state.auth.clone(), require_stream_auth))
        .with_state(state.clone());

    crate::auth::router(state.auth.clone()).merge(protected).merge(streams)
}
