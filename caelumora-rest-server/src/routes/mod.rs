use crate::auth::{require_auth, require_stream_auth, AuthenticatedUser};
use crate::error::{ApiError, Subject};
use crate::models::{ChannelResponse, SessionSnapshot};
use crate::state::AppState;
use axum::{
    routing::{get, post},
    Router,
};
use caelumora_model::snapshot::Detail;
use caelumora_model::store::StoreError;
use axum::http::StatusCode;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_runtime::channel::ChannelLoop;
use caelumora_runtime::session::SessionLoop;
use caelumora_runtime::view::{ChannelView, Viewer, Views};
use std::sync::Arc;

mod agents;
mod artifacts;
mod channels;
mod compartments;
mod events;
mod profile;
mod sessions;
mod templates;
mod workspaces;
mod skills;

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
    Ok(channel_of(state, user, session_id, channel_id).await?.snapshot(Detail::MEMBERS | Detail::BRIEFING).await.into())
}

/// Session `session_id` as `user` sees it now, with its channels and their
/// members and the briefings (issue 245) - what `GET` of one session and
/// every change of one answer.
pub async fn session_view(state: &Arc<AppState>, user: &AuthenticatedUser, session_id: SessionId) -> Result<SessionSnapshot, ApiError> {
    let session = views(state, user).session(session_id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Session, &format!("loading session {session_id}")))?;
    Ok(session.snapshot(Detail::CHANNELS | Detail::MEMBERS | Detail::BRIEFING).await.into())
}

/// Channel `channel_id` of session `session_id` as `user` reaches it
/// (issue 163) - what a handler reads a channel through, running or not.
pub async fn channel_of(state: &Arc<AppState>, user: &AuthenticatedUser, session_id: SessionId, channel_id: ChannelId) -> Result<ChannelView, ApiError> {
    let reaching = || format!("reaching channel {channel_id}");
    let session = views(state, user).session(session_id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Session, &reaching()))?;
    session.channel(channel_id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Session, &reaching()))
}

/// The views as `user` navigates them (issue 163): what a handler reads
/// through; `open_channel` is for what it does.
pub fn views(state: &Arc<AppState>, user: &AuthenticatedUser) -> Views {
    state.session_manager.views(Viewer::new(user.security_context()))
}

/// `content` of `size` bytes, streamed, as a download named `name`: always
/// an attachment of `application/octet-stream`, never shown inline and
/// never sniffed - a file of a workspace or an artifact is anybody's
/// content, and HTML or SVG would otherwise run in the web UI's origin
/// (issues 208, 253, 271) -, and not cached.
pub(crate) fn attachment_response(size: u64, content: caelumora_runtime::terminal::ByteStream, name: &str) -> axum::response::Response {
    use axum::http::{header, HeaderValue};
    use axum::response::IntoResponse;
    let mut response = axum::body::Body::from_stream(content).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(disposition) = HeaderValue::from_str(&attachment(name)) {
        headers.insert(header::CONTENT_DISPOSITION, disposition);
    }
    response
}

/// `Content-Disposition` of a download named `name` (RFC 6266): the name
/// as it is, encoded, and plain for the clients that read only that.
fn attachment(name: &str) -> String {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    let plain: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
    format!("attachment; filename=\"{plain}\"; filename*=UTF-8''{}", utf8_percent_encode(name, NON_ALPHANUMERIC))
}

/// What changing a place failed with, as the API answers it: `400` with
/// `code` - `invalid_rule` for an approval rule the place may not keep
/// (issue 13) - and why, for what the place refused to keep; otherwise as
/// `from_runtime` answers.
pub fn change_error(error: anyhow::Error, code: &'static str, subject: Subject, action: &str) -> ApiError {
    match error.downcast_ref::<StoreError>() {
        Some(StoreError::Validation(reason)) => ApiError::new(StatusCode::BAD_REQUEST, code, reason.clone()),
        _ => ApiError::from_runtime(error, subject, action),
    }
}

/// All routes. Everything except `/auth/login` and `/auth/logout` requires a
/// session (see `crate::auth`).
pub fn create_router(state: Arc<AppState>) -> Router {
    let protected = Router::new()
        .route("/templates", get(templates::list_templates))
        .route("/compartments", get(compartments::list_compartments))
        .route("/profile", get(profile::get_profile).put(profile::update_profile))
        .route("/profile/approval-rules", post(profile::add_approval_rule))
        .route("/profile/approval-rules/{id}", axum::routing::delete(profile::remove_approval_rule))
        .route("/agents", get(agents::list_agents))
        .route("/agents/{id}", axum::routing::delete(agents::delete_agent))
        .route("/agents/{id}/restore", post(agents::restore_agent))
        .route("/agents/{id}/purge", post(agents::purge_agent))
        .route("/skills", get(skills::list_skills).post(skills::create_skill))
        .route("/skills/{id}", get(skills::get_skill).patch(skills::rename_skill).delete(skills::delete_skill))
        .route("/skills/{id}/description", axum::routing::put(skills::describe_skill))
        .route("/skills/{id}/body", axum::routing::put(skills::rewrite_skill))
        .route("/skills/{id}/classification", axum::routing::put(skills::reclassify_skill))
        .route("/skills/{id}/restore", post(skills::restore_skill))
        .route("/skills/{id}/purge", post(skills::purge_skill))
        .route("/workspaces", get(workspaces::list_workspaces).post(workspaces::create_workspace))
        .route("/workspaces/storages", get(workspaces::list_storages))
        .route("/workspaces/{id}", get(workspaces::get_workspace).patch(workspaces::rename_workspace).delete(workspaces::delete_workspace))
        .route("/workspaces/{id}/description", axum::routing::put(workspaces::describe_workspace))
        .route("/workspaces/{id}/classification", axum::routing::put(workspaces::reclassify_workspace))
        .route("/workspaces/{id}/restore", post(workspaces::restore_workspace))
        .route("/workspaces/{id}/purge", post(workspaces::purge_workspace))
        .route("/workspaces/{id}/access", axum::routing::put(workspaces::set_access))
        .route("/workspaces/{id}/files", get(workspaces::files))
        .route(
            "/workspaces/{id}/files/{*path}",
            get(workspaces::download_file).put(workspaces::upload_file).delete(workspaces::delete_file),
        )
        .route("/artifacts", get(artifacts::list_artifacts))
        .route("/artifacts/{id}", get(artifacts::get_artifact).delete(artifacts::delete_artifact))
        .route("/artifacts/{id}/scope", axum::routing::put(artifacts::keep_artifact))
        .route("/artifacts/{id}/description", axum::routing::put(artifacts::describe_artifact))
        .route("/artifacts/{id}/restore", post(artifacts::restore_artifact))
        .route("/artifacts/{id}/purge", post(artifacts::purge_artifact))
        .route("/artifacts/{id}/access", axum::routing::put(artifacts::set_access))
        .route("/artifacts/{id}/versions/{n}/content", get(artifacts::download_version))
        .route("/sessions", get(sessions::list_sessions).post(sessions::create_session))
        .route(
            "/sessions/{id}",
            get(sessions::get_session).delete(sessions::delete_session).patch(sessions::rename_session),
        )
        .route("/sessions/{id}/privacy", axum::routing::put(sessions::update_session_privacy))
        .route("/sessions/{id}/approval-rules", axum::routing::put(sessions::update_session_approval_rules))
        .route("/sessions/{id}/description", axum::routing::put(sessions::update_session_description))
        .route("/sessions/{id}/restore", post(sessions::restore_session))
        .route("/sessions/{id}/purge", post(sessions::purge_session))
        .route("/sessions/{id}/workspace", axum::routing::put(workspaces::attach_workspace).delete(workspaces::detach_workspace))
        .route("/sessions/{id}/actors/{aid}/private_notes", get(sessions::agent_private_notes))
        .route("/sessions/{id}/channels", get(channels::list_channels).post(channels::create_channel))
        .route("/sessions/{id}/channels/{cid}", get(channels::get_channel).delete(channels::delete_channel))
        .route("/sessions/{id}/channels/{cid}/history", get(channels::get_channel_history))
        .route("/sessions/{id}/channels/{cid}/messages", axum::routing::post(channels::send_message))
        .route("/sessions/{id}/channels/{cid}/classification", axum::routing::put(channels::reclassify_channel))
        .route("/sessions/{id}/channels/{cid}/language", axum::routing::put(channels::set_channel_language))
        .route("/sessions/{id}/channels/{cid}/approval-rules", axum::routing::put(channels::update_channel_approval_rules))
        .route("/sessions/{id}/channels/{cid}/description", axum::routing::put(channels::update_channel_description))
        .route("/sessions/{id}/channels/{cid}/restore", post(channels::restore_channel))
        .route("/sessions/{id}/channels/{cid}/uses-workspace", axum::routing::put(workspaces::set_uses_workspace))
        .route("/sessions/{id}/channels/{cid}/artifacts", get(artifacts::channel_artifacts))
        .route(
            "/sessions/{id}/channels/{cid}/messages/{mid}",
            axum::routing::put(channels::modify_message).delete(channels::revoke_message),
        )
        .route("/sessions/{id}/channels/{cid}/messages/{mid}/classification", axum::routing::put(channels::reclassify_message))
        .route("/sessions/{id}/channels/{cid}/messages/{mid}/retry", post(channels::retry_message))
        .route("/sessions/{id}/channels/{cid}/approvals", post(channels::send_approval))
        .route("/sessions/{id}/channels/{cid}/questions/{qid}", post(channels::answer_questions))
        .route("/sessions/{id}/channels/{cid}/cancel", post(channels::cancel_channel))
        .route("/sessions/{id}/channels/{cid}/notes", get(channels::get_notes))
        .route("/sessions/{id}/channels/{cid}/participants", post(channels::add_participant))
        .route("/sessions/{id}/channels/{cid}/participants/{aid}", get(channels::get_participant).delete(channels::remove_participant))
        .route("/sessions/{id}/channels/{cid}/participants/{aid}/cancel", post(channels::cancel_participant))
        .route("/sessions/{id}/channels/{cid}/participants/{aid}/retry", post(channels::retry_turn))
        .route(
            "/sessions/{id}/channels/{cid}/actors/{aid}/messages/{mid}/context",
            get(channels::message_context),
        )
        .route("/sessions/{id}/channels/{cid}/actors/{aid}/history", get(channels::agent_history))
        .route("/sessions/{id}/channels/{cid}/actors/{aid}/tools", get(channels::agent_tools))
        .route("/sessions/{id}/channels/{cid}/actors/{aid}/components", get(channels::agent_components))
        .route(
            "/sessions/{id}/channels/{cid}/commands",
            get(channels::list_commands).post(channels::execute_command),
        )
        .route_layer(axum::middleware::from_fn_with_state(Arc::clone(&state.auth), require_auth))
        .with_state(Arc::clone(&state));

    // The event streams authenticate like everything else, but do not renew
    // the session they run on - see `require_stream_auth` (issue 33).
    let streams = Router::new()
        .route("/sessions/{id}/channels/{cid}/events", get(channels::join_channel))
        .route("/events", get(events::global_events))
        .route_layer(axum::middleware::from_fn_with_state(Arc::clone(&state.auth), require_stream_auth))
        .with_state(Arc::clone(&state));

    crate::auth::router(Arc::clone(&state.auth)).merge(protected).merge(streams)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_is_named_plainly_and_as_it_is() {
        assert_eq!(attachment("report.md"), "attachment; filename=\"report.md\"; filename*=UTF-8''report%2Emd");
        let named = attachment("Bericht \"Q3\" ü.pdf");
        assert!(named.starts_with("attachment; filename=\"Bericht__Q3___.pdf\";"), "{named}");
        assert!(named.ends_with("filename*=UTF-8''Bericht%20%22Q3%22%20%C3%BC%2Epdf"), "{named}");
    }
}
