//! What an agent saw in a channel (issue 41): its state there, as stored,
//! for a viewer who may read its context (`ReadAgentContext`). An agent run
//! here and an external agent (issue 179) keep different states, and each
//! is read by a view of its own.

use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentChannelState;
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::{ContextResolver, ResolvedContext};
use caelumora_model::digest::Digest;
use caelumora_model::external_agent::ExternalAgentChannelState;
use caelumora_model::message::MessageId;
use caelumora_model::message_history::MessageHistory;
use caelumora_model::note::Note;
use caelumora_model::transcript::{GivenUpTurn, Transcript};
use std::fmt::Debug;

/// The state of an agent in a channel, reached by whom may read what the
/// agent saw: from the store, where it is written after every step - the
/// running loop holds the in-memory state for as long as it runs.
pub trait AgentContextView: Send + Sync + Debug {
    /// The agent whose state this is.
    fn actor(&self) -> ActorId;

    /// What the agent was given of the conversation: what its model is
    /// given (issue 50), with the turns it gave up as branches (issue 145);
    /// of an external agent, what the remote agent was given (issue 179).
    fn transcript(&self) -> &Transcript;

    /// The agent's private notes in the channel (issue 51), with the digest
    /// of their chain.
    fn private_notes(&self) -> (Vec<Note>, Option<Digest>);

    /// What the agent's model saw when it produced the message `message`:
    /// its context and system prompt (issue 06).
    ///
    /// # Errors
    /// `NotRecorded` if the agent did not produce the message, or what it
    /// saw is not known here; `Unavailable` if what was recorded cannot be
    /// resolved any more.
    fn context_of(&self, message: MessageId) -> Result<ResolvedContext, ContextError>;

    /// The classification of the message `message` the agent produced, as
    /// its history holds it: the label it was written with (issue 168),
    /// unless it was reclassified since. `None` if the agent's history does
    /// not hold it.
    fn classification_of(&self, message: MessageId) -> Option<Classification>;

    /// The turns the agent gave up in the channel, with their messages
    /// (issue 145).
    fn given_up_turns(&self) -> Vec<GivenUpTurn>;
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

/// The classification of `message` as `history` holds it.
fn classification_in(history: &MessageHistory, message: MessageId) -> Option<Classification> {
    history.all_entries()
        .find(|e| e.message.id == message)
        .map(|e| e.message.classification.clone())
}

/// What an agent run here saw: what its model was given, turn by turn.
#[derive(Debug, Clone)]
pub(crate) struct InternalContext {
    actor: ActorId,
    state: AgentChannelState,
}

impl InternalContext {
    pub(crate) fn of(actor: ActorId, state: AgentChannelState) -> Self {
        Self { actor, state }
    }
}

impl AgentContextView for InternalContext {
    fn actor(&self) -> ActorId {
        self.actor
    }

    fn transcript(&self) -> &Transcript {
        &self.state.transcript
    }

    fn private_notes(&self) -> (Vec<Note>, Option<Digest>) {
        (self.state.private_notes.current(), self.state.private_notes.digest())
    }

    fn context_of(&self, message: MessageId) -> Result<ResolvedContext, ContextError> {
        let reference = self.state.recorded_context_of(message)
            .ok_or(ContextError::NotRecorded { actor: self.actor, message })?;
        ContextResolver::of(&self.state).resolve(&reference, self.actor).ok_or(ContextError::Unavailable { actor: self.actor, message })
    }

    fn classification_of(&self, message: MessageId) -> Option<Classification> {
        classification_in(&self.state.history, message)
    }

    fn given_up_turns(&self) -> Vec<GivenUpTurn> {
        self.state.transcript.given_up_turns()
    }
}

/// What an external agent was given (issue 179), and nothing of what the
/// remote agent made of it: no private notes, no turns, no context of its
/// answers.
#[derive(Debug, Clone)]
pub(crate) struct ExternalContext {
    actor: ActorId,
    state: ExternalAgentChannelState,
}

impl ExternalContext {
    pub(crate) fn of(actor: ActorId, state: ExternalAgentChannelState) -> Self {
        Self { actor, state }
    }
}

impl AgentContextView for ExternalContext {
    fn actor(&self) -> ActorId {
        self.actor
    }

    fn transcript(&self) -> &Transcript {
        &self.state.transcript
    }

    fn private_notes(&self) -> (Vec<Note>, Option<Digest>) {
        (Vec::new(), None)
    }

    fn context_of(&self, message: MessageId) -> Result<ResolvedContext, ContextError> {
        Err(ContextError::NotRecorded { actor: self.actor, message })
    }

    fn classification_of(&self, message: MessageId) -> Option<Classification> {
        classification_in(&self.state.history, message)
    }

    fn given_up_turns(&self) -> Vec<GivenUpTurn> {
        Vec::new()
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
        let view = InternalContext::of(actor, state.clone());

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
        let seen = InternalContext::of(actor, state.clone()).context_of(told.id).expect("resolved");
        assert!(matches!(&seen.items[0], ContextItem::Prompt { classification: Some(c), .. } if *c == confidential));

        // A reference to a prompt that was never kept cannot be resolved.
        let lost = text("lost");
        let gone = PromptRef { text: prompt("gone"), role: MessageRole::System, after: None, classification: None };
        state.history.push(lost.clone(), Some(ContextRef { transcript: before, prompts: vec![gone], tools: None, knowledge: Vec::new(), profile: Default::default(), turn_entries: 0 }));
        let view = InternalContext::of(actor, state);
        assert!(matches!(view.context_of(lost.id), Err(ContextError::Unavailable { .. })));
    }
}
