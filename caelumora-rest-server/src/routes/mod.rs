use std::sync::Arc;
use axum::{
    routing::{delete, get, post},
    Router,
};
use crate::state::AppState;

mod channels;
mod events;
mod sessions;
mod templates;

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
        .route("/sessions/{id}/channels/{cid}/cancel", post(channels::cancel_channel))
        .route("/sessions/{id}/channels/{cid}/events", get(events::join_channel))
        .route("/events", get(events::global_events))
        .with_state(state)
}
