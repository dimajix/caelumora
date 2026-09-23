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

use crate::auth::AuthenticatedUser;
use axum::response::sse::Event;
use futures::stream::{self, Stream, StreamExt};
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

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
        let expired = expired.clone();
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
