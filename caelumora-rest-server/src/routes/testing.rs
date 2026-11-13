//! What the tests of the routes share: an API's state on a kernel of memory stores, logging in to
//! it, asking it, a session of a user's in it, and reading a stream it sends.

use crate::auth::{Auth, TokenService};
use crate::state::AppState;
use axum::body::{to_bytes, Body, BodyDataStream};
use axum::http::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use caelumora_domain::channel::{ChannelId, Participant};
use caelumora_domain::config::{AppConfig, AuthConfig};
use caelumora_domain::session::testing::make_session;
use caelumora_domain::session::SessionId;
use caelumora_domain::user::UserId;
use caelumora_kernel::event::EventBus;
use caelumora_kernel::project::ProjectManager;
use caelumora_kernel::session::testing::create;
use caelumora_kernel::session::SessionManager;
use caelumora_kernel::testing::TestKernel;
use caelumora_kernel::user_provider::accept_any::AcceptAnyUserProvider;
use caelumora_kernel::user_provider::UserManager;
use caelumora_security::SecurityContext;
use futures::StreamExt;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use tower::ServiceExt;

/// The state of an API that lets anybody log in and permits everything, on a kernel of memory
/// stores.
pub(super) fn state() -> Arc<AppState> {
    state_with(AppConfig::default())
}

/// The state `state` makes, of a kernel configured by `config`.
pub(super) fn state_with(config: AppConfig) -> Arc<AppState> {
    let kernel = TestKernel { config, plugins: Vec::new(), ..Default::default() }.build();
    let users = Arc::clone(&kernel.entity_stores().users);
    let auth = Arc::new(Auth {
        tokens: TokenService::new(b"0123456789abcdef0123456789abcdef", &AuthConfig::default())
            .unwrap(),
        users: Arc::new(UserManager::new(
            vec![Arc::new(AcceptAnyUserProvider::new(Arc::clone(&users)))],
            users,
            Arc::<EventBus>::clone(kernel.event_bus()),
        )),
        controllers: Arc::clone(kernel.controller_manager()),
    });
    let session_manager = Arc::new(SessionManager::new(Arc::clone(&kernel)));
    let project_manager = Arc::new(ProjectManager::new(Arc::clone(&session_manager)));
    Arc::new(AppState {
        auth,
        kernel,
        session_manager,
        project_manager,
        compartments: Default::default(),
        shutdown: Default::default(),
    })
}

/// The cookie `api` answers `username` logging in with.
pub(super) async fn login(api: &Router, username: &str) -> String {
    let body = format!(r#"{{"username":"{username}","password":"x"}}"#);
    let login = Request::post("/auth/login")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap();
    let logged_in = api.clone().oneshot(login).await.unwrap();
    logged_in
        .headers()
        .get(SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .unwrap()
        .to_string()
}

/// What `api` answers `method` on `uri`, with `body` as JSON if given, for whoever `cookie` logs
/// in: its status and its body as JSON, `null` for none.
pub(super) async fn send(
    api: &Router,
    cookie: &str,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri).header(COOKIE, cookie);
    let request = match body {
        Some(body) => {
            request.header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string()))
        }
        None => request.body(Body::empty()),
    };
    let answer = api.clone().oneshot(request.unwrap()).await.unwrap();
    let status = answer.status();
    let bytes = to_bytes(answer.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// The id of whoever `cookie` logs in to `api`, as `/auth/me` answers it.
pub(super) async fn user_id(api: &Router, cookie: &str) -> UserId {
    let (status, me) = send(api, cookie, Method::GET, "/auth/me", None).await;
    let id = me["id"].as_str().unwrap_or_else(|| panic!("{status}: {me}"));
    UserId(id.parse().unwrap())
}

/// A session of `user` in `state`, made and started as the system with them in its main channel:
/// its id and the main channel's.
pub(super) async fn session_of(state: &AppState, user: UserId) -> (SessionId, ChannelId) {
    let mut session = make_session("talk", user);
    session.participants = vec![Participant::User(user)];
    let ids = (session.id, session.channels[0]);
    create(&state.session_manager, &SecurityContext::system(), session).await.unwrap();
    ids
}

/// What `stream` sent, read on until `seen` holds of all of it.
pub(super) async fn read_until(
    stream: &mut BodyDataStream,
    read: &mut String,
    seen: impl Fn(&str) -> bool,
) {
    while !seen(read) {
        let chunk = time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap_or_else(|_| panic!("nothing more came of the stream: {read}"))
            .unwrap_or_else(|| panic!("the stream ended: {read}"))
            .unwrap();
        read.push_str(&String::from_utf8_lossy(&chunk));
    }
}
