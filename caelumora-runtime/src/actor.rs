use anyhow::{Context, Result};
use async_trait::async_trait;
use std::any::Any;
use std::fmt::Debug;
use tracing::warn;

use crate::transport::{EventReceiver, EventSender};
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_model::message::Message;
use caelumora_model::security::SecurityContext;
use caelumora_model::tool::ToolSet;

#[async_trait]
pub(crate) trait Actor: Any + Debug + Send + Sync {
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

    /// The tools it has now, in their tool sets (issue 194), as it would
    /// offer them at the start of its next turn. None for an actor that has
    /// no tools here - a user, an approver, an external agent, whose tools
    /// are its remote side's.
    fn tool_sets(&self) -> Vec<ToolSet> {
        Vec::new()
    }

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

/// The contexts an actor publishes in when it answers: those of the
/// request, continued in its name - for an agent's turn those it began with
/// (`TurnState`), for an external agent's answer those of the request it
/// answers or of the task it speaks of.
#[derive(Debug, Clone)]
pub(crate) struct Answering {
    pub(crate) event_context: EventContext,
    pub(crate) security_context: SecurityContext,
}

impl Answering {
    /// The contexts of `request`, continued in the name of the actor `id`,
    /// called `name`.
    pub(crate) fn to(request: &Envelope, id: ActorId, name: String) -> Self {
        Self { event_context: request.context.next(id, name), security_context: request.security.next(id) }
    }

    /// Sends `event` as the actor publishes it: without what only its
    /// owner sees (issues 54, 133); an event with nothing left to show is
    /// not sent.
    pub(crate) async fn send(&self, event: ActorEvent, output: &dyn EventSender) -> Result<()> {
        let Some(event) = event.published() else { return Ok(()) };
        output.send(Envelope::new(self.security_context.clone(), self.event_context.clone(), event)).await
            .context(format!("actor {} failed to send an event", self.event_context.sender_id))
    }

    /// Sends `events` in turn: one that cannot be sent is logged, and the
    /// rest are sent still.
    pub(crate) async fn send_all(&self, events: Vec<ActorEvent>, output: &dyn EventSender) {
        for event in events {
            if let Err(e) = self.send(event, output).await {
                warn!("{:#}", e);
            }
        }
    }
}
