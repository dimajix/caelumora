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
//! business of wrappers such as `crate::notes::AgentNotes` and
//! `crate::memory::AgentMemory`. A turn is begun on the agent's state
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

use caelumora_runtime::actor::Answering;
use crate::notes::NoteKind;
use caelumora_runtime::agent::state::AgentStateHandle;
use tool_calls::ToolCalls;
use caelumora_runtime::transport::EventSender;
use caelumora_runtime::terminal::{Shell, Terminal};
use caelumora_runtime::runtime::ActorRuntime;
#[cfg(test)]
use crate::testing::detached_actor_runtime;
use caelumora_runtime::workspace::WorkspaceResult;
use crate::situation::Situation;
use anyhow::Result;
use caelumora_model::prompt_catalogue::PromptError;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState};
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::turn::TurnState;
use caelumora_model::work::TurnId;
#[cfg(test)]
use caelumora_model::work::Work;
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::digest::Digest;
use caelumora_model::memory::{MemoryChange, MemoryOrigin};
use caelumora_model::content::ContentRef;
use caelumora_model::message::{Message, MessageRole};
use caelumora_model::note::NoteChange;
use caelumora_model::security::Grant;
use caelumora_model::tool::Tool;
use caelumora_model::workspace::Workspace;
use std::sync::{Arc, LazyLock};

/// What a turn reads as when none runs.
static NO_TURN: LazyLock<TurnState> = LazyLock::new(TurnState::default);

/// How many times a turn, or a step of its plan, may ask its model; then it
/// is given up (issues 411, 412).
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
    /// tools and the default profile, as `under_way` - what a test needs to
    /// work in one.
    #[cfg(test)]
    pub(crate) fn begun(agent_id: AgentId, agent_channel_state: &'a mut AgentChannelState) -> Self {
        agent_channel_state.begin_turn(Work::new(Default::default(), None, Default::default(), Default::default(), Default::default()));
        Self::under_way(agent_id, agent_channel_state)
    }

    /// The turn a test began on `agent_channel_state`, of the agent
    /// `agent_id` called "agent", with a state of its own
    /// (`AgentStateHandle::detached`), in a channel of no running session
    /// (`detached_actor_runtime`).
    ///
    /// # Panics
    /// If no turn is under way.
    #[cfg(test)]
    pub(crate) fn under_way(agent_id: AgentId, agent_channel_state: &'a mut AgentChannelState) -> Self {
        Self::of(agent_id, "agent".to_string(), agent_channel_state, AgentStateHandle::detached(), detached_actor_runtime()).expect("a turn is under way")
    }

    /// The situation of the turn, to read (issues 204, 399): what the turn
    /// reads through - who works, where, in whose name, bound by what - and
    /// what components that only read are given, as an external agent's
    /// work gives it too.
    pub fn situation(&self) -> Situation<'_> {
        Situation::new(self.agent_id, &self.current().work, self.agent_channel_state, &self.agent_state, &self.runtime)
    }

    /// The workspace of the agent's session and its terminal, for what
    /// `need` asks - reading its files, or changing them too -, if the turn
    /// may use it in its channel (`ActorRuntime::workspace_terminal`, issue
    /// 344): what a tool acts on.
    ///
    /// # Errors
    /// As `ActorRuntime::workspace_terminal`.
    pub async fn workspace_terminal(&self, need: Grant) -> WorkspaceResult<(Workspace, Arc<dyn Terminal>)> {
        self.runtime.workspace_terminal(self.situation().security_context(), need).await
    }

    /// The workspace of the agent's session and its shell as its channel
    /// sees it (`ActorRuntime::workspace_shell`, issue 344): where a tool
    /// runs commands.
    ///
    /// # Errors
    /// As [`Self::workspace_terminal`].
    pub async fn workspace_shell(&self) -> WorkspaceResult<(Workspace, Option<Arc<dyn Shell>>)> {
        self.runtime.workspace_shell(self.situation().security_context()).await
    }

    /// Which turn this is (issue 235).
    pub fn turn_id(&self) -> TurnId {
        self.current().work.turn_id
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

    /// Records that the turn has seen what is classified `classification`:
    /// what it writes from now on is classified at least so (`label`).
    pub fn see(&mut self, classification: &Classification) {
        self.current_mut().work.see(classification);
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
    /// by (issue 137) - the tools of the sets it was offered
    /// (`Situation::tool_sets`), one offer for the whole turn, a resumed one
    /// included.
    pub(crate) fn tools(&self) -> impl Iterator<Item = &Tool> {
        self.situation().tool_sets().iter().flat_map(|set| &set.tools)
    }

    /// Gives the agent `message` to read in this turn (issue 22) - a skill
    /// loaded for it - as the knowledge `knowledge`, if it is knowledge
    /// (issue 389): into its transcript, as what it was given, not what it
    /// said, and the knowledge recorded (`AgentChannelState::given`); its
    /// channel is told after the step, for its history (issue 417). Not
    /// published. Written at once, as what a turn says is (issue 260), so a
    /// component gives only once nothing can fail any more.
    pub fn give(&mut self, message: Message, knowledge: Option<KnowledgeRef>) {
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
    ///
    /// # Errors
    /// `PromptError` if the plan cannot be worded to be named again;
    /// nothing is compacted then.
    pub fn compact(&mut self, replacement: Vec<(Message, Option<KnowledgeRef>)>, keep_from: Option<Digest>) -> Result<Option<(Digest, Digest)>, PromptError> {
        let replaced = replacement.len();
        let wording = self.plan_wording()?;
        let Some(heads) = self.agent_channel_state.compact(ActorId::Agent(self.agent_id), replacement, keep_from, self.runtime.prompts()) else { return Ok(None) };
        let replacement_end = replaced.checked_sub(1).and_then(|last| self.agent_channel_state.transcript.entries().nth(last)).map(|node| node.digest);
        self.name_plan_again(replacement_end, wording);
        Ok(Some(heads))
    }

    /// Records `message` as said by the turn: into the agent's transcript
    /// at once (`AgentChannelState::said`, issue 260). Returns it as it is
    /// sent: with what it was produced from (issue 417).
    pub(crate) fn push_message(&mut self, message: Message) -> Message {
        self.agent_channel_state.said(message)
    }

    /// Whether the turn may still call its model: not once it made
    /// `MAX_MODEL_CALLS` (issue 157) - since it began, or since the step of
    /// its plan under way began (issue 412). The count is the turn's and is
    /// stored with it, a resumed turn goes on from where it was.
    pub(crate) fn may_call_model(&self) -> bool {
        self.current().model_calls < MAX_MODEL_CALLS
    }

    /// Counts a call of the model, made once `may_call_model` allowed it. The
    /// activities may be tried again before the next one.
    pub(crate) fn count_model_call(&mut self) {
        let turn = self.current_mut();
        turn.model_calls += 1;
        turn.activities_tried.clear();
    }

    /// Counts the model calls anew, as a step of the plan or its final
    /// answer begins (issue 412): each may make as many as a turn without a
    /// plan. A model call came since any activity tried - the one that made
    /// or ended the plan's step -, so they may be tried again.
    pub(super) fn count_model_calls_anew(&mut self) {
        let turn = self.current_mut();
        turn.model_calls = 0;
        turn.activities_tried.clear();
    }

    /// Whether the turn tried the activity of the component `component`
    /// since its last model call (issue 464): it tries each once before a
    /// call.
    pub(crate) fn tried_activity(&self, component: &str) -> bool {
        self.current().activities_tried.iter().any(|tried| tried == component)
    }

    /// Records that the turn tries the activity of the component `component`
    /// before its next model call (`tried_activity`).
    pub(crate) fn try_activity(&mut self, component: &str) {
        self.current_mut().activities_tried.push(component.to_string());
    }

    /// The tool calls of the current step (issue 83): a view that keeps
    /// their bookkeeping off `Turn`. Short-lived on purpose - running a call
    /// needs the whole turn in between.
    pub(crate) fn tool_calls(&mut self) -> ToolCalls<'_> {
        debug_assert!(self.agent_channel_state.turn.is_some(), "a turn's working state changed while no turn runs");
        let texts = self.situation().texts();
        let turn = self.agent_channel_state.turn.get_or_insert_with(TurnState::default);
        ToolCalls::new(turn, &self.agent_name, texts)
    }

    /// Whether the turn was given or read private knowledge of the agent -
    /// its private notes (issue 51) or its memory (issue 62) - so far: what the model produces from
    /// then on may reveal it (issue 61).
    pub(crate) fn saw_private_knowledge(&self) -> bool {
        self.current().work.context.frame.knowledge.iter()
            .any(|k| matches!(k.source,
                KnowledgeSource::PrivateNotes { .. } | KnowledgeSource::Memory { .. }))
    }

    /// Records that the turn read `reference` (issue 17): it is recorded
    /// with every message the turn produces from now on.
    pub fn record_knowledge(&mut self, reference: KnowledgeRef) {
        self.current_mut().work.record_knowledge(reference);
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

    /// Puts `attachment` aside for the turn's answer (issue 282), which
    /// carries it (`say`).
    pub fn put_aside(&mut self, attachment: ContentRef) {
        self.current_mut().attachments.push(attachment);
    }

    /// Says `message`: keeps it in the turn and publishes it, the one way
    /// a turn's complete message reaches the channel (issue 258) - the
    /// model's, a step's tool message, the one answering the calls the turn
    /// abandoned. Kept before it is sent, so that a failed send loses
    /// nothing of the turn. The turn's answer - a message of the model
    /// calling no tools, not a step of a plan - carries what the turn put
    /// aside (`put_aside`), with words or without: what it attaches is
    /// heard all the same.
    pub(crate) async fn say(&mut self, mut message: Message, output: &dyn EventSender) -> Result<()> {
        if message.role == MessageRole::Assistant && !message.internal && message.tool_calls.is_empty() {
            message.attachments.append(&mut self.current_mut().attachments);
        }
        let said = self.push_message(message);
        self.send_event(event_for(said, false), output).await
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
    use crate::testing;
    use caelumora_model::actor::EventContext;
    use caelumora_model::confidentiality::Confidentiality;
    use caelumora_model::context::{ContextItem, ContextResolver, PromptPart};
    use caelumora_model::message::MessageId;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::message::MessageRole;
    use caelumora_runtime::transport::memory::InMemoryEventSender;
    use tokio::sync::mpsc;

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

    /// What its agent `agent` saw when it produced `said`, as it sent it -
    /// with what it was produced from (issue 417) -, resolved against
    /// `state`; `None` for a message it did not produce.
    pub(super) fn seen(state: &AgentChannelState, agent: AgentId, said: &Message) -> Option<Vec<ContextItem>> {
        let reference = state.context_of(said)?;
        Some(ContextResolver::of(state, &testing::catalogue()).resolve(&reference, said.turn_id?, ActorId::Agent(agent)).expect("worded")?.items)
    }

    #[test]
    fn a_resumed_turn_keeps_its_count_of_model_calls() {
        let mut state = AgentChannelState::default();
        let mut turn = Turn::begun(AgentId::default(), &mut state);
        for _ in 1..MAX_MODEL_CALLS {
            assert!(turn.may_call_model());
            turn.count_model_call();
        }

        // Stored and loaded again, as after a restart.
        let stored = serde_json::to_value(&state).unwrap();
        let mut state: AgentChannelState = serde_json::from_value(stored).unwrap();
        let mut turn = Turn::under_way(AgentId::default(), &mut state);
        assert!(turn.may_call_model(), "the last one it may make");
        turn.count_model_call();
        assert!(!turn.may_call_model(), "the limit holds across the restart");

        let mut stored = serde_json::to_value(&state).unwrap();
        stored["turn"].as_object_mut().unwrap().remove("model_calls");
        let state: AgentChannelState = serde_json::from_value(stored).unwrap();
        assert_eq!(state.turn.unwrap().model_calls, 0, "a turn stored before the count was kept starts from 0");
    }

    /// Issue 282: what a turn put aside goes with its next message of the model that others hear,
    /// not with a step of a plan, one without words or a tool message.
    #[tokio::test]
    async fn what_is_put_aside_goes_with_the_next_answer() {
        use caelumora_model::content::testing::make_content_ref;
        use caelumora_model::tool::{ToolCall, ToolVisibility};
        use caelumora_runtime::transport::Discard;
        let mut state = AgentChannelState::default();
        state.begin_turn(Work::new(Default::default(), None, Default::default(), Default::default(), Default::default()));
        let mut turn = Turn::under_way(AgentId::new(), &mut state);
        let aside = make_content_ref("a.png", b"a");
        turn.put_aside(aside.clone());
        let said = |role, internal| Message { role, internal, content: Some("said".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let call = ToolCall { id: Some("1".to_string()), function_name: "attach".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::default(), presentation: Default::default() };
        turn.say(said(MessageRole::Assistant, true), &Discard).await.unwrap();
        turn.say(Message { tool_calls: vec![call], ..said(MessageRole::Assistant, false) }, &Discard).await.unwrap();
        turn.say(said(MessageRole::Tool, false), &Discard).await.unwrap();
        turn.say(Message { content: None, ..said(MessageRole::Assistant, false) }, &Discard).await.unwrap();
        turn.say(said(MessageRole::Assistant, false), &Discard).await.unwrap();
        let carried: Vec<usize> = state.transcript.messages().iter().map(|message| message.attachments.len()).collect();
        assert_eq!(carried, [0, 0, 0, 1, 0], "the answer alone, once, with words or without - not a step of a plan, nor one that calls tools");
    }

    /// Issue 417: what a turn says leaves the agent as it is, what only its
    /// owner sees included - the channel records it so and gives everybody
    /// else what they may see of it -, and the agent keeps it in full.
    #[tokio::test]
    async fn events_leave_the_agent_as_they_are_and_its_context_keeps_the_full_call() {
        use caelumora_model::confidentiality::Visibility;
        use caelumora_model::tool::{ToolCall, ToolVisibility};
        let (tx, mut rx) = mpsc::channel(4);
        let output = InMemoryEventSender::new(tx);
        let mut state = AgentChannelState::default();
        state.begin_turn(Work::new(Default::default(), None, Default::default(), Default::default(), Default::default()));
        let mut turn = Turn::under_way(AgentId::new(), &mut state);

        let call = ToolCall { id: Some("1".to_string()), function_name: "write_private_note".to_string(), arguments: serde_json::json!({"text": "secret"}), visibility: ToolVisibility::HIDDEN, presentation: Default::default() };
        let thinking = Message { role: MessageRole::Assistant, reasoning: Some("weighing".to_string()), reasoning_visibility: Visibility::Owner, tool_calls: vec![call], ..Message::new(Classification::CHANNEL_DEFAULT) };
        turn.push_message(thinking.clone());
        turn.send_event(ActorEvent::CompleteMessageGenerated(thinking.clone()), &output).await.unwrap();
        turn.send_event(ActorEvent::TurnFinished { turn_id: TurnId::new(), messages: vec![thinking] }, &output).await.unwrap();

        match rx.recv().await.map(|e| e.event) {
            Some(ActorEvent::CompleteMessageGenerated(m)) => assert_eq!((m.tool_calls.len(), m.reasoning.as_deref()), (1, Some("weighing")), "the hidden call and the owner's reasoning go along"),
            other => panic!("expected the message, got {other:?}"),
        }
        match rx.recv().await.map(|e| e.event) {
            Some(ActorEvent::TurnFinished { messages, .. }) => assert_eq!(messages.len(), 1, "and the turn's end carries it"),
            other => panic!("expected the turn's end, got {other:?}"),
        }
        assert_eq!(turn.situation().agent_channel_state().transcript.messages().last().map(|m| m.tool_calls.len()), Some(1), "the agent keeps it");
    }

    #[test]
    fn a_turn_has_seen_its_transcript_its_prompt_parts_and_its_tool_results() {
        let internal = Classification::general(Confidentiality::Internal);
        let confidential = Classification::general(Confidentiality::Confidential);
        let said = |classification: &Classification, content: &str| Message { content: Some(content.to_string()), ..message_classified(classification) };
        let mut state = AgentChannelState::default();
        state.transcript.extend(vec![said(&Classification::PUBLIC, "Hello.")]);
        state.begin_turn(Work::new(EventContext::default(), None, SecurityContext::default(), confidential.clone(), Default::default()));
        let mut turn = Turn::under_way(AgentId::new(), &mut state);
        assert_eq!(turn.situation().label(), Classification::PUBLIC, "a public transcript");

        turn.set_context(vec![PromptPart { role: MessageRole::System, text: "Be brief.".to_string(), after: None, classification: Some(Classification::PUBLIC), source: None }], Vec::new());
        assert_eq!(turn.situation().label(), Classification::PUBLIC);
        state.perceive(vec![said(&internal, "Our numbers are good.")]);
        let mut turn = Turn::under_way(AgentId::new(), &mut state);
        assert_eq!(turn.situation().label(), internal, "what others said during the turn");
        turn.set_context(vec![PromptPart { role: MessageRole::System, text: "Be brief.".to_string(), after: None, classification: None, source: None }], Vec::new());
        assert_eq!(turn.situation().label(), confidential, "a part that does not say counts as the channel");
        assert_eq!(turn.situation().agent_channel_state().turn.as_ref().unwrap().work.context.frame.prompts[0].classification, None, "and is recorded so");
    }
}
