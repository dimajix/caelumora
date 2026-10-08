//! Actors, the participants of a channel (see CHANNELS.md): the `Actor`
//! trait, the contexts an actor answers in (`Answering`), and the actors -
//! an agent's, an external agent's, a user's and an approver's. How an
//! agent's actor takes in what it is told is `intake`.

pub mod agent;
pub mod approval;
pub mod external_agent;
mod intake;
pub mod user;

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::any::Any;
use std::fmt::Debug;
use tracing::warn;

use crate::transport::{EventReceiver, EventSender};
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext, RetryTarget};
use caelumora_model::agent::RetryRefused;
use caelumora_model::message::Message;
use caelumora_model::security::SecurityContext;
use caelumora_model::tool::ToolSet;
use caelumora_model::work::TurnId;

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
    async fn tool_sets(&self) -> Vec<ToolSet> {
        Vec::new()
    }

    /// Wakes the actor from `Sleeping`. Synchronous and infallible on
    /// purpose: it is called from other tasks and must take effect at once.
    fn wakeup(&self) {}

    /// Ends the actor's current turn. Synchronous and infallible on
    /// purpose: a cancel comes from another task and must reach a running
    /// model call without waiting for anything.
    fn cancel(&self) {}

    /// Its turn under way (issue 235), if it is in one that can still be
    /// ended. None for an actor without such turns - a user, an approver,
    /// an external agent, whose answers are its remote side's.
    fn turn_under_way(&self) -> Option<TurnId> {
        None
    }

    /// Gives its turn `turn` up because the answer is asked for again
    /// (issue 235), if that is the turn under way, and returns whether it
    /// was: nothing else is ended. Synchronous like `cancel`, and like it
    /// the consequence of an act that was checked - the channel admitted the
    /// request. An actor that cannot answer anew keeps the default: it was
    /// in no such turn.
    fn retry_turn(&self, _turn: TurnId) -> bool {
        false
    }

    /// Ends the actor for good, because it left the channel (issue 75) -
    /// removed from it, or the channel deleted: a turn under way is given
    /// up, not kept for a resume (issue 145), and the channel told of its
    /// end - a removed actor's events are taken until its task has ended
    /// (issue 262). The act that removed it checked its permission and is
    /// who did it; stopping is its consequence, and like `cancel` and
    /// `wakeup` knows no caller.
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

    /// The contexts of the actor `id`, called `name`, speaking of itself
    /// rather than answering - nobody asked: those of its channel, in its
    /// own `security` context.
    pub(crate) fn own(id: ActorId, name: String, security: &SecurityContext) -> Self {
        Self { event_context: EventContext::new(security.session_id, security.channel_id, id, name), security_context: security.clone() }
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

/// Tells whoever asked each of `retries` that the agent `own`, called
/// `name`, does not answer again, for `reason` (issue 234) - in the
/// contexts of each request, as an answer to it. One that cannot be sent is
/// logged.
async fn reject_retries(retries: Vec<Envelope>, reason: RetryRefused, own: ActorId, name: &str, output: &dyn EventSender) {
    for retry in retries {
        let Some(target) = retry_target(&retry) else { continue };
        let rejected = ActorEvent::RetryRejected { target, reason };
        if let Err(e) = Answering::to(&retry, own, name.to_string()).send(rejected, output).await {
            warn!("Agent {} cannot reject a request to answer again: {:#}", own, e);
        }
    }
}

/// What the request to answer again in `retry` is about (issue 235);
/// `None` for any other event.
fn retry_target(retry: &Envelope) -> Option<RetryTarget> {
    match retry.event {
        ActorEvent::RetryRequested { target, .. } => Some(target),
        _ => None,
    }
}

/// What the channel's users are told of `error`, which ended an answer: its
/// message where it is one for them (`crate::error::user_message`), else
/// that the details are in the server log - which has them, and nothing
/// else about the server.
fn failure_message(error: &anyhow::Error) -> String {
    crate::error::user_message(error).unwrap_or_else(|| "internal error - the details are in the server log".to_string())
}

