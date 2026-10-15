//! Actors, the participants of a channel (see CHANNELS.md): the `Actor`
//! trait, the contexts an actor answers in (`Answering`), and the actors -
//! an agent's, an external agent's, a user's and an approver's. What both
//! kinds of agent are in a channel is their `body`; how it takes in what it
//! is told is `intake`, the commands it carries out `commands`. Which actor
//! an agent gets is its kind's (`AgentKinds`, issue 370).

pub mod agent;
pub mod approval;
mod body;
mod run_control;
mod commands;
pub mod external_agent;
mod intake;
mod kinds;
pub mod user;

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::any::Any;
use std::fmt::Debug;
use tracing::warn;

use crate::transport::{EventReceiver, EventSender};
pub use kinds::AgentKinds;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext, RetryTarget};
use caelumora_model::agent::RetryRefused;
use caelumora_model::channel::ChannelId;
use caelumora_model::command::Command;
use caelumora_model::component::ComponentConfig;
use caelumora_model::message::Message;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::SessionId;
use caelumora_model::tool::ToolSet;
use caelumora_model::value::name::Name;
use caelumora_model::work::{TurnId, Work};
use uuid::Uuid;

/// A participant of a channel as the channel runs it. The runtime's own are
/// the user's and the approver's; an agent's is built by its kind
/// (`AgentKinds`, issue 370). Public, so that a kind of agent can be built
/// in a crate of its own; its control methods - `run`, `catch_up`,
/// `wakeup`, `cancel`, `interrupt_tool_call`, `retry_turn`, `stop` - check
/// nothing and are called by the channel alone, as the consequence of an
/// act that was checked (SECURITY.md).
#[async_trait]
pub trait Actor: Any + Debug + Send + Sync {
    fn id(&self) -> ActorId;
    fn name(&self) -> Name;

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

    /// What it was built from, its preconfigured components resolved (issue
    /// 270). None for an actor that is built from no components - a user,
    /// an approver.
    fn components(&self) -> ComponentConfig {
        ComponentConfig::default()
    }

    /// The commands it carries out itself that `issuer` may see (issue
    /// 269), sorted by name. None for an actor that carries out none - a
    /// user, an approver, an agent whose personality provides none.
    async fn commands(&self, _issuer: &SecurityContext) -> Vec<Command> {
        Vec::new()
    }

    /// Wakes the actor from `Sleeping`. Synchronous and infallible on
    /// purpose: it is called from other tasks and must take effect at once.
    fn wakeup(&self) {}

    /// Ends the actor's current turn. Synchronous and infallible on
    /// purpose: a cancel comes from another task and must reach a running
    /// model call without waiting for anything.
    fn cancel(&self) {}

    /// Interrupts its tool call `call` of the turn `turn` alone (issue 276),
    /// on behalf of `by`, called so, if that call runs or waits to run in
    /// the turn under way, and returns whether it did: the call is answered
    /// as interrupted and the turn goes on. Synchronous like `cancel`, and
    /// like it the consequence of an act that was checked. An actor without
    /// tool calls of its own keeps the default: it had no such call.
    fn interrupt_tool_call(&self, _turn: TurnId, _call: &str, _by: String) -> bool {
        false
    }

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
/// answers or of the task it speaks of, for the approver those of the call
/// it decides. Every envelope the runtime sends is built here (issue 352):
/// whose name it carries and what it continues is what the channel checks
/// at the door (`OriginEventSender`, the admission).
#[derive(Debug, Clone)]
pub(crate) struct Answering {
    pub(crate) event_context: EventContext,
    pub(crate) security_context: SecurityContext,
}

impl Answering {
    /// The contexts of `request`, continued in the name of the actor `id`,
    /// called `name`, answered for and with the rights its `own` context
    /// gives it - an agent's owner, with their roles as they are now and its
    /// limit (issues 255, 304, 341). The request's initiator stays the
    /// initiator - the root of the chain - and lends nothing.
    pub(crate) fn to(request: &Envelope, id: ActorId, name: impl Into<String>, own: &SecurityContext) -> Self {
        let security_context = request.security.next(id, own.owner_id, own.principal_id).with_roles(own.roles.clone(), own.limiting_roles.clone());
        Self { event_context: request.context.next(id, name), security_context }
    }

    /// The contexts `work` was begun in - the request it answers, continued
    /// in the actor's name as `to` continued it then: what comes of it,
    /// later too, is said in them, and stays in the span and the chain of
    /// the request (issue 45).
    pub(crate) fn continuing(work: &Work) -> Self {
        Self { event_context: work.event_context.clone(), security_context: work.security_context.clone() }
    }

    /// The contexts of the actor `id`, called `name`, speaking of itself
    /// rather than answering - nobody asked: those of its channel, in its
    /// own `security` context.
    pub(crate) fn own(id: ActorId, name: impl Into<String>, security: &SecurityContext) -> Self {
        Self { event_context: EventContext::new(security.session_id, security.channel_id, id, name), security_context: security.clone() }
    }

    /// The contexts in which the caller of `security`, called `name`, speaks
    /// in the channel `channel_id` of session `session_id`, whatever place
    /// `security` names - what the channel says in someone's name: that a
    /// participant was removed, in the name of who removed them, and why it
    /// did not let something in, in its own (`SecurityContext::system`).
    pub(crate) fn in_channel(session_id: SessionId, channel_id: ChannelId, name: impl Into<String>, security: SecurityContext) -> Self {
        Self { event_context: EventContext::new(session_id, channel_id, security.caller_id, name), security_context: security }
    }

    /// These contexts in the span `span_id`: a user's action, whose events
    /// share the span the frontend learns.
    pub(crate) fn with_span(mut self, span_id: Uuid) -> Self {
        self.event_context.span_id = span_id;
        self
    }

    /// These contexts in the span of `answered`, answering its event - what
    /// is said of an event that is no request, as the channel's refusal of
    /// it (`Answering::to` continues a request so).
    pub(crate) fn replying_to(self, answered: &EventContext) -> Self {
        let mut answering = self.with_span(answered.span_id);
        answering.event_context.correlation_id = answered.event_id;
        answering
    }

    /// `event` in these contexts, as it is: for a sender whose events carry
    /// nothing only an owner sees - a user, the channel - or that hands them
    /// on otherwise than through an `EventSender`. Every envelope is an event
    /// of its own, with an `event_id` of its own, however many are sent in
    /// the same contexts - the events of a turn, which share its span and
    /// what it answers (issue 352).
    pub(crate) fn envelope(&self, event: ActorEvent) -> Envelope {
        let context = EventContext { event_id: Uuid::new_v4(), ..self.event_context.clone() };
        Envelope::new(self.security_context.clone(), context, event)
    }

    /// Sends `event` as the actor publishes it: without what only its
    /// owner sees (issues 54, 133); an event with nothing left to show is
    /// not sent.
    pub(crate) async fn send(&self, event: ActorEvent, output: &dyn EventSender) -> Result<()> {
        let Some(event) = event.published() else { return Ok(()) };
        output.send(self.envelope(event)).await
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
/// `name` and acting with the rights of its context `acting`, does not
/// answer again, for `reason` (issue 234) - in the contexts of each request,
/// as an answer to it. One that cannot be sent is logged.
async fn reject_retries(retries: Vec<Envelope>, reason: RetryRefused, own: ActorId, name: &str, acting: &SecurityContext, output: &dyn EventSender) {
    for retry in retries {
        let Some(target) = retry_target(&retry) else { continue };
        let rejected = ActorEvent::RetryRejected { target, reason };
        if let Err(e) = Answering::to(&retry, own, name.to_string(), acting).send(rejected, output).await {
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


#[cfg(test)]
mod tests {
    use super::*;

    /// What the channel says of an event in its own name: in its place,
    /// whatever its context names, in the span of the event and answering
    /// it (issue 352).
    #[test]
    fn what_is_said_of_an_event_is_said_in_the_channel_and_answers_it() {
        let (session, channel) = (SessionId::new(), ChannelId::new());
        let answered = EventContext::new(session, channel, ActorId::System, "sender");

        let said = Answering::in_channel(session, channel, "system", SecurityContext::system()).replying_to(&answered).envelope(ActorEvent::WakeupRequested);

        assert_eq!((said.context.session_id, said.context.channel_id), (session, channel), "in the channel, not where the system context is");
        assert_eq!(said.context.sender_id, SecurityContext::system().caller_id, "in the name of who speaks");
        assert_eq!(said.context.span_id, answered.span_id, "in the span of what it answers");
        assert_eq!(said.context.correlation_id, answered.event_id, "answering it");
        assert_ne!(said.context.event_id, answered.event_id, "an event of its own");
    }

    /// Every envelope sent in the same contexts - the events of a turn - is
    /// an event of its own (issue 352).
    #[test]
    fn every_envelope_is_an_event_of_its_own() {
        let request = Answering::own(ActorId::System, "user", &SecurityContext::system()).envelope(ActorEvent::WakeupRequested);
        let turn = Answering::to(&request, ActorId::System, "agent", &SecurityContext::system());

        let (first, second) = (turn.envelope(ActorEvent::WakeupRequested), turn.envelope(ActorEvent::WakeupRequested));

        assert_ne!(first.context.event_id, second.context.event_id, "two events, two event ids");
        assert_eq!(first.context.span_id, second.context.span_id, "in the span of the request");
        assert_eq!(first.context.correlation_id, request.context.event_id, "answering it");
        assert_eq!(second.context.correlation_id, request.context.event_id, "both of them");
    }
}
