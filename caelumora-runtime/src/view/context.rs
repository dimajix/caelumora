//! What an agent saw in a channel (issue 41): its state there, as stored,
//! for a viewer who may read its context (`ReadAgentContext`). Agents run
//! here and external agents (issue 179) keep the same state (issue 206) and
//! are read by the same view; what an external agent cannot have - notes,
//! given-up turns - its state holds nothing of, and the context of one of
//! its answers is what the remote agent was given in the call that produced
//! it (issue 205).

use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentChannelState;
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
    state: AgentChannelState,
}

impl AgentContextView {
    pub(crate) fn of(actor: ActorId, state: AgentChannelState) -> Self {
        Self { actor, state }
    }

    /// The agent whose state this is.
    pub fn actor(&self) -> ActorId {
        self.actor
    }

    /// What the agent is given of the conversation: what its model is
    /// given (issue 50), with the turns it gave up as branches (issue 145);
    /// of an external agent, what the remote agent is given with the next
    /// call, with what each call before gave it as a branch (issue 205).
    pub fn transcript(&self) -> &Transcript {
        &self.state.transcript
    }

    /// The agent's private notes in the channel (issue 51), with the digest
    /// of their chain.
    pub fn private_notes(&self) -> (Vec<Note>, Option<Digest>) {
        (self.state.private_notes.current(), self.state.private_notes.digest())
    }

    /// What the agent's model saw when it produced the message `message`:
    /// its context and system prompt (issue 06). Of an external agent, the
    /// messages the remote agent was given in the call that produced it,
    /// and nothing else: it keeps what it was given before itself, and its
    /// prompts and tools are its own (issue 205).
    ///
    /// # Errors
    /// `NotRecorded` if the agent did not produce the message, or what it
    /// saw is not known here; `Unavailable` if what was recorded cannot be
    /// resolved any more.
    pub fn context_of(&self, message: MessageId) -> Result<ResolvedContext, ContextError> {
        let reference = self.state.recorded_context_of(message)
            .ok_or(ContextError::NotRecorded { actor: self.actor, message })?;
        ContextResolver::of(&self.state).resolve(&reference, self.actor).ok_or(ContextError::Unavailable { actor: self.actor, message })
    }

    /// The classification of the message `message` the agent produced, as
    /// its history holds it: the label it was written with (issue 168),
    /// unless it was reclassified since. `None` if the agent's history does
    /// not hold it.
    pub fn classification_of(&self, message: MessageId) -> Option<Classification> {
        self.state.history.message(message).map(|m| m.classification)
    }

    /// The turns the agent gave up in the channel, with their messages
    /// (issue 145).
    pub fn given_up_turns(&self) -> Vec<GivenUpTurn> {
        self.state.transcript.given_up_turns()
    }
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
        state.prompts.keep(system.text, || "system".to_string());
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
