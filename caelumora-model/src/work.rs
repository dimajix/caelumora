//! A piece of work of an agent (issues 85, 211, 212): what it is done in
//! and as, whether it is a turn of an agent run here (`TurnState`) or an
//! exchange of an external agent with its remote agent and the task it may
//! begin (`OpenTask`). Both are bound by the channel as it was when they
//! began, and write as what they have seen.

use crate::actor::EventContext;
use crate::confidentiality::Classification;
use crate::context::{ContextRef, KnowledgeRef};
use crate::message::Message;
use crate::security::SecurityContext;
use serde::{Deserialize, Serialize};

crate::define_id!(TurnId);

/// A piece of work of an agent: the contexts it is done in, the most it may
/// write as, what it has seen so far, and what what comes of it is produced
/// from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Work {
    /// Which piece of work it is (issue 235): given when it begins and kept
    /// with it, so that a turn resumed after a restart is the same turn.
    /// Every message it produces carries it (`Message::turn_id`), and so
    /// does `ResponseStarted`.
    pub turn_id: TurnId,
    /// The envelope it answers, continued in the agent's name - its sender
    /// is the agent: carries the ids (`span_id`, `correlation_id`) that tie
    /// it to what started it, so that what comes of it, later too, stays in
    /// the span and the chain between agents of the request (issue 45).
    pub event_context: EventContext,
    /// Whose authority it acts with.
    pub security_context: SecurityContext,
    /// The channel's classification as it was when it began (issues 61,
    /// 133): it is consistent in itself should the channel be reclassified
    /// (issue 67), and what it writes is classified so at most.
    pub classification: Classification,
    /// The join of the classifications of everything it has seen so far
    /// (issue 168) - a turn's transcript, prompt parts, knowledge read and
    /// tool results; what an external agent was given and what its remote
    /// agent brings in itself - never above `classification`: what it
    /// writes is classified so (`label`). `None` for work stored before it
    /// was kept, which counts as having seen its channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<Classification>,
    /// What comes of it is produced from, as references (issues 47, 49,
    /// 136, 137, 205): what is recorded with each message it writes. A
    /// turn's names the transcript's head and moves on with every message
    /// it says (`AgentChannelState::said`, issue 260);
    /// an external agent's is what its remote agent was given in the call,
    /// since the remote agent keeps what it was given before - empty until
    /// it was sent.
    #[serde(default)]
    pub context: ContextRef,
}

impl Work {
    /// Work begun in the contexts given, in a channel classified
    /// `classification`, which has seen nothing yet.
    pub fn new(event_context: EventContext, security_context: SecurityContext, classification: Classification) -> Self {
        Self { turn_id: TurnId::new(), event_context, security_context, classification, seen: Some(Classification::PUBLIC), context: ContextRef::default() }
    }

    /// The classification of what it writes - messages, notes, what is
    /// remembered (issue 168): what it has seen so far, at most its
    /// channel's classification when it began.
    pub fn label(&self) -> Classification {
        self.seen.clone().unwrap_or_else(|| self.classification.clone())
    }

    /// Records that it has seen what is classified `classification`: its
    /// label rises to the join of both, within the channel's
    /// classification, which everything it sees flows into
    /// (`Classification::join_within`).
    pub fn see(&mut self, classification: &Classification) {
        self.seen = Some(self.label().join_within(classification, &self.classification));
    }

    /// Records that it has seen what came with `classification`, if it came
    /// with one (`see`): what is not classified counts as the channel.
    pub fn see_classified(&mut self, classification: Option<&Classification>) {
        let channel = self.classification.clone();
        self.see(classification.unwrap_or(&channel));
    }

    /// Records that it has seen `message`: its classification and that of
    /// every tool result in it.
    pub fn see_message(&mut self, message: &Message) {
        self.see(&message.classification);
        for result in &message.tool_results {
            self.see(&result.classification);
        }
    }

    /// Records that it read `reference` (issue 17): it counts for the
    /// messages produced from now on, and it has seen what it is classified
    /// (`see`). Recorded once.
    pub fn record_knowledge(&mut self, reference: KnowledgeRef) {
        self.see_classified(reference.classification.as_ref());
        if !self.context.knowledge.contains(&reference) {
            self.context.knowledge.push(reference);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidentiality::{Compartment, Confidentiality};

    #[test]
    fn work_writes_as_what_it_has_seen_never_above_its_channel() {
        let classified = |level, compartments: &[&str]| Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap();
        let channel = classified(Confidentiality::Confidential, &["hr"]);
        let mut work = Work::new(EventContext::default(), SecurityContext::default(), channel.clone());
        assert_eq!(work.label(), Classification::PUBLIC, "nothing seen");

        work.see(&Classification::general(Confidentiality::Internal));
        assert_eq!(work.label(), Classification::general(Confidentiality::Internal));
        work.see(&classified(Confidentiality::Internal, &["hr", "sales"]));
        assert_eq!(work.label(), classified(Confidentiality::Internal, &["hr", "sales"]), "a wider audience than the channel's");
        work.see(&classified(Confidentiality::Internal, &["sales"]));
        assert_eq!(work.label(), channel, "not known to flow into the channel: the channel");
        work.see(&Classification::PUBLIC);
        assert_eq!(work.label(), channel, "it never falls");

        let mut unknown = Work::new(EventContext::default(), SecurityContext::default(), channel.clone());
        unknown.see_classified(None);
        assert_eq!(unknown.label(), channel, "what is not known counts as the channel");
        let stored_before = Work { classification: channel.clone(), ..Work::default() };
        assert_eq!(stored_before.label(), channel, "work stored before it was kept has seen its channel");
    }

    #[test]
    fn work_is_stored_as_before_it_was_one_type() {
        // An external agent's work, stored with `seen` always set and
        // `context` only once sent.
        let stored = serde_json::json!({
            "turn_id": TurnId::new(),
            "event_context": EventContext::default(),
            "security_context": SecurityContext::default(),
            "classification": Classification::CHANNEL_DEFAULT,
            "seen": Classification::PUBLIC,
        });
        let work: Work = serde_json::from_value(stored).unwrap();
        assert_eq!(work.label(), Classification::PUBLIC);
        assert_eq!(work.context, ContextRef::default());
    }

    #[test]
    fn work_keeps_its_id_when_stored() {
        let work = Work::new(EventContext::default(), SecurityContext::default(), Classification::CHANNEL_DEFAULT);
        assert_ne!(work.turn_id, Work::new(EventContext::default(), SecurityContext::default(), Classification::CHANNEL_DEFAULT).turn_id, "each its own");
        let stored: Work = serde_json::from_value(serde_json::to_value(&work).unwrap()).unwrap();
        assert_eq!(stored.turn_id, work.turn_id);
    }
}
