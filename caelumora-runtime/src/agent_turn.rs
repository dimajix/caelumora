//! The turn of an agent: what one cognitive cycle of an `AgentActor` works
//! with (see *Agent Cognitive Cycle* in `ARCHITECTURE.md`).
//!
//! A [`Turn`] is the handle an agent's actor and its components work through:
//! on the agent's state in its channel, and while a turn runs on that turn's
//! working state, `AgentChannelState::turn` (issue 85) - its messages, the
//! context and prompt parts the model is given and where they came from,
//! pending tool calls and the plan, the changes not yet taken over - and on
//! the agent's own state (issues 62, 65). What those states hold is the
//! business of wrappers such as `crate::agent_notes::AgentNotes` and
//! `crate::agent_memory::AgentMemory`. A turn is begun with `next_turn`, taken
//! over with `commit_turn` when it completes, and discarded with
//! `abandon_turn` when it fails; both end it and return it as an
//! [`EndedTurn`], which sends the turn's last event.
//!
//! Between turns there is no working state. What a `Turn` is asked about it
//! then reads as empty; nothing in the runtime asks - work between turns
//! (perceiving a message) touches only the transcript and the history.

use anyhow::{Context, Result};
use caelumora_model::agent::{AgentChannelChange, AgentChannelState, AgentId, AgentStateChange, TurnState};
use std::mem;
use std::sync::{Arc, LazyLock};
use tracing::warn;

use crate::transport::EventSender;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_model::approval::{ApprovalId, ApprovalStatus};
use caelumora_model::channel::ChannelId;
use caelumora_model::confidentiality::{ChannelClassification, Publication};
use crate::agent_state::AgentStateHandle;
use caelumora_model::agent::TurnKnowledge;
use caelumora_model::message::{ContextItem, ContextRef, KnowledgeRef, Message, PromptRef};
use caelumora_model::plan::{PlanProgress, PlanState};
use caelumora_model::security::SecurityContext;
use caelumora_model::session::SessionId;
use caelumora_model::tool::ToolCallState;

/// What a turn reads as when none runs.
static NO_TURN: LazyLock<TurnState> = LazyLock::new(TurnState::default);

/// What is left of a turn once it ended - taken over or discarded: what its
/// last event is sent with, the contexts and the publication the turn had
/// (issue 85). Nothing else; the rest of the turn was used up in ending it.
#[derive(Debug)]
pub struct EndedTurn {
    agent_id: AgentId,
    event_context: EventContext,
    security_context: SecurityContext,
    publication: Publication,
}

impl EndedTurn {
    fn of(agent_id: AgentId, turn: &TurnState) -> Self {
        Self {
            agent_id,
            event_context: turn.event_context.clone(),
            security_context: turn.security_context.clone(),
            publication: turn.publication,
        }
    }

    /// Sends `event` as the ended turn would have: see [`Turn::send_event`].
    pub async fn send_event(&self, event: ActorEvent, output: &dyn EventSender) -> Result<()> {
        send(&self.event_context, &self.security_context, self.publication, self.agent_id, event, output).await
    }
}

/// What is still needed of a turn once it is taken over (`commit_turn`).
#[derive(Debug)]
pub struct CommittedTurn {
    /// The messages that are not internal: the turn's answer, for
    /// `FullResponseGenerated`.
    pub final_messages: Vec<Message>,
    /// Whether the agent's state in the channel changed, to be announced
    /// once stored.
    pub channel_state_changed: bool,
    /// The turn itself, to send its answer with.
    pub ended: EndedTurn,
}

#[derive(Debug)]
pub struct Turn<'a> {
    pub agent_id: AgentId,
    pub agent_name: String,
    agent_channel_state: &'a mut AgentChannelState,
    /// The agent's own state, shared by all its actors (issue 62). The turn
    /// reads it and stages changes, never holds it.
    agent_state: Arc<AgentStateHandle>,
}

impl<'a> Turn<'a> {
    pub fn new(agent_id: AgentId, agent_name:String, agent_channel_state: &'a mut AgentChannelState, agent_state: Arc<AgentStateHandle>) -> Self {
        Self {
            agent_id,
            agent_name,
            agent_channel_state,
            agent_state,
        }
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

    /// The agent's channel as the confidentiality rules see it, as it was
    /// when the turn began (`next_turn`).
    pub fn classification(&self) -> ChannelClassification {
        self.current().classification
    }

    pub fn channel_id(&self) -> ChannelId {
        self.current().classification.channel_id
    }

    pub fn session_id(&self) -> SessionId {
        self.current().classification.session_id
    }

    /// Stages `change` to the agent's own state, applied when the turn
    /// completes (`commit_turn`).
    pub fn stage_agent_change(&mut self, change: AgentStateChange) {
        self.current_mut().agent_changes.push(change);
    }

    /// The changes to the agent's own state staged in this turn so far.
    pub fn staged_agent_changes(&self) -> &[AgentStateChange] {
        &self.current().agent_changes
    }

    /// Takes the completed turn over and ends it (issues 06, 17, 47, 62, 65):
    ///
    /// - appends the messages the agent produced (`push_message`) to its
    ///   transcript, and those that are not internal to its history, each
    ///   with what it was produced from - the transcript before it, the
    ///   turn's prompt parts and what it had read;
    /// - applies the changes staged to its state in this channel, anchored at
    ///   the turn's last message in the history;
    /// - applies the changes staged to its own state to that state as it is
    ///   now. The answer is already out, so a state that cannot be stored
    ///   costs the turn's memory, not the turn: it is logged.
    ///
    /// The turn's working state is gone afterwards, so none of it is stored
    /// with the channel state. What is still needed is returned.
    pub async fn commit_turn(&mut self) -> CommittedTurn {
        let turn = self.agent_channel_state.turn.take().unwrap_or_default();
        let ended = EndedTurn::of(self.agent_id, &turn);
        // Every part is used up exactly once.
        let TurnState { context, messages, knowledge, agent_changes, channel_changes, .. } = turn;
        let final_messages = messages.iter().filter(|m| !m.internal).cloned().collect();
        let channel_state_changed = self.record_turn(&context, messages, knowledge, channel_changes);
        if let Err(e) = self.agent_state.apply(agent_changes, self.agent_channel_state.history.last_message_id()).await {
            warn!("Agent {}: the changes of its turn to its own state are lost: {:#}", self.agent_id, e);
        }
        CommittedTurn { final_messages, channel_state_changed, ended }
    }

    /// Discards the turn after it failed or was stopped: its working state,
    /// the changes it staged and what it read - none of it is taken over.
    pub fn abandon_turn(&mut self) -> EndedTurn {
        let turn = self.agent_channel_state.turn.take().unwrap_or_default();
        EndedTurn::of(self.agent_id, &turn)
    }

    /// Begins a turn: fixes what it is - the envelope it answers, whose
    /// authority it acts with, the channel as the confidentiality rules see
    /// it now, and what the `SecurityManager` lets it publish there (issue
    /// 61). The last two hold for the whole turn, so that what it reads,
    /// publishes and remembers is judged alike. Whatever a previous turn
    /// left behind is gone.
    pub fn next_turn(
        &mut self,
        event_context: EventContext,
        security_context: SecurityContext,
        classification: ChannelClassification,
        publication: Publication,
    ) {
        self.agent_channel_state.turn = Some(TurnState {
            event_context,
            security_context,
            classification,
            publication,
            ..TurnState::default()
        });
    }

    /// Begins a turn with default contexts - what a test needs to work in
    /// one.
    #[cfg(test)]
    pub(crate) fn begin(&mut self) {
        self.next_turn(EventContext::default(), SecurityContext::default(), ChannelClassification::default(), Publication::default());
    }

    /// The event context of the current turn: carries the ids (`span_id`,
    /// `correlation_id`) that tie the turn to the envelope that started it.
    pub fn event_context(&self) -> &EventContext {
        &self.current().event_context
    }

    /// The context of the turn as the model is given it: the conversation,
    /// with every prompt part as a message of the agent in its role.
    pub fn context(&self) -> Vec<Message> {
        self.current().context.iter()
            .map(|item| match item {
                ContextItem::Conversation { message, .. } => message.clone(),
                ContextItem::Prompt { role, text } => Message {
                    role: *role,
                    content: Some(text.clone()),
                    sender_id: ActorId::Agent(self.agent_id),
                    sender_name: self.agent_name.clone(),
                    ..Default::default()
                },
            })
            .collect()
    }

    pub fn full_context(&self) -> Vec<Message> {
        let mut context = self.context();
        context.extend_from_slice(self.current().messages.as_slice());
        context
    }

    /// Sets the context of the turn: the conversation with the prompt parts
    /// where the caller placed them. Every prompt part's text is kept once by
    /// digest, so that what the agent saw can be told later (issues 06, 47).
    pub fn set_context(&mut self, items: Vec<ContextItem>) {
        for item in &items {
            if let ContextItem::Prompt { text, .. } = item {
                let digest = caelumora_model::digest::prompt(text);
                self.agent_channel_state.prompts.entry(digest).or_insert_with(|| text.clone());
            }
        }
        self.current_mut().context = items;
    }

    pub fn messages(&self) -> &Vec<Message> {
        &self.current().messages
    }
    pub fn last_message(&self) -> Option<&Message> {
        self.current().messages.last()
    }
    pub fn push_message(&mut self, message: Message) {
        self.current_mut().messages.push(message);
    }

    pub fn set_tool_calls(&mut self, tool_calls: Vec<ToolCallState>) {
        self.current_mut().tool_calls = tool_calls;
    }
    pub fn set_tool_approval_status(&mut self, id:ApprovalId, status: ApprovalStatus) -> bool {
        if let Some(tool_call) = self.current_mut().tool_calls.iter_mut().find(|tc| tc.id == id) {
            tool_call.status = status;
            true
        }
        else {
            false
        }
    }
    pub fn tool_calls(&self) -> &Vec<ToolCallState> {
        &self.current().tool_calls
    }
    pub fn take_tool_calls(&mut self) -> Vec<ToolCallState> {
        mem::take(&mut self.current_mut().tool_calls)
    }
    pub fn has_tool_calls(&self) -> bool {
        !self.current().tool_calls.is_empty()
    }

    pub fn set_plan(&mut self, plan: PlanState) {
        self.current_mut().plan = Some(plan);
    }
    pub fn plan(&self) -> Option<&PlanState> {
        self.current().plan.as_ref()
    }
    pub fn take_plan(&mut self) -> Option<PlanState> {
        self.current_mut().plan.take()
    }
    /// Whether a plan is currently in play. A `PlanState` is only ever
    /// constructed after its `create_plan` tool call has already been
    /// through the generic tool-call approval gate, so its mere presence
    /// implies "approved and executing".
    pub fn has_plan(&self) -> bool {
        self.current().plan.is_some()
    }
    pub fn set_plan_progress(&mut self, progress: PlanProgress) {
        if let Some(plan) = self.current_mut().plan.as_mut() {
            plan.progress = progress;
        }
    }

    pub fn security_context(&self) -> &SecurityContext {
        &self.current().security_context
    }

    pub fn transcript(&self) -> Vec<Message> {
        self.agent_channel_state.transcript.messages()
    }
    pub fn extend_transcript(&mut self, messages: Vec<Message>) {
        self.agent_channel_state.transcript.extend(messages);
    }
    pub fn extend_history(&mut self, messages: Vec<Message>) {
        self.agent_channel_state.history.extend(messages);
    }

    /// Whether the turn was given or read private knowledge of the agent -
    /// its private notes (issue 51) or its memory (issue 62) - so far: what the model produces from
    /// then on may reveal it (issue 61).
    pub fn saw_private_knowledge(&self) -> bool {
        self.current().knowledge.iter()
            .any(|k| matches!(k.reference.source,
                caelumora_model::message::KnowledgeSource::PrivateNotes { .. } | caelumora_model::message::KnowledgeSource::Memory { .. }))
    }

    /// Records that the turn read `reference` (issue 17): it is recorded
    /// with every message the turn produces from now on.
    pub fn record_knowledge(&mut self, reference: KnowledgeRef) {
        let turn = self.current_mut();
        let before = turn.messages.len();
        if !turn.knowledge.iter().any(|k| k.reference == reference) {
            turn.knowledge.push(TurnKnowledge { reference, before });
        }
    }

    /// The agent's state in this channel as it stands: what components read
    /// their state from (issue 65). Changes the turn made count only once it
    /// completes.
    pub fn agent_channel_state(&self) -> &AgentChannelState {
        self.agent_channel_state
    }

    /// Writes where the actor stands into the state, before it is stored.
    pub(crate) fn set_processing_state(&mut self, state: ActorState) {
        self.agent_channel_state.processing_state = state;
    }

    /// Stages `change` to the agent's state in this channel, applied when the
    /// turn completes (`commit_turn`).
    pub fn stage_channel_change(&mut self, change: AgentChannelChange) {
        self.current_mut().channel_changes.push(change);
    }

    /// The changes to the agent's state in this channel staged in this turn
    /// so far - to change as well: a change taken back within the turn is
    /// simply dropped.
    pub fn staged_channel_changes(&mut self) -> &mut Vec<AgentChannelChange> {
        &mut self.current_mut().channel_changes
    }

    /// The part of `commit_turn` that concerns the agent's state in this
    /// channel: the turn's `messages`, produced in `context` after reading
    /// `knowledge`, and its `changes` to this state. Returns whether there
    /// were changes.
    fn record_turn(&mut self, context: &[ContextItem], messages: Vec<Message>, knowledge: Vec<TurnKnowledge>, changes: Vec<AgentChannelChange>) -> bool {
        let before = self.agent_channel_state.transcript.extend(messages.clone());
        let prompts = prompt_refs(context);
        for (index, (message, transcript)) in messages.into_iter().zip(before).enumerate() {
            if !message.internal {
                // What was read before the message was produced.
                let knowledge = knowledge.iter().filter(|k| k.before <= index).map(|k| k.reference).collect();
                self.agent_channel_state.history.push(message, Some(ContextRef { transcript, prompts: prompts.clone(), knowledge }));
            }
        }

        // A message of the history, not an internal step of the transcript:
        // what branching (issue 08) will find the changes by.
        let after = self.agent_channel_state.history.last_message_id();
        let transcript = self.agent_channel_state.transcript.digest();
        let changed = !changes.is_empty();
        for change in changes {
            self.agent_channel_state.apply(change, after, transcript);
        }
        changed
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

    /// Which labels the agent may publish to its channel in this turn, as
    /// the `SecurityManager` decided when it began (`next_turn`).
    pub fn publication(&self) -> Publication {
        self.current().publication
    }

    /// Sends `av` to the channel as the agent publishes it: messages redacted
    /// by the labels of their parts against what the turn may publish
    /// (issues 54, 61), in the turn's contexts. The agent keeps the full
    /// messages in its context; an event with nothing left to show is not
    /// sent.
    pub async fn send_event(&self, av: ActorEvent, output: &dyn EventSender) -> Result<()> {
        let turn = self.current();
        send(&turn.event_context, &turn.security_context, turn.publication, self.agent_id, av, output).await
    }
}

/// Where the prompt parts of `context` were placed: each by the digest of
/// its text, with the transcript entry it followed (issue 47).
fn prompt_refs(context: &[ContextItem]) -> Vec<PromptRef> {
    let mut after = None;
    let mut prompts = Vec::new();
    for item in context {
        match item {
            ContextItem::Conversation { digest, .. } => after = Some(*digest),
            ContextItem::Prompt { role, text } => {
                prompts.push(PromptRef { text: caelumora_model::digest::prompt(text), role: *role, after });
            }
        }
    }
    prompts
}

/// Sends `av` as a turn of `agent_id` publishes it: redacted by what
/// `publication` allows, in the turn's contexts.
async fn send(
    event_context: &EventContext,
    security_context: &SecurityContext,
    publication: Publication,
    agent_id: AgentId,
    av: ActorEvent,
    output: &dyn EventSender,
) -> Result<()> {
    let Some(av) = av.published(&|label| publication.allows(label)) else { return Ok(()) };
    let ev = Envelope::new(security_context.clone(), event_context.clone(), av);
    output
        .send(ev)
        .await
        .context(format!("Actor {} failed to send event", agent_id))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::message::MessageRole;
    use caelumora_model::plan::{Plan, PlanId, PlanStep};

    fn make_plan_state() -> PlanState {
        PlanState {
            progress: PlanProgress::Step(0),
            plan: Plan {
                id: PlanId::new(),
                summary: "do the thing".to_string(),
                steps: vec![PlanStep { description: "step 1".to_string() }],
            },
        }
    }

    #[test]
    fn plan_round_trips_through_set_and_take() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut ctx = Turn::new(AgentId::default(), String::new(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached());
        ctx.begin();

        assert!(ctx.plan().is_none());
        assert!(!ctx.has_plan());

        ctx.set_plan(make_plan_state());
        assert!(ctx.plan().is_some());
        assert!(ctx.has_plan());

        let taken = ctx.take_plan();
        assert!(taken.is_some());
        assert!(ctx.plan().is_none());
        assert!(!ctx.has_plan());
    }

    #[test]
    fn next_turn_clears_a_stale_plan() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut ctx = Turn::new(AgentId::default(), String::new(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached());
        ctx.begin();

        ctx.set_plan(make_plan_state());
        assert!(ctx.plan().is_some());

        ctx.next_turn(EventContext::default(), SecurityContext::default(), Default::default(), Default::default());
        assert!(ctx.plan().is_none());
    }

    #[tokio::test]
    async fn what_the_agent_saw_can_be_told_for_each_message_it_produced() {
        use caelumora_model::agent::{AgentChannelState, AgentId, SeenItem};
        use caelumora_model::message::{MessageId, MessageRole};

        let text = |role: MessageRole, content: &str| Message { id: MessageId::new(), role, content: Some(content.to_string()), ..Message::default() };
        let mut state = AgentChannelState::default();
        let question = text(MessageRole::User, "What is 2+2?");
        state.transcript.push(question.clone());

        let (first, second) = (text(MessageRole::Assistant, "Let me think."), text(MessageRole::Assistant, "4"));
        let system = || ContextItem::Prompt { role: MessageRole::System, text: "You are good at sums.".to_string() };
        {
            let mut turn = Turn::new(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
            turn.begin();
            let mut items = vec![system()];
            items.extend(turn.agent_channel_state().transcript.entries().iter()
                .map(|e| ContextItem::Conversation { message: e.message.clone(), digest: e.digest }));
            turn.set_context(items);
            assert_eq!(turn.context().len(), 2, "the prompt is given to the model as a message");
            turn.push_message(first.clone());
            turn.push_message(second.clone());
            turn.commit_turn().await;
        }

        let seen = state.seen_context_of(second.id).expect("the agent produced it");
        let order: Vec<String> = seen.items.iter().map(|item| match item {
            SeenItem::Prompt { text, role, .. } => format!("{role:?}:{text}"),
            SeenItem::Message(m) => m.content.clone().unwrap_or_default(),
        }).collect();
        assert_eq!(order, ["System:You are good at sums.", "What is 2+2?", "Let me think."], "everything before it, its own earlier message included");
        assert!(state.seen_context_of(question.id).is_none(), "not produced by the agent");

        // Knowledge read in a turn counts for the messages after it.
        let (before, after) = (text(MessageRole::Assistant, "Let me look."), text(MessageRole::Assistant, "Found it."));
        let read = caelumora_model::message::KnowledgeRef {
            channel: Default::default(),
            source: caelumora_model::message::KnowledgeSource::History { digest: None, start: None },
        };
        {
            let mut turn = Turn::new(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
            turn.begin();
            turn.push_message(before.clone());
            turn.record_knowledge(read);
            turn.record_knowledge(read);
            turn.push_message(after.clone());
            turn.commit_turn().await;
        }
        let knowledge_of = |id| state.recorded_context_of(id).unwrap().knowledge;
        assert!(knowledge_of(before.id).is_empty(), "produced before the read");
        assert_eq!(knowledge_of(after.id), vec![read], "produced after it, recorded once");

        // The same prompt in another turn is kept once.
        {
            let mut turn = Turn::new(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
            turn.begin();
            turn.set_context(vec![system()]);
        }
        assert_eq!(state.prompts.len(), 1);
    }

    #[tokio::test]
    async fn events_leave_the_agent_redacted_while_its_context_keeps_the_full_call() {
        use caelumora_model::confidentiality::Confidentiality;
        use caelumora_model::tool::{ToolCall, ToolConfidentiality};
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let output = crate::transport_impl::memory::InMemoryEventSender::new(tx);
        let mut state = AgentChannelState::default();
        let mut turn = Turn::new(AgentId::new(), "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached());
        turn.next_turn(Default::default(), Default::default(), Default::default(), Publication::new(true, true));

        let call = ToolCall { id: Some("1".to_string()), function_name: "note_private".to_string(), arguments: serde_json::json!({"text": "secret"}), confidentiality: ToolConfidentiality::HIDDEN };
        let message = Message { role: MessageRole::Assistant, tool_calls: vec![call], ..Message::default() };
        turn.push_message(message.clone());
        turn.send_event(ActorEvent::CompleteMessageGenerated(message.clone()), &output).await.unwrap();
        turn.send_event(ActorEvent::FullResponseGenerated(vec![message]), &output).await.unwrap();

        match rx.recv().await.map(|e| e.event) {
            Some(ActorEvent::FullResponseGenerated(messages)) => assert!(messages.is_empty(), "the hidden call is not published"),
            other => panic!("the complete message is not sent at all, got {other:?}"),
        }
        assert_eq!(turn.messages()[0].tool_calls.len(), 1, "the agent keeps it");

        // Reasoning reaches the channel only as far as the turn may publish.
        let thinking = Message {
            role: MessageRole::Assistant,
            content: Some("done".to_string()),
            reasoning: Some("weighing".to_string()),
            reasoning_confidentiality: Confidentiality::Channel,
            ..Message::default()
        };
        turn.send_event(ActorEvent::CompleteMessageGenerated(thinking.clone()), &output).await.unwrap();
        turn.next_turn(Default::default(), Default::default(), Default::default(), Publication::new(true, false));
        turn.send_event(ActorEvent::CompleteMessageGenerated(thinking), &output).await.unwrap();
        let reasoning = |event: Option<Envelope>| match event.map(|e| e.event) {
            Some(ActorEvent::CompleteMessageGenerated(m)) => m.reasoning,
            other => panic!("expected a message, got {other:?}"),
        };
        assert_eq!(reasoning(rx.recv().await).as_deref(), Some("weighing"), "a channel-level channel admits it");
        assert!(reasoning(rx.recv().await).is_none(), "a session-level one does not");
    }
}
