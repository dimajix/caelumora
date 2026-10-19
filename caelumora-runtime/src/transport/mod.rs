pub mod memory;
pub use memory::{InMemoryEventReceiver, InMemoryEventSender, OriginEventSender};

use async_trait::async_trait;
use caelumora_model::actor::{ActorId, Envelope};
use caelumora_model::history::Heard;
use anyhow::Error;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
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
}

/// A test on an event, for `EventReceiver::recv_matching`. Spelt with its
/// lifetime bound for every reference, so that `async_trait` does not bind it
/// to one lifetime of the future.
pub type EventPredicate = for<'a> fn(&'a Envelope) -> bool;

/// What an actor receives by (issue 223): in a channel, its inbox
/// (`channel::inbox`) - a queue of what is delivered live, and behind it the
/// channel's history -, in a test a queue alone. Every method takes from one
/// sequence, in order, nothing twice; where the actor stands in it is
/// `heard`, which it stores with what it took in.
#[async_trait]
pub trait EventReceiver: Send {
    /// The next event, waiting for one; `None` once the input is closed.
    /// Cancel-safe, as a `tokio::select!` branch must be: cancelled, it has
    /// taken nothing.
    async fn recv(&mut self) -> Option<Envelope>;

    /// The next event that fulfils `predicate`, waiting for one, taken ahead
    /// of what does not, which stays where it is, in order, to be taken
    /// next. A search: it passes over what does not match, wherever the
    /// match is - unlike `try_recv_if`, which looks at the next event only.
    async fn recv_matching(&mut self, predicate: EventPredicate) -> Option<Envelope>;

    /// The next event that is there already, without waiting for one to
    /// arrive. `None` says that nothing is there now, not that the input
    /// is closed: that is `recv`'s to tell.
    async fn try_recv(&mut self) -> Option<Envelope>;

    /// The next event that is there already, without waiting, if it
    /// fulfils `predicate`; otherwise nothing, and the event stays where it
    /// is, to be taken next (issue 431) - what an actor takes in up to the
    /// first event that has to wait, so that everything behind it waits too,
    /// in order. Named as Rust names it (`Peekable::next_if`, `Vec::pop_if`):
    /// `_if` looks at the next event only, where `recv_matching` searches.
    async fn try_recv_if(&mut self, predicate: &(dyn for<'a> Fn(&'a Envelope) -> bool + Sync)) -> Option<Envelope>;

    /// Where the actor stands in its channel's history (issue 223): what of
    /// it was taken from here so far, in order and ahead. What an actor that
    /// keeps a state stores with what it took in, and what its inbox opens
    /// at when it runs again.
    fn heard(&self) -> Heard;
}
