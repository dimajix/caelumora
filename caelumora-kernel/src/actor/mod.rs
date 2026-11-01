//! The kernel's actors, the participants of a channel it runs itself (see CHANNELS.md) - a user's
//! and an approver's -, the actors of a channel (`ActorManager`), and which actor an agent gets,
//! its kind's (`AgentKinds`, issue 370). What an actor is, is the API's (`api::actor`).

pub mod approval;
mod kinds;
mod manager;
pub mod user;

pub use kinds::AgentKinds;
pub use manager::{ActorManager, Shown};

use crate::api::actor::Answering;
use caelumora_model::actor::EventContext;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::{ChannelId, SessionId};
use uuid::Uuid;

/// What of an `Answering` the kernel alone builds (issue 516): one in the channel's name, in a span
/// of a user's, or replying to an event - the agent side answers only as it is asked
/// (`Answering::to`, `continuing`, `own`).
pub(crate) trait ChannelAnswering: Sized {
    /// The context in which the caller of `security`, called `name`, speaks
    /// in the channel `channel_id` of session `session_id`, whatever place
    /// `security` names - what the channel says in someone's name: that a
    /// participant was removed, in the name of who removed them, and why it
    /// did not let something in, in its own (`SecurityContext::system`).
    fn in_channel(
        session_id: SessionId,
        channel_id: ChannelId,
        name: impl Into<String>,
        security: &SecurityContext,
    ) -> Self;

    /// This context in the span `span_id`: a user's action, whose events
    /// share the span the frontend learns.
    fn with_span(self, span_id: Uuid) -> Self;

    /// This context in the span of `answered`, answering its event - what
    /// is said of an event that is no request, as the channel's refusal of
    /// it (`Answering::to` continues a request so).
    fn replying_to(self, answered: &EventContext) -> Self;
}

impl ChannelAnswering for Answering {
    fn in_channel(
        session_id: SessionId,
        channel_id: ChannelId,
        name: impl Into<String>,
        security: &SecurityContext,
    ) -> Self {
        let event_context = EventContext {
            initiator_id: security.initiator_id,
            ..EventContext::new(session_id, channel_id, security.caller_id, name)
        };
        Answering { event_context }
    }

    fn with_span(mut self, span_id: Uuid) -> Self {
        self.event_context.span_id = span_id;
        self
    }

    fn replying_to(self, answered: &EventContext) -> Self {
        let mut answering = self.with_span(answered.span_id);
        answering.event_context.correlation_id = answered.event_id;
        answering
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::{ActorEvent, ActorId};

    /// What the channel says of an event in its own name: in its place,
    /// whatever its context names, in the span of the event and answering
    /// it (issue 352).
    #[test]
    fn what_is_said_of_an_event_is_said_in_the_channel_and_answers_it() {
        let (session, channel) = (SessionId::new(), ChannelId::new());
        let answered = EventContext::new(session, channel, ActorId::System, "sender");

        let said = Answering::in_channel(session, channel, "system", &SecurityContext::system())
            .replying_to(&answered)
            .envelope(ActorEvent::WakeupRequested);

        assert_eq!(
            (said.context.session_id, said.context.channel_id),
            (session, channel),
            "in the channel, not where the system context is"
        );
        assert_eq!(
            said.context.sender_id,
            SecurityContext::system().caller_id,
            "in the name of who speaks"
        );
        assert_eq!(said.context.span_id, answered.span_id, "in the span of what it answers");
        assert_eq!(said.context.correlation_id, answered.event_id, "answering it");
        assert_ne!(said.context.event_id, answered.event_id, "an event of its own");
    }
}
