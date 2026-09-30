use anyhow::Result;
use async_trait::async_trait;
use std::any::Any;
use std::fmt::Debug;

use crate::transport::{EventReceiver, EventSender};
use caelumora_model::actor::{ActorId, ActorState, Envelope};
use caelumora_model::context::ResolvedContext;
use caelumora_model::message::MessageId;
use caelumora_model::transcript::GivenUpTurn;
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

    /// Ends the actor for good, because it left the channel (issue 75) -
    /// removed from it, or the channel deleted: a turn under way is given
    /// up, not kept for a resume (issue 145). The act that removed it
    /// checked its permission and is who did it; stopping is its
    /// consequence, and like `cancel` and `wakeup` knows no caller.
    /// Synchronous like `cancel`; an actor with state ends its own task once
    /// that state is stored consistent, the channel closes its inbox and
    /// aborts it only if it does not end by itself.
    fn stop(&self) {}

    /// What this actor's model saw when it produced the message `message`:
    /// its context and system prompt (issue 06), on behalf of `caller`.
    /// Only agents have one; every other actor answers `NotRecorded`.
    ///
    /// # Errors
    /// `MessageContextError::Forbidden` if `caller` may not read it,
    /// `NotRecorded` if this actor did not produce the message,
    /// `Unavailable` if what was recorded cannot be resolved any more.
    async fn message_context(&self, _caller: &SecurityContext, message: MessageId) -> Result<ResolvedContext, MessageContextError> {
        Err(MessageContextError::NotRecorded { actor: self.id(), message })
    }

    /// The turns this actor gave up in its channel, with their messages
    /// (issue 145), for `caller` - who may read them where they may read
    /// the actor's context. Only agents take turns; every other actor has
    /// none.
    ///
    /// # Errors
    /// `GivenUpTurnsError::Forbidden` if `caller` may not read them,
    /// `Failed` if the actor's state cannot be read.
    async fn given_up_turns(&self, _caller: &SecurityContext) -> Result<Vec<GivenUpTurn>, GivenUpTurnsError> {
        Ok(Vec::new())
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

/// Why the turns an actor gave up cannot be shown. The messages are meant
/// for users (see `crate::error::user_message`).
#[derive(Debug, thiserror::Error)]
pub enum GivenUpTurnsError {
    #[error("You are not permitted to see what {actor} saw")]
    Forbidden { actor: ActorId },
    /// Reading the actor's state failed; the cause is in the log.
    #[error("The turns {actor} gave up cannot be read")]
    Failed { actor: ActorId },
}
