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
use caelumora_model::history::HistoryEntry;
use caelumora_model::message::MessageId;
use caelumora_model::note::Note;
use caelumora_model::transcript::Transcript;

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
    pub fn context_of(&self, message: MessageId) -> Result<ResolvedContext, AgentContextError> {
        let (reference, turn) = self.state.recorded_context_of(message)
            .ok_or(AgentContextError::NotRecorded { actor: self.actor, message })?;
        ContextResolver::of(&self.state).resolve(&reference, turn, self.actor).ok_or(AgentContextError::Unavailable { actor: self.actor, message })
    }

    /// The classification of the message `message` the agent produced, as
    /// its history holds it: the label it was written with (issue 168),
    /// unless it was reclassified since. `None` if the agent's history does
    /// not hold it.
    pub fn classification_of(&self, message: MessageId) -> Option<Classification> {
        self.state.history.message(message).map(|m| m.classification)
    }

    /// All that was offered to the agent in the channel and what it said
    /// itself (issue 207), as the messages stand now, and how its turns
    /// ended (issue 258): the conversation as it heard it, for the agent as
    /// a whole - what of it each of its messages was produced from is that
    /// message's context.
    pub fn history(&self) -> Vec<HistoryEntry> {
        self.state.history.items()
    }

    /// Whether the agent produced the message `message` and what it saw
    /// then is recorded - whether `context_of` can tell it.
    pub fn has_context(&self, message: MessageId) -> bool {
        self.state.history.all_entries().any(|e| e.entry.context_of(message).is_some())
    }

    /// Whether the message `message` the agent heard was passed on to its
    /// transcript, on whichever branch - not if its perceive processors
    /// dropped it (issue 196). One they changed keeps its id and counts as
    /// passed on.
    pub fn in_transcript(&self, message: MessageId) -> bool {
        self.state.transcript.all_entries().any(|n| n.entry.message().is_some_and(|m| m.id == message))
    }

}

/// Why a message's context cannot be shown. The messages are meant for
/// users (see `crate::error::user_message`).
#[derive(Debug, thiserror::Error)]
pub enum AgentContextError {
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
    use caelumora_model::message::Message;
    use caelumora_model::message::MessageRole;

    #[test]
    fn a_turn_under_way_shows_what_it_said_with_what_it_was_produced_from() {
        let actor = ActorId::Agent(AgentId::new());
        let mut state = AgentChannelState::default();
        let question = Message { id: MessageId::new(), role: MessageRole::User, content: Some("question".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        state.perceive(vec![question.clone()], vec![question.clone()]);
        state.begin_turn(Default::default(), Default::default(), Classification::CHANNEL_DEFAULT, Vec::new(), Default::default());
        let step = Message { id: MessageId::new(), sender_id: actor, role: MessageRole::Assistant, content: Some("thinking".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        state.said(step.clone());

        // Before the turn ended (issue 260).
        let view = AgentContextView::of(actor, state);
        assert!(view.history().iter().any(|item| matches!(item, HistoryEntry::Said { message, .. } if message.id == step.id)), "in its history as it was said");
        assert!(view.has_context(step.id) && view.in_transcript(step.id));
        let seen = view.context_of(step.id).expect("what it was produced from is recorded");
        assert!(seen.items.iter().any(|item| matches!(item, ContextItem::Conversation { message } if message.id == question.id)), "the question it answered");
    }

    #[test]
    fn a_message_context_is_found_only_for_what_the_agent_produced_and_can_still_be_resolved() {
        let actor = ActorId::Agent(AgentId::new());
        let text = |t: &str| Message { id: MessageId::new(), role: MessageRole::User, content: Some(t.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        // What the agent produced, in a turn of its own.
        let produced = |t: &str| Message { turn_id: Some(caelumora_model::work::TurnId::new()), ..text(t) };
        let mut state = AgentChannelState::default();
        let question = text("question");
        state.transcript.push(question.clone());
        state.history.extend(vec![question.clone()]);
        let answer = produced("answer");
        let before = state.transcript.push(answer.clone());
        state.history.push(answer.clone(), Some(ContextRef { transcript: before, prompts: Vec::new(), tools: None, knowledge: Vec::new(), profile: Default::default() }));
        let view = AgentContextView::of(actor, state.clone());

        let seen = view.context_of(answer.id).expect("the agent produced it");
        let ids: Vec<MessageId> = seen.items.iter().filter_map(|item| match item {
            ContextItem::Conversation { message } => Some(message.id),
            ContextItem::Prompt { .. } => None,
        }).collect();
        assert_eq!(ids, vec![question.id]);
        assert!(matches!(view.context_of(question.id), Err(AgentContextError::NotRecorded { .. })), "someone else wrote it");
        assert_eq!(view.classification_of(answer.id), Some(Classification::CHANNEL_DEFAULT), "the label it was written with");
        assert_eq!(view.classification_of(MessageId::new()), None);
        let said = |items: Vec<HistoryEntry>| items.into_iter().filter_map(|item| match item {
            HistoryEntry::Said { message, .. } => Some(message.id),
            _ => None,
        }).collect::<Vec<MessageId>>();
        let heard = said(view.history());
        assert_eq!(heard, vec![question.id, answer.id], "all it heard and said (issue 207)");
        assert!(view.has_context(answer.id) && !view.has_context(question.id));
        assert!(view.in_transcript(question.id));

        // What the perceive processors dropped stays in the history only.
        let mut heard_only = state.clone();
        let dropped = text("dropped");
        heard_only.perceive(vec![dropped.clone()], Vec::new());
        let view = AgentContextView::of(actor, heard_only);
        assert!(said(view.history()).contains(&dropped.id));
        assert!(!view.in_transcript(dropped.id) && view.in_transcript(answer.id));

        // A prompt part is told with how it was classified (issue 170).
        let confidential = Classification::general(caelumora_model::confidentiality::Confidentiality::Confidential);
        let system = PromptRef { text: prompt("system"), role: MessageRole::System, after: None, classification: Some(confidential.clone()) };
        state.prompts.keep(system.text, || "system".to_string());
        let told = produced("told");
        state.history.push(told.clone(), Some(ContextRef { transcript: before, prompts: vec![system], tools: None, knowledge: Vec::new(), profile: Default::default() }));
        let seen = AgentContextView::of(actor, state.clone()).context_of(told.id).expect("resolved");
        assert!(matches!(&seen.items[0], ContextItem::Prompt { classification: Some(c), .. } if *c == confidential));

        // A reference to a prompt that was never kept cannot be resolved.
        let lost = produced("lost");
        let gone = PromptRef { text: prompt("gone"), role: MessageRole::System, after: None, classification: None };
        state.history.push(lost.clone(), Some(ContextRef { transcript: before, prompts: vec![gone], tools: None, knowledge: Vec::new(), profile: Default::default() }));
        let view = AgentContextView::of(actor, state);
        assert!(matches!(view.context_of(lost.id), Err(AgentContextError::Unavailable { .. })));
    }
}
