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
//! `crate::agent_memory::AgentMemory`. A turn is begun with `next_turn`, taken
//! over with `commit_turn` when it completes, and given up with
//! `give_up_turn` when it fails or is stopped, which keeps its messages as a
//! branch of the transcript that is no longer active (issue 145); both end
//! it and return it as an [`EndedTurn`], which sends the turn's last event.
//!
//! The logic of a turn's steps lives here too - plan progress here, the
//! bookkeeping of a step's tool calls in `crate::tool_calls` (issue 83) - while the
//! `AgentActor` keeps what is input and output: the model call, the tool
//! executor, sending. What only the actor calls is `pub(crate)`; the rest is
//! the API components work with.
//!
//! Between turns there is no working state. What a `Turn` is asked about it
//! then reads as empty; nothing in the runtime asks - work between turns
//! (perceiving a message) touches only the transcript and the history.

use anyhow::{Context, Result};
use caelumora_model::agent::{AgentChannelChange, AgentChannelState, AgentId, AgentStateChange, TurnState};
use std::sync::{Arc, LazyLock};
use tracing::warn;

use crate::transport::EventSender;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_model::confidentiality::{ChannelRef, Classification};
use crate::agent_state::AgentStateHandle;
use caelumora_model::agent::TurnEntry;
use caelumora_model::context::{ContextItem, ContextRef, ContextResolver, KnowledgeRef, ModelProfile, PromptPart, PromptRef};
use caelumora_model::message::{executed_calls, Message, MessageRole};
use caelumora_model::transcript::{GivenUp, GivenUpReason};
use caelumora_model::plan::{Plan, PlanProgress, PlanState, PlanStep};
use caelumora_model::security::SecurityContext;
use caelumora_model::tool::{Tool, ToolCall, ToolCallResult, ToolDefinition};
use crate::tool_calls::ToolCalls;

/// What a turn reads as when none runs.
static NO_TURN: LazyLock<TurnState> = LazyLock::new(TurnState::default);

/// How many times a turn may ask its model; then it ends with what it has.
pub(crate) const MAX_MODEL_CALLS: u32 = 25;

/// What is left of a turn once it ended - taken over or discarded: what its
/// last event is sent with, the contexts the turn had (issue 85). Nothing else; the rest of the turn was used up in ending it.
#[derive(Debug)]
pub struct EndedTurn {
    agent_id: AgentId,
    event_context: EventContext,
    security_context: SecurityContext,
}

impl EndedTurn {
    fn of(agent_id: AgentId, turn: &TurnState) -> Self {
        Self {
            agent_id,
            event_context: turn.event_context.clone(),
            security_context: turn.security_context.clone(),
        }
    }

    /// Sends `event` as the ended turn would have: see [`Turn::send_event`].
    pub(crate) async fn send_event(&self, event: ActorEvent, output: &dyn EventSender) -> Result<()> {
        send(&self.event_context, &self.security_context, self.agent_id, event, output).await
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

/// What the agent is told of the calls that ran in a turn given up
/// (`executed`), and why it was given up: with arguments and results
/// `in_full`, else only which tools ran - the turn does no longer flow into
/// the channel, whose classification was lowered meanwhile (issue 141), so
/// what it saw may not either.
fn told_of_executed(reason: &GivenUpReason, executed: &[(Option<&ToolCall>, &ToolCallResult)], in_full: bool) -> String {
    let because = match reason {
        GivenUpReason::Failed { .. } => "it failed",
        GivenUpReason::Lowered { .. } => "the channel's classification was lowered",
        GivenUpReason::Stopped => "you were stopped",
    };
    let calls: Vec<String> = executed.iter().map(|(call, result)| match in_full {
        true => {
            let arguments = call.map(|c| c.arguments.to_string()).unwrap_or_default();
            let status = serde_json::to_string(&result.status).unwrap_or_default();
            format!("- {}({}) -> {}: {}", result.tool_name, arguments, status.trim_matches('"'), result.content)
        }
        false => format!("- {}", result.tool_name),
    }).collect();
    let (what, hidden) = match in_full {
        true => ("these tool calls", ""),
        false => ("calls of these tools", "Their arguments and results cannot be shown here: the channel's classification was lowered since. "),
    };
    format!(
        "Your previous turn was given up before it was complete, because {because}. Its messages are gone, but {what} had already been executed, and their effects remain:\n{}\n{hidden}Do not repeat them unless that is what is wanted.",
        calls.join("\n"))
}

/// What the model is given for a step of the turn (`Turn::full_context`):
/// the messages of its context and the tools it is offered.
#[derive(Debug)]
pub(crate) struct FullContext {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
}

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
        }
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
    /// began (`next_turn`).
    pub fn classification(&self) -> Classification {
        self.current().classification.clone()
    }

    /// The agent's channel: where the envelope the turn answers was routed
    /// - the `ChannelLoop` stamps it on every envelope it routes.
    pub fn channel(&self) -> ChannelRef {
        let event = &self.current().event_context;
        ChannelRef { session_id: event.session_id, channel_id: event.channel_id }
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
    ///   with what it was produced from (issue 136) - the transcript before
    ///   it, the prompt parts placed before it and what it had read;
    /// - applies the changes staged to its state in this channel, anchored at
    ///   the turn's last message in the history;
    /// - applies the changes staged to its own state to that state as it is
    ///   now. The answer is already out, so a state that cannot be stored
    ///   costs the turn's memory, not the turn: it is logged.
    ///
    /// The turn's working state is gone afterwards, so none of it is stored
    /// with the channel state. What is still needed is returned.
    pub(crate) async fn commit_turn(&mut self) -> CommittedTurn {
        let turn = self.agent_channel_state.turn.take().unwrap_or_default();
        let ended = EndedTurn::of(self.agent_id, &turn);
        // Every part is used up exactly once.
        let final_messages = turn.messages().filter(|m| !m.internal).cloned().collect();
        let TurnState { entries, agent_changes, channel_changes, .. } = turn;
        let channel_state_changed = self.record_turn(entries, channel_changes);
        if let Err(e) = self.agent_state.apply(agent_changes, self.agent_channel_state.history.last_message_id()).await {
            warn!("Agent {}: the changes of its turn to its own state are lost: {:#}", self.agent_id, e);
        }
        CommittedTurn { final_messages, channel_state_changed, ended }
    }

    /// Gives the turn up after it failed or was stopped, for `reason`
    /// (issue 145): its messages go into the transcript and the history as
    /// they would when taken over, each with what it was produced from, but
    /// as a branch that is no longer active - both heads go back to where
    /// the turn began, and the transcript's last message of the turn is
    /// marked as given up. Nothing it staged is applied.
    ///
    /// What its tools did stays done. If any of its calls ran, the agent is
    /// told on the active branch, in a message of the runtime in its
    /// transcript: in full, classified as the turn - or, with `names_only`,
    /// only which tools ran, classified so.
    pub(crate) fn give_up_turn(&mut self, reason: GivenUpReason, names_only: Option<Classification>) -> EndedTurn {
        let turn = self.agent_channel_state.turn.take().unwrap_or_default();
        let ended = EndedTurn::of(self.agent_id, &turn);
        let TurnState { entries, classification, .. } = turn;
        let Some(count) = std::num::NonZeroUsize::new(entries.len()) else { return ended };
        let messages: Vec<&Message> = entries.iter().map(|e| &e.entry.message).collect();
        let executed = executed_calls(&messages);
        let told = (!executed.is_empty()).then(|| Message {
            id: caelumora_model::message::MessageId::new(),
            content: Some(told_of_executed(&reason, &executed, names_only.is_none())),
            internal: true,
            ..Message::new(names_only.unwrap_or(classification))
        });

        let transcript = self.agent_channel_state.transcript.digest();
        let history = self.agent_channel_state.history.digest();
        self.record_turn(entries, Vec::new());
        if let Some(last) = self.agent_channel_state.transcript.digest() {
            let given_up = GivenUp { reason: reason.clone(), at: chrono::Utc::now(), turn_entries: count.get() };
            self.agent_channel_state.transcript.mark_given_up(&last, given_up);
        }
        self.agent_channel_state.transcript.move_head(transcript.as_ref());
        self.agent_channel_state.history.move_head(history.as_ref());

        if let Some(message) = told {
            self.agent_channel_state.transcript.push(message);
        }
        ended
    }

    /// Begins a turn: fixes what it is - the envelope it answers, whose
    /// authority it acts with, the channel and its classification now,
    /// how the agent's model is given its context (the profile, issues 49,
    /// 63), and the agent's `tools` (issue 137). All but the first two hold
    /// for the whole turn - also one resumed after the model's configuration
    /// or the agent's tools changed - so that what it reads, publishes and
    /// remembers is judged alike, and what its model is given and offered
    /// too. Whatever a previous turn left behind is gone.
    pub(crate) fn next_turn(
        &mut self,
        event_context: EventContext,
        security_context: SecurityContext,
        classification: Classification,
        tools: Vec<Tool>,
    ) {
        let offered = self.keep_tools(&tools);
        self.agent_channel_state.turn = Some(TurnState {
            event_context,
            security_context,
            classification,
            tools,
            context: ContextRef {
                transcript: self.agent_channel_state.transcript.digest(),
                tools: offered,
                profile: self.profile,
                ..ContextRef::default()
            },
            ..TurnState::default()
        });
    }

    /// Keeps the set of `tools` as the model is offered them once by digest
    /// (issue 137), and returns the digest - `None` for no tools.
    fn keep_tools(&mut self, tools: &[Tool]) -> Option<caelumora_model::digest::Digest> {
        if tools.is_empty() {
            return None;
        }
        let definitions: Vec<ToolDefinition> = tools.iter().map(Tool::definition).collect();
        let digest = caelumora_model::digest::tools(&definitions);
        self.agent_channel_state.tool_sets.entry(digest).or_insert(definitions);
        Some(digest)
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

    /// What the model is given for the next step: the transcript, continued
    /// by the messages the turn produced so far, with the prompt parts where
    /// they were placed - each as the model's profile gives it (issue 63),
    /// every prompt part as a message of the agent in its role - and the
    /// tools the turn offers (issue 137). Resolved from the turn's context the
    /// way a recorded message's context is (`ContextResolver`, issue 136),
    /// so the two cannot diverge.
    pub(crate) fn full_context(&self) -> Result<FullContext> {
        let resolved = ContextResolver::of(self.agent_channel_state)
            .resolve(&self.current().context, ActorId::Agent(self.agent_id))
            .context("the context of the turn names what the agent's state does not hold")?;
        let messages = resolved.items.into_iter()
            .map(|item| match item {
                ContextItem::Conversation { message } => message,
                ContextItem::Prompt { role, text, .. } => Message {
                    role,
                    content: Some(text),
                    sender_id: ActorId::Agent(self.agent_id),
                    sender_name: self.agent_name.clone(),
                    ..Message::new(self.current().classification.clone())
                },
            })
            .collect();
        Ok(FullContext { messages, tools: resolved.tools })
    }

    /// Sets the prompt parts the model of the turn is given besides the
    /// transcript, where the `ContextProvider` placed them - once, at the
    /// start of the turn; what was placed before is replaced. Every part's
    /// text is kept once by digest, so that what the agent saw can be told
    /// later (issues 06, 47).
    pub fn set_context(&mut self, parts: Vec<PromptPart>) {
        let prompts = parts.into_iter().map(|PromptPart { role, text, after }| self.keep_prompt(role, text, after)).collect();
        self.current_mut().context.prompts = prompts;
    }

    /// Keeps the text of a prompt part once by digest, and returns the
    /// reference to it, given in `role` after the entry `after`.
    fn keep_prompt(&mut self, role: MessageRole, text: String, after: Option<caelumora_model::digest::Digest>) -> PromptRef {
        let digest = caelumora_model::digest::prompt(&text);
        self.agent_channel_state.prompts.entry(digest).or_insert(text);
        PromptRef { text: digest, role, after }
    }

    /// Places `text`, given in `role`, after the messages the turn has
    /// produced so far (issue 49): an instruction of the runtime to the
    /// model, such as the next step of a plan. A prompt part, not a message:
    /// it is kept once by digest like the others, never goes into the
    /// transcript, and is given as the model's profile says a message in
    /// `role` there is to be given. It counts for the messages produced
    /// from now on.
    pub(crate) fn place_turn_prompt(&mut self, role: MessageRole, text: String) {
        let after = self.current().context.transcript;
        let prompt = self.keep_prompt(role, text, after);
        self.current_mut().context.prompts.push(prompt);
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
        ToolCalls::new(turn, self.agent_id, &self.agent_name)
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
    /// Where the plan in play stands, `None` without one. A `PlanState` is
    /// only ever constructed after its `create_plan` tool call has already
    /// been through the generic tool-call approval gate, so its mere
    /// presence implies "approved and executing".
    pub(crate) fn plan_progress(&self) -> Option<PlanProgress> {
        self.current().plan.as_ref().map(|p| p.progress)
    }

    /// Whether what the model produces now is part of executing a plan -
    /// a step under way, not the final answer after the last one - and so
    /// internal: kept in the transcript, not in the history, and published
    /// as intermediate.
    pub(crate) fn in_plan_step(&self) -> bool {
        matches!(self.plan_progress(), Some(PlanProgress::Step(_)))
    }
    fn set_plan_progress(&mut self, progress: PlanProgress) {
        if let Some(plan) = self.current_mut().plan.as_mut() {
            plan.progress = progress;
        }
    }

    /// Moves the plan on to step `next`: gives the model the instruction for
    /// it and returns the `PlanStepStarted` to announce - or, with no such
    /// step, gives it the instruction to write the final answer, which is no
    /// step and announces nothing (an empty plan finalizes at once). Called
    /// when a plan is accepted or extended, and each time a step converges.
    /// The instructions are system text placed after the turn's messages so
    /// far (`place_turn_prompt`, issue 49): where the model keeps system
    /// messages only at the start, they are given as user messages.
    pub(crate) fn advance_plan(&mut self, next: usize) -> Option<ActorEvent> {
        let plan = self.plan()?.plan.clone();
        match plan.steps.get(next) {
            Some(step) => {
                self.place_turn_prompt(MessageRole::System, plan_step_instruction(next + 1, plan.steps.len(), step));
                self.set_plan_progress(PlanProgress::Step(next));
                Some(ActorEvent::PlanStepStarted {
                    step_index: next,
                    total_steps: plan.steps.len(),
                    description: step.description.clone(),
                })
            }
            None => {
                self.place_turn_prompt(MessageRole::System, plan_finalize_instruction(&plan));
                self.set_plan_progress(PlanProgress::Finalizing);
                None
            }
        }
    }

    /// Moves the plan on after the model answered without calling a tool:
    /// the step under way is finished - announced as `PlanStepFinished` -
    /// and the next one begins, or the final answer is asked for
    /// (`advance_plan`); after the final answer the plan is over. Returns
    /// the events to announce, in order; none without a plan.
    pub(crate) fn plan_converged(&mut self) -> Vec<ActorEvent> {
        let Some(plan) = self.plan() else { return Vec::new() };
        match plan.progress {
            PlanProgress::Step(step_index) => {
                let total_steps = plan.plan.steps.len();
                let description = plan.plan.steps.get(step_index).map(|s| s.description.clone()).unwrap_or_default();
                let mut events = vec![ActorEvent::PlanStepFinished { step_index, total_steps, description }];
                events.extend(self.advance_plan(step_index + 1));
                events
            }
            PlanProgress::Finalizing => {
                self.take_plan();
                Vec::new()
            }
        }
    }

    pub fn security_context(&self) -> &SecurityContext {
        &self.current().security_context
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
    /// channel: the turn's `entries` - its messages, each with what it was
    /// produced from - and its `changes` to this state. Returns whether
    /// there were changes.
    fn record_turn(&mut self, entries: Vec<TurnEntry>, changes: Vec<AgentChannelChange>) -> bool {
        let expected = entries.last().map(|e| e.entry.digest);
        for TurnEntry { entry, context } in entries {
            self.agent_channel_state.transcript.push(entry.message.clone());
            if !entry.message.internal {
                self.agent_channel_state.history.push(entry.message, Some(context));
            }
        }
        // The turn's messages got their digests continuing the transcript
        // as it was when the turn began (issue 136); had it changed
        // meanwhile, what they were produced from could not be told.
        if expected.is_some() && self.agent_channel_state.transcript.digest() != expected {
            warn!("Agent {}: its transcript changed during a turn; the contexts recorded for the turn's messages cannot be resolved", self.agent_id);
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

    /// Sends `av` to the channel as the agent publishes it: messages without
    /// what only the agent's owner sees (issues 54, 133), in the turn's
    /// contexts. The agent keeps the full
    /// messages in its context; an event with nothing left to show is not
    /// sent.
    pub async fn send_event(&self, av: ActorEvent, output: &dyn EventSender) -> Result<()> {
        let turn = self.current();
        send(&turn.event_context, &turn.security_context, self.agent_id, av, output).await
    }
}

/// The instruction to work on step `step_number` of `total_steps`.
fn plan_step_instruction(step_number: usize, total_steps: usize, step: &PlanStep) -> String {
    format!(
        "Step {} of {}: {}\n\nWork on this step now, using any tools you need. \
         When you are finished, describe the outcome.",
        step_number, total_steps, step.description
    )
}

/// The instruction to write the final answer once every step of `plan` is
/// done.
fn plan_finalize_instruction(plan: &Plan) -> String {
    let recap = plan
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {}", i + 1, s.description))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "You have completed every step of the plan:\n{}\n\nNow provide your final, \
         complete answer to the user's original request, incorporating what you learned.",
        recap
    )
}

/// Sends `av` as a turn of `agent_id` publishes it: without what only its
/// owner sees, in the turn's contexts.
async fn send(
    event_context: &EventContext,
    security_context: &SecurityContext,
    agent_id: AgentId,
    av: ActorEvent,
    output: &dyn EventSender,
) -> Result<()> {
    let Some(av) = av.published() else { return Ok(()) };
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
    use caelumora_model::message::MessageId;
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

    fn plan_of(steps: &[&str]) -> PlanState {
        PlanState {
            progress: PlanProgress::Step(0),
            plan: Plan {
                id: PlanId::new(),
                summary: "do the thing".to_string(),
                steps: steps.iter().map(|d| PlanStep { description: d.to_string() }).collect(),
            },
        }
    }

    /// What the model of `turn` is given, as role and text.
    fn given(turn: &Turn) -> Vec<(MessageRole, String)> {
        turn.full_context().unwrap().messages.into_iter().map(|m| (m.role, m.content.unwrap_or_default())).collect()
    }

    /// A turn begun on `state`, whose model called `tool` with `arguments`;
    /// the call ran if `ran`, and a note was staged.
    fn a_turn_that_called<'a>(state: &'a mut AgentChannelState, tool: &str, ran: bool) -> Turn<'a> {
        use caelumora_model::note::{NoteChange, NoteId};
        use caelumora_model::tool::{ToolCall, ToolCallResult, ToolResultStatus, ToolVisibility};
        let mut turn = Turn::new(AgentId::default(), String::new(), state, crate::agent_state::AgentStateHandle::detached(), Default::default());
        turn.begin();
        let call = ToolCall { id: Some("1".to_string()), function_name: tool.to_string(), arguments: serde_json::json!({ "to": "bob" }), visibility: ToolVisibility::SHOWN };
        let mut result = ToolCallResult::of(&call, ToolResultStatus::Ok, "sent".to_string(), Classification::CHANNEL_DEFAULT);
        result.started_at = ran.then(chrono::Utc::now);
        turn.push_message(Message { id: MessageId::new(), role: MessageRole::Assistant, tool_calls: vec![call], ..Message::new(Classification::CHANNEL_DEFAULT) });
        turn.push_message(Message { id: MessageId::new(), role: MessageRole::Tool, tool_results: vec![result], ..Message::new(Classification::CHANNEL_DEFAULT) });
        turn.stage_channel_change(AgentChannelChange::Note(NoteChange::Add { id: NoteId(1), text: "noted".to_string(), classification: Classification::CHANNEL_DEFAULT }));
        turn
    }

    fn a_state_with(content: &str) -> AgentChannelState {
        let mut state = AgentChannelState::default();
        let message = Message { id: MessageId::new(), role: MessageRole::User, content: Some(content.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        state.transcript.push(message.clone());
        state.history.push(message, None);
        state
    }

    #[test]
    fn a_turn_given_up_stays_as_a_branch_and_nothing_it_staged_is_applied() {
        let mut state = a_state_with("mail bob");
        let (transcript, history) = (state.transcript.digest(), state.history.digest());
        let mut turn = a_turn_that_called(&mut state, "send_mail", false);
        turn.give_up_turn(GivenUpReason::Stopped, None);

        assert!(state.turn.is_none(), "the turn is over");
        assert_eq!((state.transcript.digest(), state.history.digest()), (transcript, history), "both are where the turn began: nothing ran, so nothing is told");
        let turns = state.transcript.given_up_turns();
        assert_eq!(turns.len(), 1);
        assert_eq!((turns[0].given_up.reason.clone(), turns[0].messages.len()), (GivenUpReason::Stopped, 2));
        assert_eq!(state.history.all_entries().len(), 3, "its messages are in the history too, on their branch");
        assert!(state.notes.current().is_empty(), "the note it staged is not taken over");
    }

    #[test]
    fn what_ran_in_a_turn_given_up_is_told_in_full_or_by_name() {
        let mut state = a_state_with("mail bob");
        let mut turn = a_turn_that_called(&mut state, "send_mail", true);
        turn.give_up_turn(GivenUpReason::Failed { message: "boom".to_string() }, None);
        let told = state.transcript.messages().pop().unwrap();
        let text = told.content.unwrap();
        assert!(text.contains("send_mail") && text.contains("bob") && text.contains("sent"), "{text}");
        assert_eq!(told.classification, Classification::CHANNEL_DEFAULT, "as the turn");

        let mut state = a_state_with("mail bob");
        let lowered = Classification::general(caelumora_model::confidentiality::Confidentiality::Public);
        let mut turn = a_turn_that_called(&mut state, "send_mail", true);
        turn.give_up_turn(GivenUpReason::Stopped, Some(lowered.clone()));
        let told = state.transcript.messages().pop().unwrap();
        let text = told.content.unwrap();
        assert!(text.contains("send_mail") && !text.contains("bob") && !text.contains("sent"), "{text}");
        assert_eq!(told.classification, lowered, "as the channel");
    }

    #[test]
    fn a_turn_given_up_before_it_said_anything_leaves_nothing() {
        let mut state = a_state_with("hello");
        let mut turn = Turn::new(AgentId::default(), String::new(), &mut state, crate::agent_state::AgentStateHandle::detached(), Default::default());
        turn.begin();
        turn.give_up_turn(GivenUpReason::Stopped, None);
        assert!(state.transcript.given_up_turns().is_empty());
        assert_eq!(state.transcript.entries().len(), 1);
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

    #[test]
    fn advance_plan_moves_to_the_next_step_when_one_remains() {
        let mut state = AgentChannelState::default();
        state.transcript.push(Message { role: MessageRole::User, content: Some("do it".to_string()), ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) });
        let mut turn = Turn::new(AgentId::default(), String::new(), &mut state, crate::agent_state::AgentStateHandle::detached(), Default::default());
        turn.begin();
        turn.set_context(Vec::new());
        turn.set_plan(plan_of(&["first", "second"]));

        let started = turn.advance_plan(1);

        assert_eq!(turn.plan().unwrap().progress, PlanProgress::Step(1));
        assert!(turn.messages().next().is_none(), "the instruction is no message of the turn");
        let (role, text) = given(&turn).pop().expect("the instruction is given to the model");
        assert_eq!(role, MessageRole::User, "a system message in the middle, given as a user message");
        assert!(text.contains("second"));
        match started {
            Some(ActorEvent::PlanStepStarted { step_index, total_steps, description }) => {
                assert_eq!((step_index, total_steps, description.as_str()), (1, 2, "second"));
            }
            other => panic!("expected PlanStepStarted, got {other:?}"),
        }
    }

    #[test]
    fn advance_plan_finalizes_once_steps_are_exhausted() {
        let mut state = AgentChannelState::default();
        let mut turn = Turn::new(AgentId::default(), String::new(), &mut state, crate::agent_state::AgentStateHandle::detached(), Default::default());
        turn.begin();
        turn.set_plan(plan_of(&["first", "second"]));

        let started = turn.advance_plan(2);

        assert_eq!(turn.plan().unwrap().progress, PlanProgress::Finalizing);
        let (_, text) = given(&turn).pop().expect("the instruction to finalize is given to the model");
        assert!(text.contains("completed every step"));
        assert!(started.is_none(), "finalization is not a step, so nothing is announced");
    }

    #[tokio::test]
    async fn plan_instructions_are_given_where_they_were_placed_and_not_kept_for_later_turns() {
        use caelumora_model::context::SystemMessages;
        let agent = AgentId::new();
        let text = |role: MessageRole, content: &str| Message { id: MessageId::new(), sender_id: ActorId::Agent(agent), role, content: Some(content.to_string()), ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) };
        let mut state = AgentChannelState::default();
        state.transcript.push(Message { sender_id: ActorId::None, ..text(MessageRole::User, "do it") });
        let (step, answer) = (text(MessageRole::Assistant, "step done"), text(MessageRole::Assistant, "all done"));
        let keeps_system = ModelProfile { system_messages: SystemMessages::Anywhere, ..ModelProfile::default() };
        {
            let mut turn = Turn::new(agent, "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached(), keeps_system);
            turn.begin();
            turn.set_context(vec![PromptPart { role: MessageRole::System, text: "Be brief.".to_string(), after: None }]);
            turn.set_plan(plan_of(&["only"]));
            turn.advance_plan(0);
            let mut internal = step.clone();
            internal.internal = true;
            turn.push_message(internal);
            turn.advance_plan(1);
            let roles = given(&turn).into_iter().map(|(role, _)| role).collect::<Vec<_>>();
            use MessageRole::{Assistant, System, User};
            assert_eq!(roles, [System, User, System, Assistant, System], "a model that keeps them in place is given system messages");
            turn.push_message(answer.clone());
            turn.commit_turn().await;
        }

        let recorded = seen(&state, agent, answer.id).expect("the agent produced it");
        let order = recorded.iter().map(|item| match item {
            ContextItem::Prompt { text, .. } => text.lines().next().unwrap_or_default().to_string(),
            ContextItem::Conversation { message } => message.content.clone().unwrap_or_default(),
        }).collect::<Vec<_>>();
        assert_eq!(order, ["Be brief.", "do it", "Step 1 of 1: only", "step done", "You have completed every step of the plan:"],
            "each instruction where it was given");
        assert_eq!(state.transcript.messages().len(), 3, "the instructions are not in the transcript");

        let mut turn = Turn::new(agent, "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached(), keeps_system);
        turn.begin();
        turn.set_context(Vec::new());
        assert_eq!(given(&turn).len(), 3, "a later turn is not given them");
    }

    #[test]
    fn advance_plan_finalizes_immediately_for_an_empty_plan() {
        let mut state = AgentChannelState::default();
        let mut turn = Turn::new(AgentId::default(), String::new(), &mut state, crate::agent_state::AgentStateHandle::detached(), Default::default());
        turn.begin();
        turn.set_plan(plan_of(&[]));

        assert!(turn.advance_plan(0).is_none());
        assert_eq!(turn.plan().unwrap().progress, PlanProgress::Finalizing);
    }

    #[test]
    fn plan_round_trips_through_set_and_take() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut ctx = Turn::new(AgentId::default(), String::new(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached(), Default::default());
        ctx.begin();

        assert!(ctx.plan_progress().is_none());

        ctx.set_plan(make_plan_state());
        assert!(ctx.plan_progress().is_some());

        let taken = ctx.take_plan();
        assert!(taken.is_some());
        assert!(ctx.plan_progress().is_none());
    }

    #[test]
    fn a_converged_answer_finishes_its_step_and_the_last_one_asks_for_the_final_answer() {
        let mut state = AgentChannelState::default();
        let mut turn = Turn::new(AgentId::default(), String::new(), &mut state, crate::agent_state::AgentStateHandle::detached(), Default::default());
        turn.begin();
        assert!(turn.plan_converged().is_empty(), "no plan, nothing to move on");
        turn.set_plan(plan_of(&["first", "second"]));
        assert!(turn.in_plan_step());

        let kinds = |events: Vec<ActorEvent>| events.iter().map(ActorEvent::kind).collect::<Vec<_>>();
        assert_eq!(kinds(turn.plan_converged()), ["PlanStepFinished", "PlanStepStarted"]);
        assert_eq!(turn.plan_progress(), Some(PlanProgress::Step(1)));
        assert_eq!(kinds(turn.plan_converged()), ["PlanStepFinished"], "the final answer is no step");
        assert_eq!(turn.plan_progress(), Some(PlanProgress::Finalizing));
        assert!(!turn.in_plan_step(), "the final answer is not internal");
        assert!(turn.plan_converged().is_empty());
        assert!(turn.plan_progress().is_none(), "the plan is over");
    }

    #[test]
    fn next_turn_clears_a_stale_plan() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut ctx = Turn::new(AgentId::default(), String::new(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached(), Default::default());
        ctx.begin();

        ctx.set_plan(make_plan_state());
        assert!(ctx.plan().is_some());

        ctx.next_turn(EventContext::default(), SecurityContext::default(), Default::default(), Vec::new());
        assert!(ctx.plan().is_none());
    }

    /// What `state` says its agent `agent` saw when it produced message `id`.
    fn seen(state: &AgentChannelState, agent: AgentId, id: MessageId) -> Option<Vec<ContextItem>> {
        let reference = state.recorded_context_of(id)?;
        Some(ContextResolver::of(state).resolve(&reference, ActorId::Agent(agent))?.items)
    }

    #[tokio::test]
    async fn the_model_is_given_reasoning_as_its_rule_says_and_that_is_recorded() {
        use caelumora_model::context::ReasoningHistory;

        let agent = AgentId::new();
        let thought = |sender: ActorId, content: &str| Message {
            id: MessageId::new(),
            sender_id: sender,
            role: if sender == ActorId::Agent(agent) { MessageRole::Assistant } else { MessageRole::User },
            content: Some(content.to_string()),
            reasoning: Some(format!("why {content}")),
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let other = ActorId::Agent(AgentId::new());
        let mut state = AgentChannelState::default();
        // An answer of an earlier turn, and another agent's message.
        let (earlier, theirs) = (thought(ActorId::Agent(agent), "earlier"), thought(other, "theirs"));
        state.transcript.push(earlier.clone());
        state.transcript.push(theirs.clone());

        let profile = |reasoning_history| ModelProfile { reasoning_history, ..ModelProfile::default() };
        let (first, second) = (thought(ActorId::Agent(agent), "first"), thought(ActorId::Agent(agent), "second"));
        let reasoning_of = |messages: &[Message]| messages.iter().map(|m| m.reasoning.is_some()).collect::<Vec<_>>();
        {
            let mut turn = Turn::new(agent, "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached(), profile(ReasoningHistory::CurrentTurn));
            turn.begin();
            turn.set_context(Vec::new());
            turn.push_message(first.clone());
            assert_eq!(reasoning_of(&turn.full_context().unwrap().messages), [false, false, true], "only the turn's own reasoning");
            turn.push_message(second.clone());
            turn.commit_turn().await;
        }

        // What was recorded says what the model was given - not what the
        // transcript holds.
        let reasoning_seen = |state: &AgentChannelState, id| seen(state, agent, id).expect("the agent produced it").iter().map(|item| match item {
            ContextItem::Conversation { message } => message.reasoning.is_some(),
            ContextItem::Prompt { .. } => false,
        }).collect::<Vec<_>>();
        assert_eq!(reasoning_seen(&state, first.id), [false, false]);
        assert_eq!(reasoning_seen(&state, second.id), [false, false, true], "the first answer's reasoning, from the same turn");
        assert!(state.transcript.entries().all(|e| e.message.reasoning.is_some()), "stored reasoning is untouched");

        // Under `Always`, the agent's own reasoning of earlier turns too -
        // still not another agent's.
        let third = thought(ActorId::Agent(agent), "third");
        let mut turn = Turn::new(agent, "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached(), profile(ReasoningHistory::Always));
        turn.begin();
        turn.set_context(Vec::new());
        turn.push_message(third);
        assert_eq!(reasoning_of(&turn.full_context().unwrap().messages), [true, false, true, true, true]);
    }

    #[tokio::test]
    async fn what_the_agent_saw_can_be_told_for_each_message_it_produced() {
        let agent = AgentId::new();
        let text = |role: MessageRole, content: &str| Message { id: MessageId::new(), role, content: Some(content.to_string()), ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) };
        let mut state = AgentChannelState::default();
        let question = text(MessageRole::User, "What is 2+2?");
        state.transcript.push(question.clone());

        let (first, second) = (text(MessageRole::Assistant, "Let me think."), text(MessageRole::Assistant, "4"));
        let system = || PromptPart { role: MessageRole::System, text: "You are good at sums.".to_string(), after: None };
        let order = |items: &[ContextItem]| items.iter().map(|item| match item {
            ContextItem::Prompt { text, role, .. } => format!("{role:?}:{text}"),
            ContextItem::Conversation { message } => message.content.clone().unwrap_or_default(),
        }).collect::<Vec<_>>();
        {
            let mut turn = Turn::new(agent, "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached(), Default::default());
            turn.begin();
            turn.set_context(vec![system()]);
            let given = turn.full_context().unwrap().messages;
            assert_eq!(given.len(), 2, "the prompt is given to the model as a message");
            assert_eq!(given[0].role, MessageRole::System);
            turn.push_message(first.clone());
            turn.push_message(second.clone());
            turn.commit_turn().await;
        }

        assert_eq!(order(&seen(&state, agent, first.id).expect("the agent produced it")),
            ["System:You are good at sums.", "What is 2+2?"], "the first answer saw exactly what the turn gave the model");
        assert_eq!(order(&seen(&state, agent, second.id).expect("the agent produced it")),
            ["System:You are good at sums.", "What is 2+2?", "Let me think."], "everything before it, its own earlier message included");
        assert!(state.recorded_context_of(question.id).is_none(), "not produced by the agent");

        // Knowledge read in a turn counts for the messages after it.
        let (before, after) = (text(MessageRole::Assistant, "Let me look."), text(MessageRole::Assistant, "Found it."));
        let read = KnowledgeRef {
            channel: Default::default(),
            source: caelumora_model::context::KnowledgeSource::History { digest: None, start: None },
        };
        {
            let mut turn = Turn::new(agent, "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached(), Default::default());
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
            let mut turn = Turn::new(agent, "agent".to_string(), &mut state, crate::agent_state::AgentStateHandle::detached(), Default::default());
            turn.begin();
            turn.set_context(vec![system()]);
        }
        assert_eq!(state.prompts.len(), 1);
    }

    #[tokio::test]
    async fn the_tools_a_turn_began_with_are_offered_and_recorded_for_each_message() {
        use caelumora_model::tool::ToolParameters;
        let agent = AgentId::new();
        let tool = |name: &str, requires_approval| Tool {
            name: name.to_string(),
            description: format!("does {name}"),
            parameters: ToolParameters::object(Vec::<(String, ToolParameters)>::new(), Vec::<String>::new()),
            requires_approval,
            visibility: Default::default(),
            classification: None,
        };
        let answer = |content: &str| Message { id: MessageId::new(), role: MessageRole::Assistant, content: Some(content.to_string()), ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) };
        let mut state = AgentChannelState::default();
        let (first, second, third, fourth) = (answer("first"), answer("second"), answer("third"), answer("fourth"));
        let take_turn = |state: &mut AgentChannelState, tools: Vec<Tool>, messages: &[&Message]| {
            let mut turn = Turn::new(agent, "agent".to_string(), state, crate::agent_state::AgentStateHandle::detached(), Default::default());
            turn.next_turn(Default::default(), Default::default(), Default::default(), tools.clone());
            let offered = turn.full_context().unwrap().tools;
            assert_eq!(offered, tools.iter().map(Tool::definition).collect::<Vec<_>>(), "the model is offered what the turn began with");
            for message in messages {
                turn.push_message((*message).clone());
            }
            futures::executor::block_on(turn.commit_turn());
        };
        take_turn(&mut state, vec![tool("echo", false), tool("notes", true)], &[&first, &second]);
        take_turn(&mut state, Vec::new(), &[&third]);
        // Only what the model is told of a tool counts for its set.
        take_turn(&mut state, vec![tool("echo", true), tool("notes", false)], &[&fourth]);

        let offered = |id| ContextResolver::of(&state).resolve(&state.recorded_context_of(id).unwrap(), ActorId::Agent(agent))
            .expect("everything it names is here").tools.into_iter().map(|t| t.name).collect::<Vec<_>>();
        assert_eq!(offered(first.id), ["echo", "notes"]);
        assert_eq!(offered(second.id), ["echo", "notes"], "the whole turn offers the same tools");
        assert!(offered(third.id).is_empty(), "none were offered");
        assert_eq!(offered(fourth.id), ["echo", "notes"]);
        assert_eq!(state.tool_sets.len(), 1, "a set offered again is kept once");
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
}
