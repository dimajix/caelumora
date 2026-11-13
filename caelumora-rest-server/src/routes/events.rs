//! The global event stream of the API: what happens to whatever a user may read, from the event
//! bus in their name, begun with a snapshot of their sessions if asked (issues 34, 39, 80).

use crate::auth::AuthenticatedUser;
use crate::error::Query;
use crate::routes::views;
use crate::sse::stream;
use crate::state::AppState;
use axum::{
    extract::State,
    response::sse::{Event, Sse},
};
use caelumora_domain::snapshot::Detail;
use caelumora_kernel::event::SubscriptionTopic;
use caelumora_kernel::view::Views;
use caelumora_rest_api::stream::{GlobalSnapshot, StreamOptions};
use futures::stream::Stream;
use std::convert::Infallible;
use std::sync::Arc;

pub(super) async fn global_events(
    State(state): State<Arc<AppState>>,
    Query(options): Query<StreamOptions>,
    user: AuthenticatedUser,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Subscribe before reading the state, so that nothing that happens while
    // the snapshot is assembled is lost (issue 34).
    // Only the events about what this user may read (issue 39), decided by
    // the same policy as reading it through the store.
    // The views are built once, in the user's name, for the first snapshot and every one after a
    // loss; building them reads nothing.
    let views = views(&state, &user);
    let receiver = state
        .kernel
        .event_bus()
        .subscribe(views.context().clone(), vec![SubscriptionTopic::All])
        .into_receiver();
    let shutdown = state.shutdown.clone();
    let snapshot = move || {
        let views = views.clone();
        async move { global_snapshot(&views).await }
    };
    // Open until the token that authenticated it expires (issue 33).
    Sse::new(
        stream(
            &user,
            &shutdown,
            options.snapshot,
            receiver,
            "the global stream".to_string(),
            snapshot,
        )
        .await,
    )
}

/// Every session the viewer of `views` may read, with the channels they may read, as the sessions'
/// events refer to them by id alone - described by the kernel, running or not (issue 80).
async fn global_snapshot(views: &Views) -> Option<GlobalSnapshot> {
    let sessions = match views.sessions().await {
        Ok(sessions) => sessions,
        Err(e) => {
            tracing::error!("Cannot assemble the snapshot of the global event stream: {:#}", e);
            Vec::new()
        }
    };
    let mut described = Vec::with_capacity(sessions.len());
    for session in sessions {
        described.push(
            session.snapshot(Detail::CHANNELS | Detail::PARTICIPANTS | Detail::UNREAD).await.into(),
        );
    }
    Some(GlobalSnapshot { sessions: described })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::api;
    use crate::routes::testing::{login, read_until, state};
    use axum::body::Body;
    use axum::http::header::COOKIE;
    use axum::http::{Request, StatusCode};
    use caelumora_domain::session::testing::make_session;
    use caelumora_domain::user::UserId;
    use caelumora_kernel::session::testing::create;
    use caelumora_security::SecurityContext;
    use serde_json::Value;
    use tower::ServiceExt;

    /// The global stream begins with a snapshot of the sessions, before anything the bus tells.
    #[tokio::test]
    async fn the_global_stream_begins_with_a_snapshot_of_the_sessions() {
        let state = state();
        let api = api(Arc::clone(&state), &[]);
        let cookie = login(&api, "alice").await;
        let session = make_session("talk", UserId::new());
        let id = session.id.0.to_string();
        create(&state.session_manager, &SecurityContext::system(), session).await.unwrap();

        let request = Request::get("/events?snapshot=true").header(COOKIE, &cookie);
        let response = api.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        let mut read = String::new();
        // The first event after the field that tells how long to wait before reconnecting.
        let first = |read: &str| {
            read.split("\n\n").find(|block| !block.starts_with("retry:")).map(str::to_string)
        };
        read_until(&mut stream, &mut read, |read| read.matches("\n\n").count() >= 2).await;
        let first = first(&read).unwrap_or_default();
        let data = first.strip_prefix("event: snapshot\ndata: ");
        let snapshot: Value = serde_json::from_str(data.expect("the snapshot first")).unwrap();
        let ids: Vec<&str> = snapshot["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s["id"].as_str())
            .collect();
        assert!(ids.contains(&id.as_str()), "{ids:?}");
    }
}
