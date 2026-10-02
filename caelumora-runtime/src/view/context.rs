//! What an agent saw in a channel (issue 41): its state there, as stored,
//! for a viewer who may read its context (`ReadAgentContext`). Of an
//! external agent (issue 179), what it was given: its transcript.

use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentChannelState;
use caelumora_model::external_agent::ExternalAgentChannelState;
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::{ContextResolver, ResolvedContext};
use caelumora_model::digest::Digest;
use caelumora_model::message::MessageId;
use caelumora_model::note::Note;
use caelumora_model::transcript::{GivenUpTurn, Transcript};

/// The state of an agent in a channel, reached by whom may read what the
/// agent saw: from the store, where it is written after every step - the
/// running loop holds the in-memory state for as long as it runs.
#[derive(Debug, Clone)]
pub struct AgentContextView {
    actor: ActorId,
    seen: Seen,
}

/// The state the view reads, as the kind of agent keeps it.
#[derive(Debug, Clone)]
enum Seen {
    /// An agent run here: what its model was given, turn by turn.
    Internal(Box<AgentChannelState>),
    /// An external agent: what the remote agent was given, and nothing of
    /// what it made of it.
    External(Box<ExternalAgentChannelState>),
}

/// Why a message's context cannot be shown. The messages are meant for
/// users (see `crate::error::user_message`).
#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("No context is recorded for message {message} by {actor}")]
    NotRecorded { actor: ActorId, message: MessageId },
    #[error("The context of message {message} by {actor} is no longer available")]
    Unavailable { actor: ActorId, message: MessageId },
}

impl AgentContextView {
    pub(crate) fn of(actor: ActorId, state: AgentChannelState) -> Self {
        Self { actor, seen: Seen::Internal(Box::new(state)) }
    }

    /// The view of an external agent (issue 179).
    pub(crate) fn of_external(actor: ActorId, state: ExternalAgentChannelState) -> Self {
        Self { actor, seen: Seen::External(Box::new(state)) }
    }

    /// The agent whose state this is.
    pub fn actor(&self) -> ActorId {
        self.actor
    }

    /// What the agent's model is given of the conversation (issue 50), with
    /// the turns it gave up as branches (issue 145); of an external agent,
    /// what the remote agent was given (issue 179).
    pub fn transcript(&self) -> &Transcript {
        match &self.seen {
            Seen::Internal(state) => &state.transcript,
            Seen::External(state) => &state.transcript,
        }
    }

    /// The agent's private notes in the channel (issue 51), with the digest
    /// of their chain; an external agent writes none.
    pub fn private_notes(&self) -> (Vec<Note>, Option<Digest>) {
        match &self.seen {
            Seen::Internal(state) => (state.private_notes.current(), state.private_notes.digest()),
            Seen::External(_) => (Vec::new(), None),
        }
    }

    /// What the agent's model saw when it produced the message `message`:
    /// its context and system prompt (issue 06).
    ///
    /// # Errors
    /// `NotRecorded` if the agent did not produce the message - or is an
    /// external agent, whose context is the remote agent's and not known
    /// here; `Unavailable` if what was recorded cannot be resolved any more.
    pub fn context_of(&self, message: MessageId) -> Result<ResolvedContext, ContextError> {
        let not_recorded = ContextError::NotRecorded { actor: self.actor, message };
        let Seen::Internal(state) = &self.seen else { return Err(not_recorded) };
        let reference = state.recorded_context_of(message).ok_or(not_recorded)?;
        ContextResolver::of(state).resolve(&reference, self.actor).ok_or(ContextError::Unavailable { actor: self.actor, message })
    }

    /// The classification of the message `message` the agent produced, as
    /// its history holds it: the label of the turn that produced it (issue
    /// 168), unless it was reclassified since. `None` if the agent's
    /// history does not hold it.
    pub fn classification_of(&self, message: MessageId) -> Option<Classification> {
        let history = match &self.seen {
            Seen::Internal(state) => &state.history,
            Seen::External(state) => &state.history,
        };
        history.all_entries()
            .find(|e| e.message.id == message)
            .map(|e| e.message.classification.clone())
    }

    /// The turns the agent gave up in the channel, with their messages
    /// (issue 145).
    pub fn given_up_turns(&self) -> Vec<GivenUpTurn> {
        self.transcript().given_up_turns()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::AgentId;
    use caelumora_model::context::{ContextItem, ContextRef, PromptRef};
    use caelumora_model::digest::prompt;
    use caelumora_model::message::{Message, MessageRole};

    #[test]
    fn a_message_context_is_found_only_for_what_the_agent_produced_and_can_still_be_resolved() {
        let actor = ActorId::Agent(AgentId::new());
        let text = |t: &str| Message { id: MessageId::new(), role: MessageRole::User, content: Some(t.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let mut state = AgentChannelState::default();
        let question = text("question");
        state.transcript.push(question.clone());
        state.history.extend(vec![question.clone()]);
        let answer = text("answer");
        let before = state.transcript.push(answer.clone());
        state.history.push(answer.clone(), Some(ContextRef { transcript: before, prompts: Vec::new(), tools: None, knowledge: Vec::new(), profile: Default::default(), turn_entries: 0 }));
        let view = AgentContextView::of(actor, state.clone());

        let seen = view.context_of(answer.id).expect("the agent produced it");
        let ids: Vec<MessageId> = seen.items.iter().filter_map(|item| match item {
            ContextItem::Conversation { message } => Some(message.id),
            ContextItem::Prompt { .. } => None,
        }).collect();
        assert_eq!(ids, vec![question.id]);
        assert!(matches!(view.context_of(question.id), Err(ContextError::NotRecorded { .. })), "someone else wrote it");
        assert_eq!(view.classification_of(answer.id), Some(Classification::CHANNEL_DEFAULT), "the label it was written with");
        assert_eq!(view.classification_of(MessageId::new()), None);

        // A prompt part is told with how it was classified (issue 170).
        let confidential = Classification::general(caelumora_model::confidentiality::Confidentiality::Confidential);
        let system = PromptRef { text: prompt("system"), role: MessageRole::System, after: None, classification: Some(confidential.clone()) };
        state.prompts.insert(system.text, "system".to_string());
        let told = text("told");
        state.history.push(told.clone(), Some(ContextRef { transcript: before, prompts: vec![system], tools: None, knowledge: Vec::new(), profile: Default::default(), turn_entries: 0 }));
        let seen = AgentContextView::of(actor, state.clone()).context_of(told.id).expect("resolved");
        assert!(matches!(&seen.items[0], ContextItem::Prompt { classification: Some(c), .. } if *c == confidential));

        // A reference to a prompt that was never kept cannot be resolved.
        let lost = text("lost");
        let gone = PromptRef { text: prompt("gone"), role: MessageRole::System, after: None, classification: None };
        state.history.push(lost.clone(), Some(ContextRef { transcript: before, prompts: vec![gone], tools: None, knowledge: Vec::new(), profile: Default::default(), turn_entries: 0 }));
        let view = AgentContextView::of(actor, state);
        assert!(matches!(view.context_of(lost.id), Err(ContextError::Unavailable { .. })));
    }
}
