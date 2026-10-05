//! What every server-sent event stream has in common (issue 33).
//!
//! A stream is one response that stays open, so `require_auth` sees it once
//! and never again: without a bound it would keep delivering after its token
//! had expired, which is the one place where the limit that
//! [`crate::auth`] puts on a token that cannot be revoked would not hold.
//!
//! So a stream ends shortly before its token does. `EventSource` reconnects
//! by itself, the new request is authenticated again - with the cookie the
//! client has renewed in the meantime (issue 31) - and the snapshot it asks
//! for restores whatever happened in between (issue 34). A client that is no
//! longer entitled gets a `401` on that request instead.
//!
//! A stream also has to answer a client that falls behind and loses events:
//! with a fresh snapshot if it asked for one, so that "here is the state"
//! replaces "something is missing" (issues 34, 38). See [`resync_on_lag`].

use crate::auth::AuthenticatedUser;
use axum::response::sse::Event;
use futures::stream::{self, Stream, StreamExt};
use serde::Serialize;
use std::convert::Infallible;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;

/// How long a client waits before reconnecting after the stream broke. Sent
/// as the SSE `retry` field, because the browser default (3-5 seconds,
/// depending on the browser) is otherwise the only thing deciding it. It
/// applies to unexpected ends only: an expiring stream announces itself with
/// `expired`, and a client that listens for it reconnects at once.
const RECONNECT_DELAY: Duration = Duration::from_millis(3000);

/// How long before the token's expiry a stream ends, so that the reconnect it
/// causes still carries a valid token rather than racing its expiry.
const EXPIRY_MARGIN: Duration = Duration::from_secs(5);

/// The event announcing that the stream ends because its session token is
/// about to expire. Named, so that clients which do not know it are
/// unaffected - for them the stream simply ends, as it would anyway.
fn expired_event() -> Event {
    Event::default().event("expired").data("the session token is about to expire")
}

/// `events`, bounded by the lifetime of the token that authenticated the
/// request: preceded by the reconnect delay, and ended by an `expired` event
/// shortly before the token is no longer valid.
pub fn bounded_by_token<S>(user: &AuthenticatedUser, events: S) -> impl Stream<Item = Result<Event, Infallible>> + Send
where
    S: Stream<Item = Result<Event, Infallible>> + Send + 'static,
{
    let remaining = user.expires_in();
    // A token with little left keeps its stream to the very end instead.
    // Subtracting the margin would leave a stream so short that the client
    // would reconnect into another one just as short, over and over until the
    // token finally expired.
    let lifetime = if remaining > 2 * EXPIRY_MARGIN { remaining - EXPIRY_MARGIN } else { remaining };

    // Whether the stream ended because of the deadline or by itself - only
    // the first is an expiry, and only it may tell a client to reconnect.
    let expired = Arc::new(AtomicBool::new(false));
    let deadline = {
        let expired = Arc::clone(&expired);
        async move {
            tokio::time::sleep(lifetime).await;
            expired.store(true, Ordering::Relaxed);
        }
    };

    let ending = stream::once(async move { expired.load(Ordering::Relaxed).then(expired_event) })
        .filter_map(|event| std::future::ready(event.map(Ok)));

    stream::once(std::future::ready(Ok(Event::default().retry(RECONNECT_DELAY))))
        .chain(events.take_until(deadline))
        .chain(ending)
}

/// `value` as a named event - `snapshot` - serialized as JSON. Named, so that
/// clients that do not know the name (and the events that follow, which stay
/// unnamed) are unaffected.
pub fn named_event(name: &str, value: &impl Serialize) -> Event {
    Event::default().event(name).data(json(value))
}

/// `value` as JSON, or - should it not serialize - nothing, and a warning:
/// an event without data rather than a broken stream.
fn json(value: &impl Serialize) -> String {
    serde_json::to_string(value).unwrap_or_else(|e| {
        tracing::warn!("An SSE event could not be serialized: {}", e);
        String::new()
    })
}

/// One server-sent event stream (issue 104): the snapshot `snapshot`
/// produces, if the client asked for one (`with_snapshot`) - none if it
/// produces `None`, having logged why; then the events
/// of `receiver` - with a fresh snapshot instead of a gap when the client
/// falls behind, see [`resync_on_lag`]; until `until` completes, or the
/// token that authenticated the request is about to expire (see
/// [`bounded_by_token`]). `what` names the stream in the log.
pub async fn stream<T, V, S, F, U>(
    user: &AuthenticatedUser,
    with_snapshot: bool,
    receiver: broadcast::Receiver<T>,
    what: String,
    snapshot: S,
    until: U,
) -> impl Stream<Item = Result<Event, Infallible>> + Send
where
    T: Serialize + Clone + Send + 'static,
    V: Serialize,
    S: Fn() -> F + Send + Sync + 'static,
    F: Future<Output = Option<V>> + Send,
    U: Future<Output = ()> + Send + 'static,
{
    let snapshot = Arc::new(snapshot);
    let first = if with_snapshot { snapshot().await.map(|value| named_event("snapshot", &value)) } else { None };
    let resync = with_snapshot.then(|| move || {
        let snapshot = Arc::clone(&snapshot);
        async move { snapshot().await.map(|value| named_event("snapshot", &value)) }
    });
    let updates = resync_on_lag(receiver, what, resync).take_until(until);
    bounded_by_token(user, stream::iter(first.map(Ok)).chain(updates))
}

/// The events of `receiver` as server-sent events, each serialized as JSON.
///
/// When the client falls behind and events are lost, it is sent what
/// `snapshot` produces, if it asked for snapshots - the state instead of a
/// gap - and otherwise, or if `snapshot` produces none, a comment saying how
/// many were lost. `what` names the
/// stream in the log.
pub fn resync_on_lag<T, S, F>(
    receiver: broadcast::Receiver<T>,
    what: String,
    snapshot: Option<S>,
) -> impl Stream<Item = Result<Event, Infallible>> + Send
where
    T: Serialize + Clone + Send + 'static,
    S: Fn() -> F + Send + Sync + 'static,
    F: Future<Output = Option<Event>> + Send,
{
    let snapshot = snapshot.map(Arc::new);
    BroadcastStream::new(receiver).then(move |item| {
        let snapshot = snapshot.clone();
        let what = what.clone();
        async move {
            match item {
                Ok(value) => Ok(Event::default().data(json(&value))),
                Err(BroadcastStreamRecvError::Lagged(lost)) => {
                    tracing::warn!("SSE client of {} fell behind and lost {} events", what, lost);
                    let resynced = match snapshot {
                        Some(snapshot) => snapshot().await,
                        None => None,
                    };
                    Ok(resynced.unwrap_or_else(|| Event::default().comment(format!("{} events lost", lost))))
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::{IntoResponse, Sse};
    use caelumora_model::user::UserId;
    use chrono::Utc;

    fn user(expires_in: i64) -> AuthenticatedUser {
        AuthenticatedUser { id: UserId::new(), expires_at: Utc::now().timestamp() + expires_in }
    }

    /// The stream as it goes over the wire, read to its end. Time is paused
    /// in these tests, so waiting for the deadline takes none.
    async fn wire<S>(user: &AuthenticatedUser, events: S) -> String
    where
        S: Stream<Item = Result<Event, Infallible>> + Send + 'static,
    {
        let response = Sse::new(bounded_by_token(user, events)).into_response();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("the stream ends");
        String::from_utf8(body.to_vec()).expect("events are text")
    }

    #[tokio::test(start_paused = true)]
    async fn a_stream_ends_with_an_expired_event_before_its_token_does() {
        // A stream that never ends by itself, so only the deadline ends it.
        let body = wire(&user(60), stream::pending()).await;

        assert!(body.starts_with("retry: 3000"), "the reconnect delay comes first: {body}");
        assert!(body.contains("event: expired"), "the client is told why the stream ends: {body}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_stream_that_ends_by_itself_does_not_claim_an_expiry() {
        let events = stream::iter([Ok(Event::default().data("one"))]);
        let body = wire(&user(600), events).await;

        assert!(body.contains("data: one"));
        assert!(!body.contains("expired"), "the token is still valid, the stream just ended: {body}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_stream_ends_a_margin_early() {
        let started = tokio::time::Instant::now();
        wire(&user(60), stream::pending()).await;
        assert_eq!(started.elapsed(), Duration::from_secs(55));
    }

    /// What `resync_on_lag` sends for three events on a channel with room
    /// for two, read until the channel closes.
    async fn after_losing_one(with_snapshot: bool) -> String {
        let (tx, rx) = broadcast::channel(2);
        for n in 1..=3 {
            tx.send(n).unwrap();
        }
        drop(tx);
        let snapshot = with_snapshot.then_some(|| async { Some(Event::default().event("snapshot").data("state")) });
        let events = resync_on_lag(rx, "test".to_string(), snapshot);
        let body = axum::body::to_bytes(Sse::new(events).into_response().into_body(), usize::MAX).await.unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn a_client_that_lost_events_gets_the_state_instead() {
        let body = after_losing_one(true).await;
        let snapshot = body.find("event: snapshot").expect("the loss is answered with a snapshot");
        assert!(snapshot < body.find("data: 2").unwrap(), "before the events that follow: {body}");
        assert!(body.contains("data: 3") && !body.contains("data: 1"), "{body}");
    }

    #[tokio::test]
    async fn a_client_without_snapshots_is_told_how_many_it_lost() {
        let body = after_losing_one(false).await;
        assert!(body.contains(": 1 events lost"), "{body}");
        assert!(!body.contains("snapshot"), "{body}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_token_with_little_left_carries_its_stream_to_the_end() {
        // Subtracting the margin would leave next to nothing here, and a
        // client reconnecting at once would do so over and over.
        for left in [10, 5, 2] {
            let started = tokio::time::Instant::now();
            let body = wire(&user(left), stream::pending()).await;

            assert_eq!(started.elapsed(), Duration::from_secs(left as u64), "with {left}s left");
            assert!(body.contains("event: expired"), "with {left}s left");
        }
    }
}
