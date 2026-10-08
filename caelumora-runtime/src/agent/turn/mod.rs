//! The turn of an agent: what one cognitive cycle of an `AgentActor` works
//! with (see `docs/AGENT-TURN.md`).
//!
//! A [`Turn`] is the handle an agent's actor and its components work through:
//! on the agent's state in its channel, and while a turn runs on that turn's
//! working state, `AgentChannelState::turn` (issue 85) - its messages, the
//! context and prompt parts the model is given and where they came from,
//! pending tool calls and the plan, the changes not yet taken over - and on
//! the agent's own state (issues 62, 65). What those states hold is the
//! business of wrappers such as `crate::agent::notes::AgentNotes` and
//! `crate::agent::memory::AgentMemory`. A turn is begun on the agent's state
//! (`AgentChannelState::begin_turn`); a `Turn` is had only while one is
//! under way (`Turn::of`), and ending it uses it up.
//!
//! This file holds the handle and what components work with; the rest of a
//! turn's logic is in a file per subject:
//!
//! - `ending` - taking the turn over (`commit`) and giving it up (`give_up`,
//!   issue 145), and what is left of it then;
//! - `context` - what the model is given: the transcript, the prompt parts
//!   and the tools (issues 47, 49, 136, 137);
//! - `plan` - the plan in play and its progress;
//!
//! the bookkeeping of a step's tool calls is in `tool_calls` (issue 83).
//! The `AgentActor` keeps what is input and output: the model call, the
//! tool executor, sending. What only the actor calls is `pub(crate)`; the
//! rest is the API components work with.
//!
//! Between turns the actor works on its `AgentChannelState` itself:
//! perceiving, catching up and storing need no turn.

mod context;
mod ending;
mod plan;
mod tool_calls;

pub(crate) use ending::CommittedTurn;

use crate::actor::Answering;
use crate::agent::notes::NoteKind;
use crate::agent::state::AgentStateHandle;
use tool_calls::ToolCalls;
use crate::transport::EventSender;
use crate::view::{SessionView, ViewsFactory};
use crate::agent::work::AgentWork;
use anyhow::Result;
use caelumora_model::actor::{ActorEvent, ActorState};
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::turn::TurnState;
use caelumora_model::work::TurnId;
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::KnowledgeRef;
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
    /// What the turn navigates (issue 163): the agent's session, read from
    /// its channel at the turn's classification. `None` for a turn outside
    /// of a channel - in tests - which reads nothing elsewhere.
    views: Option<ViewsFactory>,
}

impl<'a> Turn<'a> {
    /// The turn under way in the agent `agent_id`'s `agent_channel_state`,
    /// as a step of its run loop works with it; `None` between turns. A
    /// turn is begun on the state (`AgentChannelState::begin_turn`), and
    /// ended through its handle (`commit`, `give_up`).
    pub fn of(agent_id: AgentId, agent_name: String, agent_channel_state: &'a mut AgentChannelState, agent_state: Arc<AgentStateHandle>) -> Option<Self> {
        agent_channel_state.turn.as_ref()?;
        Some(Self {
            agent_id,
            agent_name,
            agent_channel_state,
            agent_state,
            views: None,
        })
    }

    /// A turn begun on `agent_channel_state` with default contexts, no
    /// tools and the default profile - what a test needs to work in one.
    #[cfg(test)]
    pub(crate) fn begun(agent_id: AgentId, agent_name: String, agent_channel_state: &'a mut AgentChannelState, agent_state: Arc<AgentStateHandle>) -> Self {
        agent_channel_state.begin_turn(Default::default(), Default::default(), Default::default(), Vec::new(), Default::default());
        Self { agent_id, agent_name, agent_channel_state, agent_state, views: None }
    }

    /// With what the turn navigates: the agent's views.
    pub fn with_views(mut self, views: ViewsFactory) -> Self {
        self.views = Some(views);
        self
    }

    /// The turn as a piece of work of the agent, to read (issue 204): what
    /// components that only read are given, as an external agent's work
    /// gives it too.
    pub fn work(&self) -> AgentWork<'_> {
        AgentWork::new(self.agent_id, &self.current().work, self.agent_channel_state, &self.agent_state, self.views.as_ref())
    }

    /// The agent's session as the turn reads it from its channel
    /// (`AgentWork::session`).
    pub fn session(&self) -> Option<SessionView> {
        self.work().session()
    }

    /// Which turn this is (issue 235).
    pub fn turn_id(&self) -> TurnId {
        self.current().work.turn_id
    }

    /// The agent whose turn this is.
    pub fn agent_id(&self) -> AgentId {
        self.agent_id
    }

    /// The agent's name, as the channel knows it.
    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }

    /// The working state of the turn under way. A `Turn` is only had while
    /// one is (`of`); the empty one it falls back to cannot be reached.
    fn current(&self) -> &TurnState {
        self.agent_channel_state.turn.as_ref().unwrap_or(&NO_TURN)
    }

    /// The working state of the turn under way, to change. That there is
    /// one is checked in tests; in a release build it would start from an
    /// empty turn rather than fail.
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
    /// began (`AgentChannelState::begin_turn`, `AgentWork::classification`).
    pub fn classification(&self) -> Classification {
        self.work().classification()
    }

    /// The classification of what the turn writes (`AgentWork::label`).
    pub fn label(&self) -> Classification {
        self.work().label()
    }

    /// Records that the turn has seen what is classified `classification`:
    /// what it writes from now on is classified at least so (`label`).
    pub fn see(&mut self, classification: &Classification) {
        self.current_mut().work.see(classification);
    }

    /// The agent's channel (`AgentWork::channel_id`).
    pub fn channel_id(&self) -> ChannelId {
        self.work().channel_id()
    }

    /// The session of the agent's channel (`AgentWork::session_id`).
    pub fn session_id(&self) -> SessionId {
        self.work().session_id()
    }

    /// Stages `change` to the agent's memory, made at `origin`, applied when
    /// the turn completes (`commit`).
    pub fn stage_memory(&mut self, change: MemoryChange, origin: MemoryOrigin) {
        self.current_mut().memory.push((change, origin));
    }

    /// The changes to the agent's memory staged in this turn so far, each
    /// with where it was made.
    pub fn staged_memory(&self) -> &[(MemoryChange, MemoryOrigin)] {
        &self.current().memory
    }

    /// The agent's tools as they were when the turn began: what its model
    /// is offered, and what its calls are labelled and put up for approval
    /// by (issue 137).
    pub(crate) fn tools(&self) -> &[Tool] {
        &self.current().tools
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

    /// Whose authority the turn acts with (`AgentWork::security_context`).
    pub fn security_context(&self) -> &SecurityContext {
        self.work().security_context()
    }

    /// Whether the turn was given or read private knowledge of the agent -
    /// its private notes (issue 51) or its memory (issue 62) - so far: what the model produces from
    /// then on may reveal it (issue 61).
    pub(crate) fn saw_private_knowledge(&self) -> bool {
        self.current().work.context.knowledge.iter()
            .any(|k| matches!(k.source,
                caelumora_model::context::KnowledgeSource::PrivateNotes { .. } | caelumora_model::context::KnowledgeSource::Memory { .. }))
    }

    /// Records that the turn read `reference` (issue 17): it is recorded
    /// with every message the turn produces from now on.
    pub fn record_knowledge(&mut self, reference: KnowledgeRef) {
        self.current_mut().work.record_knowledge(reference);
    }

    /// The agent's state in this channel as it stands: what components read
    /// their state from (issue 65). Changes the turn made count only once it
    /// completes.
    pub fn agent_channel_state(&self) -> &AgentChannelState {
        self.agent_channel_state
    }

    /// Stages `change` to the agent's notes of `kind` in this channel,
    /// applied when the turn completes (`commit`).
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

    /// The contexts the turn publishes in: those it began with.
    pub(crate) fn answering(&self) -> Answering {
        let turn = self.current();
        Answering { event_context: turn.work.event_context.clone(), security_context: turn.work.security_context.clone() }
    }

    /// Sends `event` to the channel as the agent publishes it, in the
    /// turn's contexts (`Answering::send`): messages without what only the
    /// agent's owner sees (issues 54, 133). The agent keeps the full
    /// messages in its context; an event with nothing left to show is not
    /// sent.
    pub async fn send_event(&self, event: ActorEvent, output: &dyn EventSender) -> Result<()> {
        self.answering().send(event, output).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::{Envelope, EventContext};
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
        let (reference, turn) = state.recorded_context_of(id)?;
        Some(ContextResolver::of(state).resolve(&reference, turn, ActorId::Agent(agent))?.items)
    }

    #[test]
    fn a_resumed_turn_keeps_its_count_of_model_calls() {
        let mut state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), String::new(), &mut state, crate::agent::state::AgentStateHandle::detached());
        for _ in 1..MAX_MODEL_CALLS {
            assert!(turn.count_model_call());
        }

        // Stored and loaded again, as after a restart.
        let stored = serde_json::to_value(&state).unwrap();
        let mut state: AgentChannelState = serde_json::from_value(stored).unwrap();
        let mut turn = Turn::of(AgentId::default(), String::new(), &mut state, crate::agent::state::AgentStateHandle::detached()).expect("a turn is under way");
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
        let output = crate::transport::memory::InMemoryEventSender::new(tx);
        let mut state = AgentChannelState::default();
        state.begin_turn(Default::default(), Default::default(), Default::default(), Vec::new(), Default::default());
        let mut turn = Turn::of(AgentId::new(), "agent".to_string(), &mut state, crate::agent::state::AgentStateHandle::detached()).expect("a turn was begun");

        let call = ToolCall { id: Some("1".to_string()), function_name: "note_private".to_string(), arguments: serde_json::json!({"text": "secret"}), visibility: ToolVisibility::HIDDEN, presentation: Default::default() };
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
        state.begin_turn(EventContext::default(), SecurityContext::default(), confidential.clone(), Vec::new(), Default::default());
        let mut turn = Turn::of(AgentId::new(), "agent".to_string(), &mut state, crate::agent::state::AgentStateHandle::detached()).expect("a turn was begun");
        assert_eq!(turn.label(), Classification::PUBLIC, "a public transcript");

        turn.set_context(vec![PromptPart { role: MessageRole::System, text: "Be brief.".to_string(), after: None, classification: Some(Classification::PUBLIC), source: None }]);
        assert_eq!(turn.label(), Classification::PUBLIC);
        state.perceive(Vec::new(), vec![said(&internal, "Our numbers are good.")]);
        let mut turn = Turn::of(AgentId::new(), "agent".to_string(), &mut state, crate::agent::state::AgentStateHandle::detached()).expect("a turn is under way");
        assert_eq!(turn.label(), internal, "what others said during the turn");
        turn.set_context(vec![PromptPart { role: MessageRole::System, text: "Be brief.".to_string(), after: None, classification: None, source: None }]);
        assert_eq!(turn.label(), confidential, "a part that does not say counts as the channel");
        assert_eq!(turn.agent_channel_state().turn.as_ref().unwrap().work.context.prompts[0].classification, None, "and is recorded so");
    }
}
