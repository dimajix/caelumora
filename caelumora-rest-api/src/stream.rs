//! What the event streams are asked and start with (issue 34).

use crate::session::SessionSnapshot;
use caelumora_model::actor::{ActorEvent, Envelope, EventContext};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Whether an event stream starts with a snapshot of the current state
/// (issue 34). Off by default, so a client that does not want one does not
/// pay for it.
#[derive(Debug, Default, Deserialize)]
pub struct StreamOptions {
    #[serde(default)]
    pub snapshot: bool,
}

/// The state of the system when a global stream connects: the sessions and
/// channels the user may read, as the REST endpoints decide it.
#[derive(Debug, Serialize)]
pub struct GlobalSnapshot {
    pub sessions: Vec<SessionSnapshot>,
}

/// An event as a channel's stream shows it (issue 432): when it was sent,
/// in which chain and by whom, and what it is - not the security context it
/// was sent in, which the server checks by and no client is to see: it names
/// the sender's owner and their roles.
#[derive(Debug, Serialize)]
pub struct ShownEnvelope {
    pub timestamp: DateTime<Utc>,
    pub context: EventContext,
    pub event: ActorEvent,
}

impl From<Envelope> for ShownEnvelope {
    fn from(envelope: Envelope) -> Self {
        let Envelope { timestamp, context, event, .. } = envelope;
        Self { timestamp, context, event }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::ActorId;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::user::UserId;
    use caelumora_model::work::TurnId;

    #[test]
    fn a_shown_envelope_names_no_security_context() {
        let alice = ActorId::User(UserId::new());
        let envelope = Envelope::new(SecurityContext::new(alice, Default::default(), Default::default()), EventContext { sender_id: alice, ..EventContext::default() }, ActorEvent::ResponseStarted { turn_id: TurnId::new() });
        let shown = serde_json::to_value(ShownEnvelope::from(envelope)).unwrap();
        let mut fields: Vec<_> = shown.as_object().unwrap().keys().cloned().collect();
        fields.sort();
        assert_eq!(fields, ["context", "event", "timestamp"], "when, in which chain, what - nobody's roles");
    }
}
