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

    /// Wakes up the actor from a Sleep state.
    async fn wakeup(&self) -> Result<()> {
        Ok(())
    }

    async fn cancel(&self) -> Result<()> {
        Ok(())
    }

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
