use crate::auth::{require_auth, require_stream_auth, AuthenticatedUser};
use crate::error::{ApiError, Subject};
use crate::models::ChannelResponse;
use crate::state::AppState;
use axum::{
    routing::{get, post},
    Router,
};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_runtime::channel::ChannelLoop;
use caelumora_runtime::session::SessionLoop;
use caelumora_runtime::view::{Viewer, Views};
use std::sync::Arc;

mod channels;
mod compartments;
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

/// Session `session_id` and its channel `channel_id`, as `user` may read
/// them - the one way a handler reaches a channel (issue 102). A channel the
/// user may not read is answered as absent, like one that does not exist;
/// the runtime decides that (`SessionLoop::channel_for`).
pub async fn open_channel(state: &Arc<AppState>, user: &AuthenticatedUser, session_id: SessionId, channel_id: ChannelId) -> Result<(Arc<SessionLoop>, Arc<ChannelLoop>), ApiError> {
    let session = ensure_session(state, user, session_id).await?;
    let channel = session.channel_for(&user.security_context(), channel_id).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, &format!("opening channel {channel_id}")))?;
    Ok((session, channel))
}

/// Channel `channel_id` of session `session_id` as `user` sees it (issue
/// 80) - what a handler answers with a channel. Described by the runtime,
/// which decides what the user may read and do; does not start the session.
pub async fn channel_view(state: &Arc<AppState>, user: &AuthenticatedUser, session_id: SessionId, channel_id: ChannelId) -> Result<ChannelResponse, ApiError> {
    let describing = || format!("describing channel {channel_id}");
    let session = views(state, user).session(session_id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Session, &describing()))?;
    let channel = session.channel(channel_id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Session, &describing()))?;
    Ok(channel.snapshot().await.into())
}

/// The views as `user` navigates them (issue 163): what a handler reads
/// through; `open_channel` is for what it does.
pub fn views(state: &Arc<AppState>, user: &AuthenticatedUser) -> Views {
    state.session_manager.views(Viewer::new(user.security_context()))
}

/// All routes. Everything except `/auth/login` and `/auth/logout` requires a
/// session (see `crate::auth`).
pub fn create_router(state: Arc<AppState>) -> Router {
    let protected = Router::new()
        .route("/templates", get(templates::list_templates))
        .route("/compartments", get(compartments::list_compartments))
        .route("/sessions", get(sessions::list_sessions).post(sessions::create_session))
        .route(
            "/sessions/{id}",
            get(sessions::get_session).delete(sessions::delete_session).patch(sessions::rename_session),
        )
        .route("/sessions/{id}/privacy", axum::routing::put(sessions::update_session_privacy))
        .route("/sessions/{id}/channels", get(channels::list_channels).post(channels::create_channel))
        .route("/sessions/{id}/channels/{cid}", get(channels::get_channel).delete(channels::delete_channel))
        .route(
            "/sessions/{id}/channels/{cid}/messages",
            get(channels::get_channel_messages).post(channels::send_message),
        )
        .route("/sessions/{id}/channels/{cid}/classification", axum::routing::put(channels::reclassify_channel))
        .route("/sessions/{id}/channels/{cid}/messages/{mid}/classification", axum::routing::put(channels::reclassify_message))
        .route("/sessions/{id}/channels/{cid}/approvals", post(channels::send_approval))
        .route("/sessions/{id}/channels/{cid}/cancel", post(channels::cancel_channel))
        .route("/sessions/{id}/channels/{cid}/notes", get(channels::get_notes))
        .route("/sessions/{id}/channels/{cid}/participants", post(channels::add_participant))
        .route("/sessions/{id}/channels/{cid}/participants/{aid}", axum::routing::delete(channels::remove_participant))
        .route(
            "/sessions/{id}/channels/{cid}/actors/{aid}/messages/{mid}/context",
            get(channels::message_context),
        )
        .route("/sessions/{id}/channels/{cid}/actors/{aid}/given-up-turns", get(channels::given_up_turns))
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
