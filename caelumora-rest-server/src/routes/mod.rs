use crate::auth::{require_auth, require_stream_auth, AuthenticatedUser, router};
use crate::error::ApiError;
use crate::plugin::RestPlugin;
use caelumora_rest_api::channel::ChannelResponse;
use caelumora_rest_api::session::SessionSnapshot;
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
use anyhow::Error;
use axum::body::Body;
use axum::response::Response;
use caelumora_runtime::terminal::ByteStream;

mod agents;
mod channels;
mod compartments;
mod events;
mod profile;
mod sessions;
mod templates;
mod workspaces;
mod groups;

pub async fn ensure_session(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    session_id: SessionId,
) -> Result<Arc<SessionLoop>, ApiError> {
    // The running session, or loaded from the store and started - either way
    // only if this user may read it (issue 11).
    Ok(state.session_manager.open(&user.security_context(), session_id).await?)
}

/// Session `session_id` and its channel `channel_id`, as `user` may read
/// them - the one way a handler reaches a channel (issue 102). A channel the
/// user may not read is answered as absent, like one that does not exist;
/// the runtime decides that (`SessionLoop::channel_for`).
pub async fn open_channel(state: &Arc<AppState>, user: &AuthenticatedUser, session_id: SessionId, channel_id: ChannelId) -> Result<(Arc<SessionLoop>, Arc<ChannelLoop>), ApiError> {
    let session = ensure_session(state, user, session_id).await?;
    let channel = session.channel_for(&user.security_context(), channel_id).await?;
    Ok((session, channel))
}

/// Channel `channel_id` of session `session_id` as `user` sees it (issue
/// 80) - what a handler answers with a channel, with what is new in it to
/// the user (issue 404). Described by the runtime, which decides what the
/// user may read and do; does not start the session.
pub async fn channel_view(state: &Arc<AppState>, user: &AuthenticatedUser, session_id: SessionId, channel_id: ChannelId) -> Result<ChannelResponse, ApiError> {
    Ok(channel_of(state, user, session_id, channel_id).await?.snapshot(Detail::PARTICIPANTS | Detail::BRIEFING | Detail::UNREAD).await.into())
}

/// Session `session_id` as `user` sees it now, with its channels and their
/// participants, the briefings (issue 245) and what is new to the user in each
/// (issue 404) - what `GET` of one session and every change of one answer.
pub async fn session_view(state: &Arc<AppState>, user: &AuthenticatedUser, session_id: SessionId) -> Result<SessionSnapshot, ApiError> {
    let session = views(state, user).session(session_id).await?;
    Ok(session.snapshot(Detail::CHANNELS | Detail::PARTICIPANTS | Detail::BRIEFING | Detail::UNREAD).await.into())
}

/// Channel `channel_id` of session `session_id` as `user` reaches it
/// (issue 163) - what a handler reads a channel through, running or not.
pub async fn channel_of(state: &Arc<AppState>, user: &AuthenticatedUser, session_id: SessionId, channel_id: ChannelId) -> Result<ChannelView, ApiError> {
    let session = views(state, user).session(session_id).await?;
    Ok(session.channel(channel_id).await?)
}

/// The views as `user` navigates them (issue 163): what a handler reads
/// through; `open_channel` is for what it does.
pub fn views(state: &Arc<AppState>, user: &AuthenticatedUser) -> Views {
    state.session_manager.views(Viewer::new(user.security_context()))
}

/// `content` of `size` bytes, streamed, as a download named `name`: always
/// an attachment of `application/octet-stream`, never shown inline and
/// never sniffed - a file of a workspace or a plugin's content, such as an
/// artifact's, is anybody's content, and HTML or SVG would otherwise run in
/// the web UI's origin (issues 208, 253, 271) -, and not cached.
pub fn attachment_response(size: u64, content: ByteStream, name: &str) -> Response {
    use axum::http::{header, HeaderValue};
    use axum::response::IntoResponse;
    let mut response = Body::from_stream(content).into_response();
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
/// the error answers itself.
pub fn change_error(error: Error, code: &'static str) -> ApiError {
    match error.downcast_ref::<StoreError>() {
        Some(StoreError::Validation(reason)) => ApiError::new(StatusCode::BAD_REQUEST, code, reason.clone()),
        _ => error.into(),
    }
}

/// The routes of the API: the core's and those of `plugins`, as the API
/// serves them below the path the application nests them at (issue 323).
/// Everything except `/auth/login` and `/auth/logout` requires a session
/// (see `crate::auth`), the plugins' routes too.
pub fn api(state: Arc<AppState>, plugins: &[Box<dyn RestPlugin>]) -> Router {
    let protected = Router::new()
        .route("/templates", get(templates::list_templates))
        .route("/compartments", get(compartments::list_compartments))
        .route("/profile", get(profile::get_profile).put(profile::update_profile))
        .route("/profile/approval-rules", post(profile::add_approval_rule))
        .route("/profile/approval-rules/{id}", axum::routing::delete(profile::remove_approval_rule))
        .route("/agents", get(agents::list_agents).post(agents::create_agent))
        .route("/agents/{id}", get(agents::get_agent).patch(agents::rename_agent).delete(agents::delete_agent))
        .route("/agents/{id}/restore", post(agents::restore_agent))
        .route("/agents/{id}/purge", post(agents::purge_agent))
        .route("/personalities", get(agents::list_personalities))
        .route("/groups", get(groups::list_groups).post(groups::create_group))
        .route("/groups/{id}", get(groups::get_group).patch(groups::rename_group).delete(groups::delete_group))
        .route("/groups/{id}/members", axum::routing::put(groups::set_members))
        .route("/groups/{id}/restore", post(groups::restore_group))
        .route("/groups/{id}/purge", post(groups::purge_group))
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
        .route("/sessions", get(sessions::list_sessions).post(sessions::create_session))
        .route(
            "/sessions/{id}",
            get(sessions::get_session).delete(sessions::delete_session).patch(sessions::rename_session),
        )
        .route("/sessions/{id}/privacy", axum::routing::put(sessions::update_session_privacy))
        .route("/sessions/{id}/language", axum::routing::put(sessions::set_session_language))
        .route("/sessions/{id}/time-zone", axum::routing::put(sessions::set_session_time_zone))
        .route("/sessions/{id}/participants", post(sessions::add_participant))
        .route("/sessions/{id}/join", post(sessions::join_session))
        .route("/sessions/{id}/candidates", get(sessions::candidates))
        .route("/sessions/{id}/approval-rules", axum::routing::put(sessions::update_session_approval_rules))
        .route("/sessions/{id}/description", axum::routing::put(sessions::update_session_description))
        .route("/sessions/{id}/restore", post(sessions::restore_session))
        .route("/sessions/{id}/purge", post(sessions::purge_session))
        .route("/sessions/{id}/workspace", axum::routing::put(workspaces::attach_workspace).delete(workspaces::detach_workspace))
        .route("/sessions/{id}/actors/{aid}/private_notes", get(sessions::agent_private_notes))
        .route("/sessions/{id}/channels", get(channels::list_channels).post(channels::create_channel))
        .route("/sessions/{id}/channels/{cid}", get(channels::get_channel).delete(channels::delete_channel))
        .route("/sessions/{id}/channels/{cid}/history", get(channels::get_channel_history))
        .route("/sessions/{id}/channels/{cid}/read", axum::routing::put(channels::mark_read))
        .route("/sessions/{id}/channels/{cid}/messages", axum::routing::post(channels::send_message))
        .route("/sessions/{id}/channels/{cid}/classification", axum::routing::put(channels::reclassify_channel))
        .route("/sessions/{id}/channels/{cid}/language", axum::routing::put(channels::set_channel_language))
        .route("/sessions/{id}/channels/{cid}/approval-rules", axum::routing::put(channels::update_channel_approval_rules))
        .route("/sessions/{id}/channels/{cid}/description", axum::routing::put(channels::update_channel_description))
        .route("/sessions/{id}/channels/{cid}/restore", post(channels::restore_channel))
        .route("/sessions/{id}/channels/{cid}/uses-workspace", axum::routing::put(workspaces::set_uses_workspace))
        .route("/sessions/{id}/channels/{cid}/commands/{command_id}/kill", post(workspaces::kill_command))
        .route(
            "/sessions/{id}/channels/{cid}/messages/{mid}",
            get(channels::get_message).put(channels::modify_message).delete(channels::revoke_message),
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
        .route("/sessions/{id}/channels/{cid}/participants/{aid}/tool-calls/{call_id}/cancel", post(channels::interrupt_tool_call))
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

    let mut api = router(Arc::clone(&state.auth)).merge(protected).merge(streams);
    for plugin in plugins {
        let routes = plugin.routes(Arc::clone(&state))
            .route_layer(axum::middleware::from_fn_with_state(Arc::clone(&state.auth), require_auth));
        api = api.merge(routes);
    }
    api
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::auth::{Auth, TokenService, SESSION_COOKIE};
    use axum::body::Body;
    use axum::http::header::{COOKIE, SET_COOKIE};
    use axum::http::Request;
    use caelumora_model::config::AuthConfig;
    use caelumora_model::user_provider::UserManager;
    use caelumora_model::user_provider::accept_any::AcceptAnyUserProvider;
    use caelumora_runtime::event::EventBus;
    use caelumora_runtime::plugin::Plugin;
    use caelumora_runtime::runtime::RuntimeBuilder;
    use caelumora_runtime::session::SessionManager;
    use caelumora_runtime::testing::TestRuntime;
    use tower::ServiceExt;
    use anyhow::Result;

    /// A plugin answering who asks.
    struct Hello;

    impl Plugin for Hello {
        fn name(&self) -> &'static str {
            "hello"
        }

        fn register(&self, _runtime: &mut RuntimeBuilder) -> Result<()> {
            Ok(())
        }
    }

    impl RestPlugin for Hello {
        fn routes(&self, _state: Arc<AppState>) -> Router {
            Router::new().route("/hello", get(|user: AuthenticatedUser| async move { user.security_context().caller_id.to_string() }))
        }
    }

    /// The API with `plugins`, which lets anybody log in.
    fn api_with(plugins: &[Box<dyn RestPlugin>]) -> Router {
        api(state(), plugins)
    }

    /// The state of an API that lets anybody log in and permits everything,
    /// on a runtime of memory stores.
    pub(crate) fn state() -> Arc<AppState> {
        let runtime = TestRuntime { plugins: Vec::new(), ..Default::default() }.build();
        let users = Arc::clone(&runtime.entity_stores().users);
        let auth = Arc::new(Auth {
            tokens: TokenService::new(b"0123456789abcdef0123456789abcdef", &AuthConfig::default()).unwrap(),
            users: Arc::new(UserManager::new(vec![Arc::new(AcceptAnyUserProvider::new(Arc::clone(&users)))], users, Arc::<EventBus>::clone(runtime.event_bus()))),
            controllers: Arc::clone(runtime.controller_manager()),
        });
        let session_manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        Arc::new(AppState { auth, runtime_manager: runtime, session_manager, compartments: Default::default(), shutdown: Default::default() })
    }

    /// The cookie `api` answers `username` logging in with.
    pub(crate) async fn login(api: &Router, username: &str) -> String {
        let body = format!(r#"{{"username":"{username}","password":"x"}}"#);
        let login = Request::post("/auth/login").header("content-type", "application/json").body(Body::from(body)).unwrap();
        let logged_in = api.clone().oneshot(login).await.unwrap();
        logged_in.headers().get(SET_COOKIE).and_then(|v| v.to_str().ok()).and_then(|v| v.split(';').next()).unwrap().to_string()
    }

    #[tokio::test]
    async fn a_plugins_routes_are_served_behind_the_login() {
        let api = api_with(&[Box::new(Hello)]);
        let anonymous = api.clone().oneshot(Request::get("/hello").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

        let cookie = login(&api, "alice").await;
        assert!(cookie.starts_with(SESSION_COOKIE), "{cookie}");
        let asked = api.oneshot(Request::get("/hello").header(COOKIE, cookie).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(asked.status(), StatusCode::OK);
    }

    #[test]
    fn a_download_is_named_plainly_and_as_it_is() {
        assert_eq!(attachment("report.md"), "attachment; filename=\"report.md\"; filename*=UTF-8''report%2Emd");
        let named = attachment("Bericht \"Q3\" ü.pdf");
        assert!(named.starts_with("attachment; filename=\"Bericht__Q3___.pdf\";"), "{named}");
        assert!(named.ends_with("filename*=UTF-8''Bericht%20%22Q3%22%20%C3%BC%2Epdf"), "{named}");
    }
}
