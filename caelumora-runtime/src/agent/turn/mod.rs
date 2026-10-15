//! The turn of an agent: what one cognitive cycle of an `AgentActor` works
//! with (see `docs/AGENT-TURN.md`).
//!
//! A [`Turn`] is the handle an agent's actor and its components work through:
//! on the agent's state in its channel, and while a turn runs on that turn's
//! working state, `AgentChannelState::turn` (issue 85) - the context and
//! prompt parts the model is given and where they came from, pending tool
//! calls and the plan, the changes not yet taken over; what it says goes
//! into the transcript and the history at once (issue 260) - and on
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
use crate::terminal::{Shell, Terminal};
use crate::runtime::ActorRuntime;
#[cfg(test)]
use crate::session::testing::detached_actor_runtime;
use crate::view::SessionView;
use crate::workspace::WorkspaceResult;
use crate::agent::work::AgentWork;
use anyhow::Result;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState};
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::turn::TurnState;
use caelumora_model::work::TurnId;
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::KnowledgeRef;
use caelumora_model::digest::Digest;
use caelumora_model::memory::{MemoryChange, MemoryOrigin};
use caelumora_model::message::Message;
use caelumora_model::note::NoteChange;
use caelumora_model::security::{Grant, SecurityContext};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::tool::Tool;
use caelumora_model::workspace::Workspace;
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
    /// The runtime of the agent's actor, in whose name the turn acts: what
    /// it asks of its channel - its workspace among it (issue 370) - and
    /// what it navigates (issue 163). Its own: the components the turn is
    /// given to see the turn's methods, never the runtime (issue 128).
    runtime: ActorRuntime,
}

impl<'a> Turn<'a> {
    /// The turn under way in the agent `agent_id`'s `agent_channel_state`,
    /// as a step of its run loop works with it; `None` between turns. A
    /// turn is begun on the state (`AgentChannelState::begin_turn`), and
    /// ended through its handle (`commit`, `give_up`). It acts through the
    /// actor's `runtime`.
    pub fn of(agent_id: AgentId, agent_name: String, agent_channel_state: &'a mut AgentChannelState, agent_state: Arc<AgentStateHandle>, runtime: ActorRuntime) -> Option<Self> {
        agent_channel_state.turn.as_ref()?;
        Some(Self { agent_id, agent_name, agent_channel_state, agent_state, runtime })
    }

    /// A turn begun on `agent_channel_state` with default contexts, no
    /// tools and the default profile, in a channel of no running session
    /// (`detached_actor_runtime`) - what a test needs to work in one.
    #[cfg(test)]
    pub(crate) fn begun(agent_id: AgentId, agent_name: String, agent_channel_state: &'a mut AgentChannelState, agent_state: Arc<AgentStateHandle>) -> Self {
        agent_channel_state.begin_turn(Default::default(), Default::default(), Default::default(), Vec::new(), Default::default());
        Self { agent_id, agent_name, agent_channel_state, agent_state, runtime: detached_actor_runtime() }
    }

    /// The turn as a piece of work of the agent, to read (issue 204): what
    /// components that only read are given, as an external agent's work
    /// gives it too.
    pub fn work(&self) -> AgentWork<'_> {
        AgentWork::new(self.agent_id, &self.current().work, self.agent_channel_state, &self.agent_state, &self.runtime)
    }

    /// The agent's session as the turn reads it from its channel
    /// (`AgentWork::session`).
    pub async fn session(&self) -> Option<SessionView> {
        self.work().session().await
    }

    /// The workspace of the agent's session and its terminal, for what
    /// `need` asks - reading its files, or changing them too -, if the turn
    /// may use it in its channel (`ActorRuntime::workspace_terminal`, issue
    /// 344): what a tool acts on.
    ///
    /// # Errors
    /// As `ActorRuntime::workspace_terminal`.
    pub async fn workspace_terminal(&self, need: Grant) -> WorkspaceResult<(Workspace, Arc<dyn Terminal>)> {
        self.runtime.workspace_terminal(self.security_context(), need).await
    }

    /// The workspace of the agent's session and its shell as its channel
    /// sees it (`ActorRuntime::workspace_shell`, issue 344): where a tool
    /// runs commands.
    ///
    /// # Errors
    /// As [`Self::workspace_terminal`].
    pub async fn workspace_shell(&self) -> WorkspaceResult<(Workspace, Option<Arc<dyn Shell>>)> {
        self.runtime.workspace_shell(self.security_context()).await
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

    /// The messages the turn said so far, as they stand (issue 260): as the
    /// history holds them, whatever a compaction left of them in the
    /// transcript (issue 284).
    pub fn messages(&self) -> Vec<Message> {
        self.agent_channel_state.history.said_in_turn(self.turn_id())
    }

    /// Gives the agent `message` to read in this turn (issue 22) - a skill a
    /// user loaded for it - as the knowledge `knowledge`: into its
    /// transcript and history at once, as what it was given, not what it
    /// said, and the knowledge recorded (`AgentChannelState::given`). Not
    /// published. Written at once, as what a turn says is (issue 260), so a
    /// component gives only once nothing can fail any more.
    pub fn give(&mut self, message: Message, knowledge: KnowledgeRef) {
        self.agent_channel_state.given(message, knowledge);
    }

    /// Replaces the agent's context in this turn (issue 16): its transcript
    /// begins anew with `replacement` - a summary, the skills given again,
    /// each with the knowledge it is, if it is one - and the entries from
    /// `keep_from` on as they stand; the old branch stays, the turn's prompt
    /// parts are carried over (`AgentChannelState::compact`), and a plan
    /// whose instructions were replaced is named again (`name_plan_again`,
    /// issue 284). Written at once, so a component compacts only once
    /// nothing can fail any more. Returns the heads of the transcript before
    /// and after, `None` if nothing was compacted.
    pub fn compact(&mut self, replacement: Vec<(Message, Option<KnowledgeRef>)>, keep_from: Option<Digest>) -> Option<(Digest, Digest)> {
        let replaced = replacement.len();
        let heads = self.agent_channel_state.compact(ActorId::Agent(self.agent_id), replacement, keep_from)?;
        let replacement_end = replaced.checked_sub(1).and_then(|last| self.agent_channel_state.transcript.entries().nth(last)).map(|node| node.digest);
        self.name_plan_again(replacement_end);
        Some(heads)
    }

    /// Records `message` as said by the turn: into the agent's transcript
    /// and history at once (`AgentChannelState::said`, issue 260).
    pub(crate) fn push_message(&mut self, message: Message) {
        self.agent_channel_state.said(message);
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

    /// Whether the turn may still try to compact its context before its next
    /// model call: it has not since its last (issue 284).
    pub(crate) fn may_compact(&self) -> bool {
        let turn = self.current();
        turn.compaction_tried_at != Some(turn.model_calls)
    }

    /// Records that the turn tries to compact its context before its next
    /// model call (`may_compact`).
    pub(crate) fn try_compaction(&mut self) {
        let turn = self.current_mut();
        turn.compaction_tried_at = Some(turn.model_calls);
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
        Answering::continuing(&self.current().work)
    }

    /// Sends `event` to the channel as the agent publishes it, in the
    /// turn's contexts (`Answering::send`): messages without what only the
    /// agent's owner sees (issues 54, 133). The agent keeps the full
    /// messages in its context; an event with nothing left to show is not
    /// sent. Only the actor holds `output`: what the turn says reaches the
    /// channel through it alone, never through a component the turn is
    /// handed to.
    pub(crate) async fn send_event(&self, event: ActorEvent, output: &dyn EventSender) -> Result<()> {
        self.answering().send(event, output).await
    }

    /// Says `message`: keeps it in the turn and publishes it, the one way
    /// a turn's complete message reaches the channel (issue 258) - the
    /// model's, a step's tool message, the one answering the calls the turn
    /// abandoned. Kept before it is sent, so that a failed send loses
    /// nothing of the turn.
    pub(crate) async fn say(&mut self, message: Message, output: &dyn EventSender) -> Result<()> {
        self.push_message(message.clone());
        self.send_event(event_for(message, false), output).await
    }

    /// Shows `partial`, a message still streaming in: published, not kept.
    pub(crate) async fn show(&self, partial: Message, output: &dyn EventSender) -> Result<()> {
        self.send_event(event_for(partial, true), output).await
    }
}

/// The event that publishes `message`: internal messages (plan steps)
/// are intermediate, the others what the channel shows as the answer;
/// `partial` for a streamed chunk.
fn event_for(message: Message, partial: bool) -> ActorEvent {
    match (message.internal, partial) {
        (true, true) => ActorEvent::IntermediatePartialGenerated(message),
        (false, true) => ActorEvent::PartialMessageGenerated(message),
        (true, false) => ActorEvent::IntermediateMessageGenerated(message),
        (false, false) => ActorEvent::CompleteMessageGenerated(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::{Envelope, EventContext};
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
        let mut turn = Turn::of(AgentId::default(), String::new(), &mut state, crate::agent::state::AgentStateHandle::detached(), detached_actor_runtime()).expect("a turn is under way");
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
        let mut turn = Turn::of(AgentId::new(), "agent".to_string(), &mut state, crate::agent::state::AgentStateHandle::detached(), detached_actor_runtime()).expect("a turn was begun");

        let call = ToolCall { id: Some("1".to_string()), function_name: "write_private_note".to_string(), arguments: serde_json::json!({"text": "secret"}), visibility: ToolVisibility::HIDDEN, presentation: Default::default() };
        let message = Message { role: MessageRole::Assistant, tool_calls: vec![call], ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) };
        turn.push_message(message.clone());
        turn.send_event(ActorEvent::CompleteMessageGenerated(message.clone()), &output).await.unwrap();
        turn.send_event(ActorEvent::TurnFinished { turn_id: caelumora_model::work::TurnId::new(), messages: vec![message] }, &output).await.unwrap();

        match rx.recv().await.map(|e| e.event) {
            Some(ActorEvent::TurnFinished { messages, .. }) => assert!(messages.is_empty(), "the hidden call is not published"),
            other => panic!("the complete message is not sent at all, got {other:?}"),
        }
        assert_eq!(turn.messages().first().map(|m| m.tool_calls.len()), Some(1), "the agent keeps it");

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
        let mut turn = Turn::of(AgentId::new(), "agent".to_string(), &mut state, crate::agent::state::AgentStateHandle::detached(), detached_actor_runtime()).expect("a turn was begun");
        assert_eq!(turn.label(), Classification::PUBLIC, "a public transcript");

        turn.set_context(vec![PromptPart { role: MessageRole::System, text: "Be brief.".to_string(), after: None, classification: Some(Classification::PUBLIC), source: None }], Vec::new());
        assert_eq!(turn.label(), Classification::PUBLIC);
        state.perceive(Vec::new(), vec![said(&internal, "Our numbers are good.")]);
        let mut turn = Turn::of(AgentId::new(), "agent".to_string(), &mut state, crate::agent::state::AgentStateHandle::detached(), detached_actor_runtime()).expect("a turn is under way");
        assert_eq!(turn.label(), internal, "what others said during the turn");
        turn.set_context(vec![PromptPart { role: MessageRole::System, text: "Be brief.".to_string(), after: None, classification: None, source: None }], Vec::new());
        assert_eq!(turn.label(), confidential, "a part that does not say counts as the channel");
        assert_eq!(turn.agent_channel_state().turn.as_ref().unwrap().work.context.prompts[0].classification, None, "and is recorded so");
    }
}
