//! A piece of work of an agent (issues 85, 211, 212): what it is done in
//! and as, whether it is a turn of an agent run here (`TurnState`) or an
//! exchange of an external agent with its remote agent and the task it may
//! begin (`OpenTask`). Both are bound by the channel as it was when they
//! began, and write as what they have seen.

use crate::actor::{EventContext, RequestReason};
use crate::confidentiality::Classification;
use crate::context::{ContextRef, FrameRef, KnowledgeRef, ModelProfile, PromptPart};
use crate::message::Message;
use crate::security::SecurityContext;
use crate::value::time_zone::TimeZone;
use serde::{Deserialize, Serialize};

define_id!(TurnId);

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
    /// Why it was asked for (issue 439): the reason of the request to
    /// respond it answers - the strongest of those it took in. `None` for
    /// work not begun by one, such as an answer given again.
    pub asked: Option<RequestReason>,
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
    /// Work begun in the contexts given, asked for as `asked` says, in a
    /// channel classified `classification`, which has seen nothing yet; the
    /// times of messages are given to its model in `time_zone`, its
    /// session's as it began (issue 424). How its model is given its
    /// context is the default until it is given as a profile says
    /// (`given_as`).
    pub fn new(event_context: EventContext, asked: Option<RequestReason>, security_context: SecurityContext, classification: Classification, time_zone: TimeZone) -> Self {
        let context = ContextRef { transcript: None, frame: FrameRef { time_zone, ..FrameRef::default() } };
        Self { turn_id: TurnId::new(), event_context, asked, security_context, classification, seen: Some(Classification::PUBLIC), context }
    }

    /// This work, its model given its context as `profile` says (issues 49,
    /// 135): recorded with the context of what comes of it, so that the
    /// context is resolved as it was given.
    pub fn given_as(mut self, profile: ModelProfile) -> Self {
        self.context.frame.profile = profile;
        self
    }

    /// The classification of what it writes - messages, notes, what is
    /// remembered (issue 168): what it has seen so far, at most its
    /// channel's classification when it began.
    pub fn label(&self) -> Classification {
        self.seen.clone().unwrap_or_else(|| self.classification.clone())
    }

    /// Whether it may see what is classified `classification` while it is under way (issue
    /// 431): if it flows into its classification. What it saw above it would be written below
    /// its level, since `see` keeps its label within its classification.
    pub fn may_see(&self, classification: &Classification) -> bool {
        classification.surely_flows_to(&self.classification)
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
        if !self.context.frame.knowledge.contains(&reference) {
            self.context.frame.knowledge.push(reference);
        }
    }

    /// Takes in what its `ContextProvider` assembled (issues 214, 285): the
    /// `knowledge` the prompt parts hold, recorded, and every one of the
    /// `parts` seen. The same for a turn, which places the parts itself, and
    /// for an external agent's work, whose remote agent is given only those
    /// it was not given before - and knows the others.
    pub fn take_in_context(&mut self, parts: &[PromptPart], knowledge: Vec<KnowledgeRef>) {
        for reference in knowledge {
            self.record_knowledge(reference);
        }
        for part in parts {
            self.see_classified(part.classification.as_ref());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidentiality::{Compartment, Confidentiality};
    use crate::context::KnowledgeSource;
    use crate::message::MessageRole;

    #[test]
    fn work_takes_in_the_knowledge_and_every_part_of_its_context() {
        let internal = Classification::general(Confidentiality::Internal);
        let confidential = Classification::general(Confidentiality::Confidential);
        let mut work = Work::new(EventContext::default(), None, SecurityContext::default(), confidential.clone(), Default::default());
        let part = |classification: Option<Classification>| PromptPart { role: MessageRole::System, text: "Be brief.".to_string(), after: None, classification, source: None };
        let read = KnowledgeRef {
            channel: Default::default(),
            session: Default::default(),
            source: KnowledgeSource::History { digest: None, from: None },
            classification: Some(internal.clone()),
        };

        work.take_in_context(&[part(Some(Classification::PUBLIC))], vec![read.clone()]);
        assert_eq!(work.context.frame.knowledge, [read], "the knowledge recorded");
        assert_eq!(work.label(), internal, "and seen");
        work.take_in_context(&[part(None)], Vec::new());
        assert_eq!(work.label(), confidential, "a part that does not say counts as the channel");
    }

    #[test]
    fn work_may_see_what_flows_into_its_classification() {
        let internal = Classification::general(Confidentiality::Internal);
        let work = Work::new(EventContext::default(), None, SecurityContext::default(), internal.clone(), Default::default());
        assert!(work.may_see(&Classification::PUBLIC), "below its classification");
        assert!(work.may_see(&internal), "at it");
        assert!(!work.may_see(&Classification::general(Confidentiality::Confidential)), "above it");
    }

    #[test]
    fn work_writes_as_what_it_has_seen_never_above_its_channel() {
        let classified = |level, compartments: &[&str]| Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap();
        let channel = classified(Confidentiality::Confidential, &["hr"]);
        let mut work = Work::new(EventContext::default(), None, SecurityContext::default(), channel.clone(), Default::default());
        assert_eq!(work.label(), Classification::PUBLIC, "nothing seen");

        work.see(&Classification::general(Confidentiality::Internal));
        assert_eq!(work.label(), Classification::general(Confidentiality::Internal));
        work.see(&classified(Confidentiality::Internal, &["hr", "sales"]));
        assert_eq!(work.label(), classified(Confidentiality::Internal, &["hr", "sales"]), "a wider audience than the channel's");
        work.see(&classified(Confidentiality::Internal, &["sales"]));
        assert_eq!(work.label(), channel, "not known to flow into the channel: the channel");
        work.see(&Classification::PUBLIC);
        assert_eq!(work.label(), channel, "it never falls");

        let mut unknown = Work::new(EventContext::default(), None, SecurityContext::default(), channel.clone(), Default::default());
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
        let work = Work::new(EventContext::default(), None, SecurityContext::default(), Classification::CHANNEL_DEFAULT, Default::default());
        assert_ne!(work.turn_id, Work::new(EventContext::default(), None, SecurityContext::default(), Classification::CHANNEL_DEFAULT, Default::default()).turn_id, "each its own");
        let stored: Work = serde_json::from_value(serde_json::to_value(&work).unwrap()).unwrap();
        assert_eq!(stored.turn_id, work.turn_id);
    }
}
