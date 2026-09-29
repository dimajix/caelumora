use async_trait::async_trait;
use caelumora_model::actor::Envelope;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("channel full")]
    Full,
    #[error("channel closed")]
    Closed,
    #[error("event filtered")]
    Filtered,
    #[error("transport backend: {0}")]
    Backend(String),
}

pub type TransportResult<T> = Result<T, TransportError>;

impl From<tokio::sync::mpsc::error::TrySendError<Envelope>> for TransportError {
    fn from(e: tokio::sync::mpsc::error::TrySendError<Envelope>) -> Self {
        match e {
            tokio::sync::mpsc::error::TrySendError::Full(_) => TransportError::Full,
            tokio::sync::mpsc::error::TrySendError::Closed(_) => TransportError::Closed,
        }
    }
}

/// Whether `error` says the receiving end is gone - for an actor's output,
/// that its channel no longer runs. Found through any context added to it.
pub fn is_closed(error: &anyhow::Error) -> bool {
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

/// Transport-agnostic inbound receiver with selective extraction and Kafka-style commit.
#[async_trait]
pub trait EventReceiver: Send {
    /// Block until the next available event (checks the internal buffer first).
    /// Suitable as a `tokio::select!` branch.
    async fn recv(&mut self) -> Option<Envelope>;

    /// Block until an event matching `predicate` arrives.
    /// Non-matching events are stored in an internal buffer and returned by
    /// subsequent `recv()` calls or `take_buffered()`.
    async fn recv_matching(&mut self, predicate: fn(&Envelope) -> bool) -> Option<Envelope>;

    /// Non-blocking: drain all currently buffered events that match `predicate`,
    /// leaving the rest in the buffer.
    fn drain_matching(&mut self, predicate: fn(&Envelope) -> bool) -> Vec<Envelope>;

    /// Take all internally buffered (held) events, clearing the buffer.
    fn take_buffered(&mut self) -> Vec<Envelope>;

    /// Commit all events delivered since the last commit.
    /// In-memory transport: no-op. Persistent transports: advance offset / send ack.
    async fn commit(&mut self) -> TransportResult<()>;
}
