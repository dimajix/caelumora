//! The routes of the API (issue 323): the core's, behind the login, with the plugins', and what
//! the handlers share to reach the runtime as the user who asks.

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
use axum::middleware;
use axum::routing;

mod agents;
mod attachments;
mod channels;
mod compartments;
mod events;
mod profile;
mod sessions;
mod templates;
mod workspaces;
mod groups;
mod projects;

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
    content_response(size, content, name, None)
}

/// `content` of `size` bytes, streamed, named `name`: shown inline as the
/// image of the media type `image` names - png, jpeg or webp, which a
/// browser shows without running anything (`ContentRef::is_image`, issue
/// 282) -, otherwise as `attachment_response` answers; never sniffed, and
/// not cached.
pub(super) fn content_response(size: u64, content: ByteStream, name: &str, image: Option<&str>) -> Response {
    use axum::http::{header, HeaderValue};
    use axum::response::IntoResponse;
    let mut response = Body::from_stream(content).into_response();
    let headers = response.headers_mut();
    let (media_type, disposition) = match image.and_then(|image| HeaderValue::from_str(image).ok()) {
        Some(image) => (image, "inline"),
        None => (HeaderValue::from_static("application/octet-stream"), "attachment"),
    };
    headers.insert(header::CONTENT_TYPE, media_type);
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(disposition) = HeaderValue::from_str(&content_disposition(disposition, name)) {
        headers.insert(header::CONTENT_DISPOSITION, disposition);
    }
    response
}

/// `Content-Disposition` of `disposition` - `attachment` or `inline` -
/// named `name` (RFC 6266): the name as it is, encoded, and plain for the
/// clients that read only that.
fn content_disposition(disposition: &str, name: &str) -> String {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    let plain: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
    format!("{disposition}; filename=\"{plain}\"; filename*=UTF-8''{}", utf8_percent_encode(name, NON_ALPHANUMERIC))
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
        .route("/profile/approval-rules/{id}", routing::delete(profile::remove_approval_rule))
        .route("/agents", get(agents::list_agents).post(agents::create_agent))
        .route("/agents/{id}", get(agents::get_agent).patch(agents::rename_agent).delete(agents::delete_agent))
        .route("/agents/{id}/restore", post(agents::restore_agent))
        .route("/agents/{id}/purge", post(agents::purge_agent))
        .route("/personalities", get(agents::list_personalities))
        .route("/groups", get(groups::list_groups).post(groups::create_group))
        .route("/groups/{id}", get(groups::get_group).patch(groups::rename_group).delete(groups::delete_group))
        .route("/groups/{id}/members", routing::put(groups::set_members))
        .route("/groups/{id}/restore", post(groups::restore_group))
        .route("/groups/{id}/purge", post(groups::purge_group))
        .route("/projects", get(projects::list_projects).post(projects::create_project))
        .route("/projects/{id}", get(projects::get_project).patch(projects::rename_project).delete(projects::delete_project))
        .route("/projects/{id}/description", routing::put(projects::describe_project))
        .route("/projects/{id}/members", routing::put(projects::set_members))
        .route("/projects/{id}/access", routing::put(projects::set_access))
        .route("/projects/{id}/lift", post(projects::lift_project))
        .route("/projects/{id}/restore", post(projects::restore_project))
        .route("/projects/{id}/purge", post(projects::purge_project))
        .route("/workspaces", get(workspaces::list_workspaces).post(workspaces::create_workspace))
        .route("/workspaces/storages", get(workspaces::list_storages))
        .route("/workspaces/{id}", get(workspaces::get_workspace).patch(workspaces::rename_workspace).delete(workspaces::delete_workspace))
        .route("/workspaces/{id}/description", routing::put(workspaces::describe_workspace))
        .route("/workspaces/{id}/classification", routing::put(workspaces::reclassify_workspace))
        .route("/workspaces/{id}/restore", post(workspaces::restore_workspace))
        .route("/workspaces/{id}/purge", post(workspaces::purge_workspace))
        .route("/workspaces/{id}/access", routing::put(workspaces::set_access))
        .route("/workspaces/{id}/files", get(workspaces::files))
        .route(
            "/workspaces/{id}/files/{*path}",
            get(workspaces::download_file).put(workspaces::upload_file).delete(workspaces::delete_file),
        )
        .merge(session_routes())
        .route_layer(middleware::from_fn_with_state(Arc::clone(&state.auth), require_auth))
        .with_state(Arc::clone(&state));

    // The event streams authenticate like everything else, but do not renew
    // the session they run on - see `require_stream_auth` (issue 33).
    let streams = Router::new()
        .route("/sessions/{id}/channels/{cid}/events", get(channels::join_channel))
        .route("/events", get(events::global_events))
        .route_layer(middleware::from_fn_with_state(Arc::clone(&state.auth), require_stream_auth))
        .with_state(Arc::clone(&state));

    let mut api = router(Arc::clone(&state.auth)).merge(protected).merge(streams);
    for plugin in plugins {
        let routes = plugin.routes(Arc::clone(&state))
            .route_layer(middleware::from_fn_with_state(Arc::clone(&state.auth), require_auth));
        api = api.merge(routes);
    }
    api
}

/// The routes of sessions, their channels and what is reached through them - the participants,
/// the messages, the commands -, which `api` serves behind the login.
fn session_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sessions", get(sessions::list_sessions).post(sessions::create_session))
        .route(
            "/sessions/{id}",
            get(sessions::get_session).delete(sessions::delete_session).patch(sessions::rename_session),
        )
        .route("/sessions/{id}/privacy", routing::put(sessions::update_session_privacy))
        .route("/sessions/{id}/language", routing::put(sessions::set_session_language))
        .route("/sessions/{id}/time-zone", routing::put(sessions::set_session_time_zone))
        .route("/sessions/{id}/participants", post(sessions::add_participant))
        .route("/sessions/{id}/join", post(sessions::join_session))
        .route("/sessions/{id}/candidates", get(sessions::candidates))
        .route("/sessions/{id}/approval-rules", routing::put(sessions::update_session_approval_rules))
        .route("/sessions/{id}/description", routing::put(sessions::update_session_description))
        .route("/sessions/{id}/restore", post(sessions::restore_session))
        .route("/sessions/{id}/purge", post(sessions::purge_session))
        .route("/sessions/{id}/workspace", routing::put(workspaces::attach_workspace).delete(workspaces::detach_workspace))
        .route("/sessions/{id}/project", routing::put(projects::put_into_project).delete(projects::take_out_of_project))
        .route("/sessions/{id}/actors/{aid}/private_notes", get(sessions::agent_private_notes))
        .route("/sessions/{id}/channels", get(channels::list_channels).post(channels::create_channel))
        .route("/sessions/{id}/channels/{cid}", get(channels::get_channel).delete(channels::delete_channel))
        .route("/sessions/{id}/channels/{cid}/history", get(channels::get_channel_history))
        .route("/sessions/{id}/channels/{cid}/read", routing::put(channels::mark_read))
        .route("/sessions/{id}/channels/{cid}/messages", routing::post(channels::send_message))
        .route("/sessions/{id}/channels/{cid}/attachments", routing::post(attachments::upload_attachment))
        .route("/sessions/{id}/channels/{cid}/classification", routing::put(channels::reclassify_channel))
        .route("/sessions/{id}/channels/{cid}/language", routing::put(channels::set_channel_language))
        .route("/sessions/{id}/channels/{cid}/approval-rules", routing::put(channels::update_channel_approval_rules))
        .route("/sessions/{id}/channels/{cid}/description", routing::put(channels::update_channel_description))
        .route("/sessions/{id}/channels/{cid}/restore", post(channels::restore_channel))
        .route("/sessions/{id}/channels/{cid}/uses-workspace", routing::put(workspaces::set_uses_workspace))
        .route("/sessions/{id}/channels/{cid}/commands/{command_id}/kill", post(workspaces::kill_command))
        .route(
            "/sessions/{id}/channels/{cid}/messages/{mid}",
            get(channels::get_message).put(channels::modify_message).delete(channels::revoke_message),
        )
        .route("/sessions/{id}/channels/{cid}/messages/{mid}/classification", routing::put(channels::reclassify_message))
        .route("/sessions/{id}/channels/{cid}/messages/{mid}/retry", post(channels::retry_message))
        .route("/sessions/{id}/channels/{cid}/messages/{mid}/attachments/{aid}", get(attachments::download_attachment))
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
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::auth::{Auth, TokenService, SESSION_COOKIE};
    use axum::http::header::{COOKIE, SET_COOKIE};
    use axum::http::Request;
    use caelumora_model::config::{AppConfig, AuthConfig};
    use caelumora_model::user_provider::UserManager;
    use caelumora_model::user_provider::accept_any::AcceptAnyUserProvider;
    use caelumora_runtime::event::EventBus;
    use caelumora_runtime::plugin::Plugin;
    use caelumora_runtime::runtime::RuntimeBuilder;
    use caelumora_runtime::session::SessionManager;
    use caelumora_runtime::project::ProjectManager;
    use caelumora_runtime::testing::TestRuntime;
    use tower::ServiceExt;
    use axum::body::{to_bytes, Body};
    use serde_json::{json, Value};
    use uuid::Uuid;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::scope::Scope;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::template::{ChannelTemplate, ParticipantTemplate, SessionTemplate, SessionTemplateId};
    use caelumora_model::value::name::{Name, ScopedName};
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
        state_with(AppConfig::default())
    }

    /// The state `state` makes, of a runtime configured by `config`.
    pub(crate) fn state_with(config: AppConfig) -> Arc<AppState> {
        let runtime = TestRuntime { config, plugins: Vec::new(), ..Default::default() }.build();
        let users = Arc::clone(&runtime.entity_stores().users);
        let auth = Arc::new(Auth {
            tokens: TokenService::new(b"0123456789abcdef0123456789abcdef", &AuthConfig::default()).unwrap(),
            users: Arc::new(UserManager::new(vec![Arc::new(AcceptAnyUserProvider::new(Arc::clone(&users)))], users, Arc::<EventBus>::clone(runtime.event_bus()))),
            controllers: Arc::clone(runtime.controller_manager()),
        });
        let session_manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let project_manager = Arc::new(ProjectManager::new(Arc::clone(&session_manager)));
        Arc::new(AppState { auth, runtime_manager: runtime, session_manager, project_manager, compartments: Default::default(), shutdown: Default::default() })
    }

    /// The cookie `api` answers `username` logging in with.
    pub(crate) async fn login(api: &Router, username: &str) -> String {
        let body = format!(r#"{{"username":"{username}","password":"x"}}"#);
        let login = Request::post("/auth/login").header("content-type", "application/json").body(Body::from(body)).unwrap();
        let logged_in = api.clone().oneshot(login).await.unwrap();
        logged_in.headers().get(SET_COOKIE).and_then(|v| v.to_str().ok()).and_then(|v| v.split(';').next()).unwrap().to_string()
    }

    /// What `api` answers `method` on `uri` with the JSON `body` - none for `null` - for whoever
    /// is logged in by `cookie`: its status and its body as JSON, `null` for none.
    async fn send(api: &Router, cookie: &str, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
        let body = if body.is_null() { Body::empty() } else { Body::from(body.to_string()) };
        let request = Request::builder().method(method).uri(uri).header(COOKIE, cookie).header("content-type", "application/json").body(body).unwrap();
        let answer = api.clone().oneshot(request).await.unwrap();
        let status = answer.status();
        (status, serde_json::from_slice(&to_bytes(answer.into_body(), usize::MAX).await.unwrap()).unwrap_or(Value::Null))
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

    /// Issue 467: a user makes a shared project, finds, renames, describes, deletes and restores
    /// it; a session's own scope is no place to make one in.
    #[tokio::test]
    async fn the_routes_serve_projects() {
        let api = api_with(&[]);
        let cookie = login(&api, "alice").await;
        let (status, made) = send(&api, &cookie, "POST", "/projects", json!({ "name": "report", "description": "The yearly report" })).await;
        assert_eq!((status, made["scope"].clone(), made["can_change"].clone()), (StatusCode::CREATED, json!("user"), json!(true)), "{made}");
        let id = made["id"].as_str().unwrap().to_string();
        let (_, listed) = send(&api, &cookie, "GET", "/projects", Value::Null).await;
        assert_eq!(listed[0]["id"], json!(id), "{listed}");
        let (status, renamed) = send(&api, &cookie, "PATCH", &format!("/projects/{id}"), json!({ "name": "annual" })).await;
        assert_eq!((status, renamed["name"].clone()), (StatusCode::OK, json!("annual")));
        let (_, described) = send(&api, &cookie, "PUT", &format!("/projects/{id}/description"), json!({ "description": "The annual report" })).await;
        assert_eq!(described["description"], json!("The annual report"));
        let (status, refused) = send(&api, &cookie, "POST", "/projects", json!({ "name": "own", "scope": "session" })).await;
        assert_eq!((status, refused["code"].clone()), (StatusCode::BAD_REQUEST, json!("invalid_scope")));
        let (status, _) = send(&api, &cookie, "DELETE", &format!("/projects/{id}"), Value::Null).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, deleted) = send(&api, &cookie, "GET", "/projects?deleted=true", Value::Null).await;
        assert_eq!(deleted[0]["id"], json!(id), "{deleted}");
        let (status, restored) = send(&api, &cookie, "POST", &format!("/projects/{id}/restore"), Value::Null).await;
        assert_eq!((status, restored["name"].clone()), (StatusCode::OK, json!("annual")));
        let (status, missing) = send(&api, &cookie, "GET", &format!("/projects/{}", Uuid::new_v4()), Value::Null).await;
        assert_eq!((status, missing["code"].clone()), (StatusCode::NOT_FOUND, json!("project_not_found")));
    }

    /// Issue 467: a session asked for in a shared project is made in its own and put into that
    /// one; one that cannot be put there is not made.
    #[tokio::test]
    async fn a_session_is_made_in_the_project_asked_for_or_not_at_all() {
        let state = state();
        let template = SessionTemplate {
            access: Vec::new(), summary: None, briefing: None, language: None, time_zone: None, approval_rules: Vec::new(), workspace: None,
            id: SessionTemplateId::new(), name: ScopedName::new("alone", Scope::GlobalScope).unwrap(), description: None,
            participants: vec![ParticipantTemplate::User { name: "user".to_string() }],
            channels: vec![ChannelTemplate {
                access: Vec::new(), summary: None, briefing: None, language: None, approval_rules: Vec::new(), uses_workspace: false,
                name: Name::new("main").unwrap(), components: Default::default(), classification: Classification::CHANNEL_DEFAULT,
                participants: vec!["user".to_string()], internal_actors: Vec::new(),
            }],
            security_attributes: Default::default(), commands: Default::default(), privacy: Default::default(), deleted: None,
        };
        state.runtime_manager.entity_stores().session_templates.create(&SecurityContext::system(), &template).await.unwrap();
        let api = api(Arc::clone(&state), &[]);
        let cookie = login(&api, "alice").await;
        let (_, project) = send(&api, &cookie, "POST", "/projects", json!({ "name": "report" })).await;
        let (status, made) = send(&api, &cookie, "POST", "/sessions", json!({ "template_id": template.id, "project_id": project["id"] })).await;
        assert_eq!((status, made["project_id"].clone()), (StatusCode::OK, project["id"].clone()), "{made}");

        let (status, refused) = send(&api, &cookie, "POST", "/sessions", json!({ "template_id": template.id, "project_id": Uuid::new_v4() })).await;
        assert_eq!((status, refused["code"].clone()), (StatusCode::NOT_FOUND, json!("project_not_found")), "{refused}");
        let (_, listed) = send(&api, &cookie, "GET", "/sessions", Value::Null).await;
        assert_eq!(listed.as_array().map(Vec::len), Some(1), "the session that could not be put there is not made: {listed}");
    }

    #[test]
    fn a_download_is_named_plainly_and_as_it_is() {
        assert_eq!(content_disposition("attachment", "report.md"), "attachment; filename=\"report.md\"; filename*=UTF-8''report%2Emd");
        assert_eq!(content_disposition("inline", "a.png"), "inline; filename=\"a.png\"; filename*=UTF-8''a%2Epng");
        let named = content_disposition("attachment", "Bericht \"Q3\" ü.pdf");
        assert!(named.starts_with("attachment; filename=\"Bericht__Q3___.pdf\";"), "{named}");
        assert!(named.ends_with("filename*=UTF-8''Bericht%20%22Q3%22%20%C3%BC%2Epdf"), "{named}");
    }
}
