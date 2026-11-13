//! What the tests of the routes share: an API's state on a kernel of memory stores, logging in to
//! it, and reading a stream it sends.

use crate::auth::{Auth, TokenService};
use crate::state::AppState;
use axum::body::{Body, BodyDataStream};
use axum::http::header::SET_COOKIE;
use axum::http::Request;
use axum::Router;
use caelumora_domain::config::{AppConfig, AuthConfig};
use caelumora_kernel::event::EventBus;
use caelumora_kernel::project::ProjectManager;
use caelumora_kernel::session::SessionManager;
use caelumora_kernel::testing::TestKernel;
use caelumora_kernel::user_provider::accept_any::AcceptAnyUserProvider;
use caelumora_kernel::user_provider::UserManager;
use futures::StreamExt;
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
        .header("content-type", "application/json")
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
