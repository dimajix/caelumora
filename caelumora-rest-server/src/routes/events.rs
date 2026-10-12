use caelumora_model::snapshot::Detail;
use crate::auth::AuthenticatedUser;
use caelumora_rest_api::stream::{GlobalSnapshot, StreamOptions};
use crate::state::AppState;
use axum::{
    extract::{Query, State},
    response::sse::{Event, Sse},
};
use futures::stream::Stream;
use std::convert::Infallible;
use std::sync::Arc;

pub async fn global_events(
    State(state): State<Arc<AppState>>,
    Query(options): Query<StreamOptions>,
    user: AuthenticatedUser,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Subscribe before reading the state, so that nothing that happens while
    // the snapshot is assembled is lost (issue 34).
    // Only the events about what this user may read (issue 39), decided by
    // the same policy as reading it through the store.
    let receiver = state.runtime_manager.event_bus().subscribe_as(user.security_context()).into_receiver();
    let reader = user.clone();
    let snapshot = move || {
        let (state, user) = (Arc::clone(&state), reader.clone());
        async move { global_snapshot(&state, &user).await }
    };
    // Open until the token that authenticated it expires (issue 33).
    Sse::new(crate::sse::stream(&user, options.snapshot, receiver, "the global stream".to_string(), snapshot, std::future::pending()).await)
}

/// Every session the user may read, with the channels they may read, as the
/// sessions' events refer to them by id alone - described by the runtime,
/// running or not (issue 80).
async fn global_snapshot(state: &Arc<AppState>, user: &AuthenticatedUser) -> Option<GlobalSnapshot> {
    let sessions = match crate::routes::views(state, user).sessions().await {
        Ok(sessions) => sessions,
        Err(e) => {
            tracing::error!("Cannot assemble the snapshot of the global event stream: {:#}", e);
            Vec::new()
        }
    };
    let mut described = Vec::with_capacity(sessions.len());
    for session in sessions {
        described.push(session.snapshot(Detail::CHANNELS | Detail::MEMBERS).await.into());
    }
    Some(GlobalSnapshot { sessions: described })
}
