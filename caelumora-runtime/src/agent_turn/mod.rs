//! The turn of an agent: what one cognitive cycle of an `AgentActor` works
//! with (see `docs/AGENT-TURN.md`).
//!
//! A [`Turn`] is the handle an agent's actor and its components work through:
//! on the agent's state in its channel, and while a turn runs on that turn's
//! working state, `AgentChannelState::turn` (issue 85) - its messages, the
//! context and prompt parts the model is given and where they came from,
//! pending tool calls and the plan, the changes not yet taken over - and on
//! the agent's own state (issues 62, 65). What those states hold is the
//! business of wrappers such as `crate::agent_notes::AgentNotes` and
//! `crate::agent_memory::AgentMemory`. A turn is begun with `next_turn`.
//!
//! This file holds the handle and what components work with; the rest of a
//! turn's logic is in a file per subject:
//!
//! - `ending` - taking the turn over (`commit_turn`) and giving it up
//!   (`give_up_turn`, issue 145), and what is left of it then;
//! - `context` - what the model is given: the transcript, the prompt parts
//!   and the tools (issues 47, 49, 136, 137);
//! - `plan` - the plan in play and its progress;
//!
//! the bookkeeping of a step's tool calls is in `crate::tool_calls` (issue
//! 83). The `AgentActor` keeps what is input and output: the model call, the
//! tool executor, sending. What only the actor calls is `pub(crate)`; the
//! rest is the API components work with.
//!
//! Between turns there is no working state. What a `Turn` is asked about it
//! then reads as empty; nothing in the runtime asks - work between turns
//! (perceiving a message) touches only the transcript and the history.

mod context;
mod ending;
mod plan;

pub use ending::{CommittedTurn, EndedTurn};

use crate::agent_notes::NoteKind;
use crate::agent_state::AgentStateHandle;
use crate::tool_calls::ToolCalls;
use crate::transport::EventSender;
use crate::view::{SessionView, ViewsFactory};
use anyhow::{Context, Result};
use caelumora_model::actor::{ActorEvent, ActorState, Envelope, EventContext};
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::turn::TurnState;
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::{KnowledgeRef, ModelProfile};
use caelumora_model::memory::{MemoryChange, MemoryOrigin};
use caelumora_model::message::Message;
use caelumora_model::note::NoteChange;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::tool::Tool;
use std::sync::{Arc, LazyLock};

/// What a turn reads as when none runs.
static NO_TURN: LazyLock<TurnState> = LazyLock::new(TurnState::default);

/// How many times a turn may ask its model; then it ends with what it has.
pub(crate) const MAX_MODEL_CALLS: u32 = 25;

#[derive(Debug)]
pub struct Turn<'a> {
    agent_id: AgentId,
    agent_name: String,
    agent_channel_state: &'a mut AgentChannelState,
    /// The agent's own state, shared by all its actors (issue 62). The turn
    /// reads it and stages changes, never holds it.
    agent_state: Arc<AgentStateHandle>,
    /// How the agent's model is given its context (issues 49, 63): a
    /// property of that model (`LLMClient::profile`), fixed in the context
    /// of every turn that begins (`next_turn`).
    profile: ModelProfile,
    /// What the turn navigates (issue 163): the agent's session, read from
    /// its channel at the turn's classification. `None` for a turn outside
    /// of a channel - in tests - which reads nothing elsewhere.
    views: Option<ViewsFactory>,
}

impl<'a> Turn<'a> {
    /// The handle for one step of the agent `agent_id`'s run loop, whose
    /// model is given its context by `profile`.
    pub fn new(agent_id: AgentId, agent_name: String, agent_channel_state: &'a mut AgentChannelState, agent_state: Arc<AgentStateHandle>, profile: ModelProfile) -> Self {
        Self {
            agent_id,
            agent_name,
            agent_channel_state,
            agent_state,
            profile,
            views: None,
        }
    }

    /// With what the turn navigates: the agent's views.
    pub fn with_views(mut self, views: ViewsFactory) -> Self {
        self.views = Some(views);
        self
    }

    /// The agent's session as the turn reads it from its channel (issue
    /// 163): the root of what it may read elsewhere - the other channels'
    /// notes and histories, its own private notes there - at the
    /// classification the turn began with (issue 67). `None` for a turn
    /// outside of a channel, which reads nothing elsewhere.
    pub fn session(&self) -> Option<SessionView> {
        self.views.as_ref()?.session(self.channel_id(), self.classification())
    }

    /// The agent whose turn this is.
    pub fn agent_id(&self) -> AgentId {
        self.agent_id
    }

    /// The agent's name, as the channel knows it.
    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }

    /// The working state of the turn under way; an empty one between turns.
    fn current(&self) -> &TurnState {
        self.agent_channel_state.turn.as_ref().unwrap_or(&NO_TURN)
    }

    /// The working state of the turn under way, to change. Between turns
    /// nothing may change it: a test catches that; in a release build it
    /// starts from an empty turn rather than failing.
    fn current_mut(&mut self) -> &mut TurnState {
        debug_assert!(self.agent_channel_state.turn.is_some(), "a turn's working state changed while no turn runs");
        self.agent_channel_state.turn.get_or_insert_with(TurnState::default)
    }

    /// The agent's own state (issue 62): `current()` is a snapshot of it as
    /// it is now, without the changes of the current turn.
    pub fn agent_state(&self) -> &Arc<AgentStateHandle> {
        &self.agent_state
    }

    /// The classification of the agent's channel as it was when the turn
    /// began (`next_turn`): the most the turn may read, and the most it
    /// writes as.
    pub fn classification(&self) -> Classification {
        self.current().classification.clone()
    }

    /// The classification of what the turn writes (issue 168): the join of
    /// everything it has seen so far, at most `classification`.
    pub fn label(&self) -> Classification {
        self.current().label()
    }

    /// Records that the turn has seen what is classified `classification`:
    /// what it writes from now on is classified at least so (`label`).
    pub fn see(&mut self, classification: &Classification) {
        self.current_mut().see(classification);
    }

    /// The agent's channel: where the envelope the turn answers was routed
    /// - the `ChannelLoop` stamps it on every envelope it routes.
    pub fn channel_id(&self) -> ChannelId {
        self.current().event_context.channel_id
    }

    /// The session of the agent's channel, as `channel_id`.
    pub fn session_id(&self) -> SessionId {
        self.current().event_context.session_id
    }

    /// Stages `change` to the agent's memory, made at `origin`, applied when
    /// the turn completes (`commit_turn`).
    pub fn stage_memory(&mut self, change: MemoryChange, origin: MemoryOrigin) {
        self.current_mut().memory.push((change, origin));
    }

    /// The changes to the agent's memory staged in this turn so far, each
    /// with where it was made.
    pub fn staged_memory(&self) -> &[(MemoryChange, MemoryOrigin)] {
        &self.current().memory
    }

    /// Begins a turn on the agent's state (`AgentChannelState::begin_turn`),
    /// its model given its context as the turn's profile says.
    pub(crate) fn next_turn(
        &mut self,
        event_context: EventContext,
        security_context: SecurityContext,
        classification: Classification,
        tools: Vec<Tool>,
    ) {
        self.agent_channel_state.begin_turn(event_context, security_context, classification, tools, self.profile);
    }

    /// Begins a turn with default contexts and no tools - what a test needs
    /// to work in one.
    #[cfg(test)]
    pub(crate) fn begin(&mut self) {
        self.next_turn(EventContext::default(), SecurityContext::default(), Classification::default(), Vec::new());
    }

    /// The agent's tools as they were when the turn began: what its model
    /// is offered, and what its calls are labelled and put up for approval
    /// by (issue 137).
    pub(crate) fn tools(&self) -> &[Tool] {
        &self.current().tools
    }

    /// The event context of the current turn: carries the ids (`span_id`,
    /// `correlation_id`) that tie the turn to the envelope that started it.
    pub(crate) fn event_context(&self) -> &EventContext {
        &self.current().event_context
    }

    /// The messages the turn produced so far.
    pub fn messages(&self) -> impl Iterator<Item = &Message> {
        self.current().messages()
    }

    pub(crate) fn push_message(&mut self, message: Message) {
        self.current_mut().push(message);
    }

    /// Counts a call of the model, if the turn may still make one: `false`
    /// once it made `MAX_MODEL_CALLS`. The count is the turn's and is stored
    /// with it, a resumed turn goes on from where it was (issue 157).
    pub(crate) fn count_model_call(&mut self) -> bool {
        let turn = self.current_mut();
        if turn.model_calls >= MAX_MODEL_CALLS {
            return false;
        }
        turn.model_calls += 1;
        true
    }

    /// The tool calls of the current step (issue 83): a view that keeps
    /// their bookkeeping off `Turn`. Short-lived on purpose - running a call
    /// needs the whole turn in between.
    pub(crate) fn tool_calls(&mut self) -> ToolCalls<'_> {
        debug_assert!(self.agent_channel_state.turn.is_some(), "a turn's working state changed while no turn runs");
        let turn = self.agent_channel_state.turn.get_or_insert_with(TurnState::default);
        ToolCalls::new(turn, &self.agent_name)
    }

    pub fn security_context(&self) -> &SecurityContext {
        &self.current().security_context
    }

    /// Whether the turn was given or read private knowledge of the agent -
    /// its private notes (issue 51) or its memory (issue 62) - so far: what the model produces from
    /// then on may reveal it (issue 61).
    pub(crate) fn saw_private_knowledge(&self) -> bool {
        self.current().context.knowledge.iter()
            .any(|k| matches!(k.source,
                caelumora_model::context::KnowledgeSource::PrivateNotes { .. } | caelumora_model::context::KnowledgeSource::Memory { .. }))
    }

    /// Records that the turn read `reference` (issue 17): it is recorded
    /// with every message the turn produces from now on.
    pub fn record_knowledge(&mut self, reference: KnowledgeRef) {
        self.current_mut().record_knowledge(reference);
    }

    /// The agent's state in this channel as it stands: what components read
    /// their state from (issue 65). Changes the turn made count only once it
    /// completes.
    pub fn agent_channel_state(&self) -> &AgentChannelState {
        self.agent_channel_state
    }

    /// The agent's state in this channel, to store it: a store marks it
    /// stored (issue 193).
    pub(crate) fn agent_channel_state_mut(&mut self) -> &mut AgentChannelState {
        self.agent_channel_state
    }

    /// Writes where the actor stands into the state, before it is stored.
    pub(crate) fn set_processing_state(&mut self, state: ActorState) {
        self.agent_channel_state.processing_state = state;
    }

    /// Stages `change` to the agent's notes of `kind` in this channel,
    /// applied when the turn completes (`commit_turn`).
    pub fn stage_note(&mut self, kind: NoteKind, change: NoteChange) {
        kind.staged_mut(self.current_mut()).push(change);
    }

    /// The changes to the agent's notes of `kind` staged in this turn so
    /// far. A change taken back within the turn is taken back by another
    /// change, not dropped (issue 156).
    pub fn staged_notes(&self, kind: NoteKind) -> &[NoteChange] {
        kind.staged(self.current())
    }

    /// Records what state the turn should land on once `Reflecting` finishes,
    /// overriding the default `Idle` - and that the turn ends with the
    /// current tool step. This does not touch the live `ActorState` atomic
    /// directly; only the run loop does that.
    pub fn request_state(&mut self, state: ActorState) {
        self.current_mut().requested_state = Some(state);
    }

    /// The end-of-turn state a tool requested, if any.
    pub fn requested_state(&self) -> Option<ActorState> {
        self.current().requested_state
    }

    /// Sends `av` to the channel as the agent publishes it: messages without
    /// what only the agent's owner sees (issues 54, 133), in the turn's
    /// contexts. The agent keeps the full
    /// messages in its context; an event with nothing left to show is not
    /// sent.
    pub async fn send_event(&self, av: ActorEvent, output: &dyn EventSender) -> Result<()> {
        let turn = self.current();
        send(&turn.event_context, &turn.security_context, av, output).await
    }
}

/// Sends `av` as a turn publishes it: without what only its owner sees, in
/// the turn's contexts - sent by its agent.
async fn send(
    event_context: &EventContext,
    security_context: &SecurityContext,
    av: ActorEvent,
    output: &dyn EventSender,
) -> Result<()> {
    let Some(av) = av.published() else { return Ok(()) };
    let ev = Envelope::new(security_context.clone(), event_context.clone(), av);
    output
        .send(ev)
        .await
        .context(format!("Actor {} failed to send event", event_context.sender_id))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::ActorId;
    use caelumora_model::confidentiality::Confidentiality;
    use caelumora_model::context::{ContextItem, ContextResolver, PromptPart};
    use caelumora_model::message::MessageId;
    use caelumora_model::message::MessageRole;

    pub(super) fn message(content: &str) -> Message {
        Message {
            id: MessageId::new(),
            role: MessageRole::User,
            content: Some(content.to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        }
    }

    fn message_classified(classification: &Classification) -> Message {
        Message { id: MessageId::new(), role: MessageRole::User, ..Message::new(classification.clone()) }
    }

    /// What `state` says its agent `agent` saw when it produced message `id`.
    pub(super) fn seen(state: &AgentChannelState, agent: AgentId, id: MessageId) -> Option<Vec<ContextItem>> {
        let reference = state.recorded_context_of(id)?;
        Some(ContextResolver::of(state).resolve(&reference, ActorId::Agent(agent))?.items)
    }

    #[test]
    fn a_resumed_turn_keeps_its_count_of_model_calls() {
        let mut state = AgentChannelState::default();
        let mut turn = Turn::new(AgentId::default(), String::new(), &mut state, crate::agent_state::AgentStateHandle::detached(), Default::default());
        turn.begin();
        for _ in 1..MAX_MODEL_CALLS {
            assert!(turn.count_model_call());
        }

        // Stored and loaded again, as after a restart.
        let stored = serde_json::to_value(&state).unwrap();
        let mut state: AgentChannelState = serde_json::from_value(stored).unwrap();
        let mut turn = Turn::new(AgentId::default(), String::new(), &mut state, crate::agent_state::AgentStateHandle::detached(), Default::default());
        assert!(turn.count_model_call(), "the last one it may make");
        assert!(!turn.count_model_call(), "the limit holds across the restart");

        let mut stored = serde_json::to_value(&state).unwrap();
        stored["turn"].as_object_mut().unwrap().remove("model_calls");
        let state: AgentChannelState = serde_json::from_value(stored).unwrap();
        assert_eq!(state.turn.unwrap().model_calls, 0, "a turn stored before the count was kept starts from 0");
    }

    #[tokio::test]
    async fn events_leave_the_agent_redacted_while_its_context_keeps_the_full_call() {
        use caelumora_model::confidentiality::Visibility;
        use caelumora_model::tool::{ToolCall, ToolVisibility};
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let output = crate::transport_impl::memory::InMemoryEventSender::new(tx);
        let mut state = AgentChannelState::default();
        let mut turn = Turn::new(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached(), Default::default());
        turn.next_turn(Default::default(), Default::default(), Default::default(), Vec::new());

        let call = ToolCall { id: Some("1".to_string()), function_name: "note_private".to_string(), arguments: serde_json::json!({"text": "secret"}), visibility: ToolVisibility::HIDDEN };
        let message = Message { role: MessageRole::Assistant, tool_calls: vec![call], ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) };
        turn.push_message(message.clone());
        turn.send_event(ActorEvent::CompleteMessageGenerated(message.clone()), &output).await.unwrap();
        turn.send_event(ActorEvent::FullResponseGenerated(vec![message]), &output).await.unwrap();

        match rx.recv().await.map(|e| e.event) {
            Some(ActorEvent::FullResponseGenerated(messages)) => assert!(messages.is_empty(), "the hidden call is not published"),
            other => panic!("the complete message is not sent at all, got {other:?}"),
        }
        assert_eq!(turn.messages().next().map(|m| m.tool_calls.len()), Some(1), "the agent keeps it");

        // Reasoning reaches the channel unless it is the owner's.
        let thinking = Message {
            role: MessageRole::Assistant,
            content: Some("done".to_string()),
            reasoning: Some("weighing".to_string()),
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        turn.send_event(ActorEvent::CompleteMessageGenerated(thinking.clone()), &output).await.unwrap();
        let owners = Message { reasoning_visibility: Visibility::Owner, ..thinking };
        turn.send_event(ActorEvent::CompleteMessageGenerated(owners), &output).await.unwrap();
        let reasoning = |event: Option<Envelope>| match event.map(|e| e.event) {
            Some(ActorEvent::CompleteMessageGenerated(m)) => m.reasoning,
            other => panic!("expected a message, got {other:?}"),
        };
        assert_eq!(reasoning(rx.recv().await).as_deref(), Some("weighing"), "the channel's");
        assert!(reasoning(rx.recv().await).is_none(), "the owner's is not published");
    }

    #[test]
    fn a_turn_has_seen_its_transcript_its_prompt_parts_and_its_tool_results() {
        let (internal, confidential) = (Classification::general(Confidentiality::Internal), Classification::general(Confidentiality::Confidential));
        let said = |classification: &Classification, content: &str| Message { content: Some(content.to_string()), ..message_classified(classification) };
        let mut state = AgentChannelState::default();
        state.transcript.extend(vec![said(&Classification::PUBLIC, "Hello.")]);
        let mut turn = Turn::new(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached(), Default::default());
        turn.next_turn(EventContext::default(), SecurityContext::default(), confidential.clone(), Vec::new());
        assert_eq!(turn.label(), Classification::PUBLIC, "a public transcript");

        turn.set_context(vec![PromptPart { role: MessageRole::System, text: "Be brief.".to_string(), after: None, classification: Some(Classification::PUBLIC) }]);
        assert_eq!(turn.label(), Classification::PUBLIC);
        turn.agent_channel_state_mut().perceive(Vec::new(), vec![said(&internal, "Our numbers are good.")]);
        assert_eq!(turn.label(), internal, "what others said during the turn");
        turn.set_context(vec![PromptPart { role: MessageRole::System, text: "Be brief.".to_string(), after: None, classification: None }]);
        assert_eq!(turn.label(), confidential, "a part that does not say counts as the channel");
        assert_eq!(turn.agent_channel_state().turn.as_ref().unwrap().context.prompts[0].classification, None, "and is recorded so");
    }
}
