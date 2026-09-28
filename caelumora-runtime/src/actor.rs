use anyhow::Result;
use async_trait::async_trait;
use std::any::Any;
use std::fmt::Debug;

use crate::transport::{EventReceiver, EventSender};
use caelumora_model::actor::{ActorId, ActorState, Envelope};
use caelumora_model::agent::SeenContext;
use caelumora_model::message::MessageId;
use caelumora_model::security::SecurityContext;

#[async_trait]
pub trait Actor: Any + Debug + Send + Sync {
    fn id(&self) -> ActorId;
    fn kind(&self) -> String;
    fn name(&self) -> String;

    /// Returns the predicate that declares which events this actor wants to receive.
    /// The ChannelLoop wraps the actor's input sender in a `FilteringEventSender` using
    /// this predicate, so events that don't match are silently dropped at dispatch time.
    fn event_filter(&self) -> fn(&Envelope) -> bool;

    async fn run(
        &self,
        rx: Box<dyn EventReceiver>,
        tx: Box<dyn EventSender>,
    ) -> Result<()>;

    /// Returns the current processing state of the actor.
    fn state(&self) -> ActorState;

    /// Wakes the actor from `Sleeping`. Synchronous and infallible on
    /// purpose: it is called from other tasks and must take effect at once.
    fn wakeup(&self) {}

    /// Ends the actor's current turn. Synchronous and infallible on
    /// purpose: a cancel comes from another task and must reach a running
    /// model call without waiting for anything.
    fn cancel(&self) {}

    /// Ends the actor for good, because it left the channel (issue 75): a
    /// turn under way is abandoned, not kept for a resume. Synchronous like
    /// `cancel`; an actor with state ends its own task once that state is
    /// stored consistent, the channel closes its inbox and aborts it only
    /// if it does not end by itself.
    fn stop(&self) {}

    /// What this actor's model saw when it produced the message `message`:
    /// its context and system prompt (issue 06), on behalf of `caller`.
    /// Only agents have one; every other actor answers `NotRecorded`.
    ///
    /// # Errors
    /// `MessageContextError::Forbidden` if `caller` may not read it,
    /// `NotRecorded` if this actor did not produce the message,
    /// `Unavailable` if what was recorded cannot be resolved any more.
    async fn message_context(&self, _caller: &SecurityContext, message: MessageId) -> Result<SeenContext, MessageContextError> {
        Err(MessageContextError::NotRecorded { actor: self.id(), message })
    }

    /// Whether `caller` may read what this actor saw (`message_context`) -
    /// asked in advance, so that a client offers it only where it works.
    /// Only agents have anything to read.
    async fn may_read_context(&self, _caller: &SecurityContext) -> bool {
        false
    }
}

/// Why a message's context cannot be shown. The messages are meant for users
/// (see `crate::error::user_message`).
#[derive(Debug, thiserror::Error)]
pub enum MessageContextError {
    #[error("You are not permitted to see what {actor} saw")]
    Forbidden { actor: ActorId },
    #[error("No context is recorded for message {message} by {actor}")]
    NotRecorded { actor: ActorId, message: MessageId },
    #[error("The context of message {message} by {actor} is no longer available")]
    Unavailable { actor: ActorId, message: MessageId },
    /// Reading the actor's state failed; the cause is in the log.
    #[error("The context of message {message} by {actor} cannot be read")]
    Failed { actor: ActorId, message: MessageId },
}
