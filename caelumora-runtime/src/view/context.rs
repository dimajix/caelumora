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
use caelumora_model::history::HistoryPage;
use caelumora_model::message::MessageId;
use caelumora_model::note::Note;
use caelumora_model::store::history::{every, HistoryKey, HistoryStore};
use caelumora_model::store::StoreError;
use caelumora_model::transcript::Transcript;
use std::sync::Arc;

/// The state of an agent in a channel, reached by whom may read what the
/// agent saw: from the store, where it is written after every step - the
/// running loop holds the in-memory state for as long as it runs. Its
/// history is read from the store in pages (issue 379), not from the part of
/// it the state holds.
#[derive(Debug, Clone)]
pub struct AgentContextView {
    actor: ActorId,
    state: AgentChannelState,
    histories: Arc<dyn HistoryStore>,
    /// The agent's history in the channel.
    history: HistoryKey,
}

/// What the tests of the agents' logic build a view with (issue 370).
#[cfg(any(test, feature = "test-support"))]
impl AgentContextView {
    /// The context of `actor`, an agent, as `state` holds it, reached by
    /// nobody - for a test that compares what an agent was given with what
    /// the view shows of it. Its history is put into a store of its own.
    ///
    /// # Panics
    /// If `actor` is no agent.
    pub async fn of_state(actor: ActorId, mut state: AgentChannelState) -> Self {
        use caelumora_model::channel::ChannelId;
        use caelumora_model::store::memory::MemoryStore;
        use caelumora_model::store::StateStore;
        let ActorId::Agent(agent) = actor else { panic!("{actor} is no agent") };
        let store = Arc::new(MemoryStore::new());
        let channel = ChannelId::default();
        store.put_agent_channel_state(agent, channel, &mut state).await.expect("a state is kept in memory");
        Self::of(actor, state, store, HistoryKey::Agent(agent, channel))
    }
}

impl AgentContextView {
    pub(crate) fn of(actor: ActorId, state: AgentChannelState, histories: Arc<dyn HistoryStore>, history: HistoryKey) -> Self {
        Self { actor, state, histories, history }
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
    /// prompts and tools are its own (issue 205). What it was produced from
    /// is read from the message's item in the agent's history.
    ///
    /// # Errors
    /// `NotRecorded` if the agent did not produce the message, or what it
    /// saw is not known here; `Unavailable` if what was recorded cannot be
    /// resolved any more; `Failed` if the history cannot be read.
    pub async fn context_of(&self, message: MessageId) -> Result<ResolvedContext, AgentContextError> {
        let said = self.histories.said(self.history, message).await.map_err(|source| self.failed(message, source))?;
        let (reference, turn) = said.as_ref().and_then(|(_, item)| item.context())
            .ok_or(AgentContextError::NotRecorded { actor: self.actor, message })?;
        ContextResolver::of(&self.state).resolve(reference, turn, self.actor).ok_or(AgentContextError::Unavailable { actor: self.actor, message })
    }

    /// The classification of the message `message` the agent produced, as
    /// its history holds it: the label it was written with (issue 168),
    /// unless it was reclassified since. `None` if the agent's history does
    /// not hold it.
    ///
    /// # Errors
    /// `Failed` if the history cannot be read.
    pub async fn classification_of(&self, message: MessageId) -> Result<Option<Classification>, AgentContextError> {
        let said = self.histories.said(self.history, message).await.map_err(|source| self.failed(message, source))?;
        Ok(said.and_then(|(_, item)| item.said().map(|m| m.classification.clone())))
    }

    /// A page of all that was offered to the agent in the channel and what
    /// it said itself (issues 207, 218), as the messages stand now, and how
    /// its turns ended (issue 258): the last `limit` items before the
    /// position `before` - the last ones without it -, and where the page
    /// before it ends. What of it each of its messages was produced from is
    /// that message's context.
    ///
    /// # Errors
    /// `Failed` if the history cannot be read.
    pub async fn history_page(&self, before: Option<u64>, limit: usize) -> Result<HistoryPage, AgentContextError> {
        self.histories.page(self.history, 0, before, limit, &every).await
            .map_err(|source| AgentContextError::Failed { actor: self.actor, message: None, source })
    }

    /// That the history could not be read for `message`.
    fn failed(&self, message: MessageId, source: StoreError) -> AgentContextError {
        AgentContextError::Failed { actor: self.actor, message: Some(message), source }
    }

    /// Whether the message `message` the agent heard was passed on to its
    /// transcript, on whichever branch - not if its perceive processors
    /// dropped it (issue 196). One they changed keeps its id and counts as
    /// passed on. Looked for from the end backwards, where the messages of a
    /// page of the history usually are. Whether what the agent saw when it
    /// produced a message can be shown its item says
    /// (`HistoryEntry::context`).
    pub fn in_transcript(&self, message: MessageId) -> bool {
        self.state.transcript.all_entries().rev().any(|n| n.entry.message().is_some_and(|m| m.id == message))
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
    /// The agent's history could not be read; the cause is in the log.
    #[error("Reading what {actor} saw failed")]
    Failed { actor: ActorId, message: Option<MessageId>, #[source] source: StoreError },
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::AgentId;
    use caelumora_model::context::{ContextItem, ContextRef, PromptRef};
    use caelumora_model::digest::prompt;
    use caelumora_model::history::HistoryEntry;
    use caelumora_model::message::Message;
    use caelumora_model::message::MessageRole;
    use caelumora_model::confidentiality::Confidentiality;
    use caelumora_model::work::TurnId;

    /// The items of the agent's history as the view reads them.
    async fn history(view: &AgentContextView) -> Vec<HistoryEntry> {
        view.history_page(None, 1000).await.expect("read").items
    }

    #[tokio::test]
    async fn a_turn_under_way_shows_what_it_said_with_what_it_was_produced_from() {
        let actor = ActorId::Agent(AgentId::new());
        let mut state = AgentChannelState::default();
        let question = Message { id: MessageId::new(), role: MessageRole::User, content: Some("question".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        state.perceive(vec![question.clone()], vec![question.clone()]);
        state.begin_turn(Default::default(), Default::default(), Classification::CHANNEL_DEFAULT, Vec::new(), Default::default());
        let step = Message { id: MessageId::new(), sender_id: actor, role: MessageRole::Assistant, content: Some("thinking".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        state.said(step.clone());

        // Before the turn ended (issue 260).
        let view = AgentContextView::of_state(actor, state).await;
        let items = history(&view).await;
        assert!(items.iter().any(|item| matches!(item, HistoryEntry::Said { message, .. } if message.id == step.id)), "in its history as it was said");
        assert!(items.iter().any(|item| item.context_of(step.id).is_some()) && view.in_transcript(step.id));
        let seen = view.context_of(step.id).await.expect("what it was produced from is recorded");
        assert!(seen.items.iter().any(|item| matches!(item, ContextItem::Conversation { message } if message.id == question.id)), "the question it answered");
    }

    #[tokio::test]
    async fn a_message_context_is_found_only_for_what_the_agent_produced_and_can_still_be_resolved() {
        let actor = ActorId::Agent(AgentId::new());
        let text = |t: &str| Message { id: MessageId::new(), role: MessageRole::User, content: Some(t.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        // What the agent produced, in a turn of its own.
        let produced = |t: &str| Message { turn_id: Some(TurnId::new()), ..text(t) };
        let mut state = AgentChannelState::default();
        let question = text("question");
        state.transcript.push(question.clone());
        state.history.extend(vec![question.clone()]);
        let answer = produced("answer");
        let before = state.transcript.push(answer.clone());
        state.history.push(answer.clone(), Some(ContextRef { transcript: before, prompts: Vec::new(), tools: None, knowledge: Vec::new(), profile: Default::default() }));
        let view = AgentContextView::of_state(actor, state.clone()).await;

        let seen = view.context_of(answer.id).await.expect("the agent produced it");
        let ids: Vec<MessageId> = seen.items.iter().filter_map(|item| match item {
            ContextItem::Conversation { message } => Some(message.id),
            ContextItem::Prompt { .. } => None,
        }).collect();
        assert_eq!(ids, vec![question.id]);
        assert!(matches!(view.context_of(question.id).await, Err(AgentContextError::NotRecorded { .. })), "someone else wrote it");
        assert_eq!(view.classification_of(answer.id).await.unwrap(), Some(Classification::CHANNEL_DEFAULT), "the label it was written with");
        assert_eq!(view.classification_of(MessageId::new()).await.unwrap(), None);
        let said = |items: Vec<HistoryEntry>| items.into_iter().filter_map(|item| match item {
            HistoryEntry::Said { message, .. } => Some(message.id),
            _ => None,
        }).collect::<Vec<MessageId>>();
        let items = history(&view).await;
        assert_eq!(said(items.clone()), vec![question.id, answer.id], "all it heard and said (issue 207)");
        let has_context = |id| items.iter().any(|item| item.context_of(id).is_some());
        assert!(has_context(answer.id) && !has_context(question.id));
        assert!(view.in_transcript(question.id));

        // What the perceive processors dropped stays in the history only.
        let mut heard_only = state.clone();
        let dropped = text("dropped");
        heard_only.perceive(vec![dropped.clone()], Vec::new());
        let view = AgentContextView::of_state(actor, heard_only).await;
        assert!(said(history(&view).await).contains(&dropped.id));
        assert!(!view.in_transcript(dropped.id) && view.in_transcript(answer.id));

        // A prompt part is told with how it was classified (issue 170).
        let confidential = Classification::general(Confidentiality::Confidential);
        let system = PromptRef { text: prompt("system"), role: MessageRole::System, after: None, classification: Some(confidential.clone()) };
        state.prompts.keep(system.text, || "system".to_string());
        let told = produced("told");
        state.history.push(told.clone(), Some(ContextRef { transcript: before, prompts: vec![system], tools: None, knowledge: Vec::new(), profile: Default::default() }));
        let seen = AgentContextView::of_state(actor, state.clone()).await.context_of(told.id).await.expect("resolved");
        assert!(matches!(&seen.items[0], ContextItem::Prompt { classification: Some(c), .. } if *c == confidential));

        // A reference to a prompt that was never kept cannot be resolved.
        let lost = produced("lost");
        let gone = PromptRef { text: prompt("gone"), role: MessageRole::System, after: None, classification: None };
        state.history.push(lost.clone(), Some(ContextRef { transcript: before, prompts: vec![gone], tools: None, knowledge: Vec::new(), profile: Default::default() }));
        let view = AgentContextView::of_state(actor, state).await;
        assert!(matches!(view.context_of(lost.id).await, Err(AgentContextError::Unavailable { .. })));
    }
}
