pub mod memory;
pub use memory::{FilteringEventSender, InMemoryEventReceiver, InMemoryEventSender, OriginEventSender};

use async_trait::async_trait;
use caelumora_model::actor::{ActorId, Envelope};
use anyhow::Error;
use tokio::sync::mpsc::error::TrySendError;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("channel full")]
    Full,
    #[error("channel closed")]
    Closed,
    #[error("transport backend: {0}")]
    Backend(String),
    /// The sender may not send it (issue 228): `origin` sent an envelope
    /// that speaks as another.
    #[error("{origin} may not send in the name of another")]
    Refused { origin: ActorId },
}

pub type TransportResult<T> = Result<T, TransportError>;

impl From<TrySendError<Envelope>> for TransportError {
    fn from(e: TrySendError<Envelope>) -> Self {
        match e {
            TrySendError::Full(_) => TransportError::Full,
            TrySendError::Closed(_) => TransportError::Closed,
        }
    }
}

/// Whether `error` says the receiving end is gone - for an actor's output,
/// that its channel no longer runs. Found through any context added to it.
pub fn is_closed(error: &Error) -> bool {
    matches!(error.downcast_ref::<TransportError>(), Some(TransportError::Closed))
}

/// Transport-agnostic outbound sender.
/// Used by actors to emit events and by channels to route events to actors.
#[async_trait]
pub trait EventSender: Send + Sync {
    /// Send an event, waiting for capacity if the underlying transport is full.
    async fn send(&self, event: Envelope) -> TransportResult<()>;

    /// Non-blocking send attempt.
    fn try_send(&self, event: Envelope) -> TransportResult<()>;
}

/// An output that drops what it is sent: for tests of what an actor does
/// besides sending.
#[cfg(any(test, feature = "test-support"))]
pub struct Discard;

#[cfg(any(test, feature = "test-support"))]
#[async_trait]
impl EventSender for Discard {
    async fn send(&self, _event: Envelope) -> TransportResult<()> {
        Ok(())
    }

    fn try_send(&self, _event: Envelope) -> TransportResult<()> {
        Ok(())
    }
}

/// A test on an event, for `EventReceiver::recv_matching`. Spelt with its
/// lifetime bound for every reference, so that `async_trait` does not bind it
/// to one lifetime of the future.
pub type EventPredicate = for<'a> fn(&'a Envelope) -> bool;

/// Transport-agnostic inbound receiver with selective extraction and Kafka-style commit.
#[async_trait]
pub trait EventReceiver: Send {
    /// Block until the next available event (checks the internal buffer first).
    /// Suitable as a `tokio::select!` branch.
    async fn recv(&mut self) -> Option<Envelope>;

    /// Block until an event matching `predicate` arrives.
    /// Non-matching events are stored in an internal buffer and returned by
    /// subsequent `recv()` calls. A search: it passes over what does not
    /// match, wherever the match is - unlike `try_recv_if`, which looks at
    /// the next event only.
    async fn recv_matching(&mut self, predicate: EventPredicate) -> Option<Envelope>;

    /// The next event that is there already, without waiting (checks the
    /// internal buffer first) - the counterpart of `EventSender::try_send`.
    /// `None` says that nothing is queued now, not that the input is
    /// closed: that is `recv`'s to tell.
    fn try_recv(&mut self) -> Option<Envelope>;

    /// The next event that is there already, without waiting, if it
    /// fulfils `predicate`; otherwise nothing, and the event stays where it
    /// is, to be taken next (issue 431) - what an actor takes in up to the
    /// first event that has to wait, so that everything behind it waits too,
    /// in order. Named as Rust names it (`Peekable::next_if`, `Vec::pop_if`):
    /// `_if` looks at the next event only, where `recv_matching` searches.
    fn try_recv_if(&mut self, predicate: &dyn Fn(&Envelope) -> bool) -> Option<Envelope>;

    /// Commit all events delivered since the last commit.
    /// In-memory transport: no-op. Persistent transports: advance offset / send ack.
    async fn commit(&mut self) -> TransportResult<()>;
}
