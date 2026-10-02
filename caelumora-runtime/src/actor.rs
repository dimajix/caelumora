use anyhow::Result;
use async_trait::async_trait;
use std::any::Any;
use std::fmt::Debug;

use crate::transport::{EventReceiver, EventSender};
use caelumora_model::actor::{ActorId, ActorState, Envelope};
use caelumora_model::message::Message;

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

    /// Takes in what it missed of the channel when it joins (issue 190):
    /// `visible` is the part of the channel's history it may see. An actor
    /// that keeps what it hears keeps of it what it does not have yet, as if
    /// it had heard it - for one that joins again, what was said after the
    /// last message it has. Called once, before it runs; one that keeps
    /// nothing has nothing to do.
    ///
    /// # Errors
    /// If what it keeps cannot be read or stored.
    async fn catch_up(&self, _visible: Vec<Message>) -> Result<()> {
        Ok(())
    }

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
    /// Synchronous like `cancel`; every actor ends its own task, one with
    /// state once that state is stored consistent. The channel waits for
    /// that and aborts the task only if it does not end by itself in time
    /// (issue 187) - which is why there is no default that does nothing.
    fn stop(&self);
}
