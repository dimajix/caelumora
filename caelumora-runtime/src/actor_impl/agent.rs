use crate::actor::{Actor, MessageContextError, TurnContext};
use crate::event::EventSubscription;
use crate::component::{ComponentContainer, ComponentContext, ComponentImpl};
use crate::component_impl::context::ContextProvider;
use crate::component_impl::message::{EmitOutProcessor, MessageProcessor, PerceiveInProcessor};
use crate::runtime::ActorRuntime;
use crate::tool::{ToolExecutor, ToolProvider};
use crate::transport::{EventReceiver, EventSender};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext, ProcessingStateHolder};
use caelumora_model::agent::{
    Agent, AgentChannelState, AgentController, AgentId, AgentState, SeenContext,
};
use caelumora_model::approval::{ApprovalId, ApprovalRequest, ApprovalStatus};
use caelumora_model::channel::{Channel, ChannelId};
use caelumora_model::component::ComponentFailPolicy;
use caelumora_model::event::GlobalEvent;
use caelumora_model::message::{Message, MessageEvent, MessageId, MessageRole};
use caelumora_model::plan::{Plan, PlanProgress, PlanStep};
use caelumora_model::security::{ActionType, SecurityContext};
use caelumora_model::store::StateStore;
use caelumora_model::tool::{ToolCall, ToolCallResult, ToolCallState, ToolResultStatus};
use futures::stream::BoxStream;
use tracing::{debug, info, warn, Instrument, Span};
use std::mem;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug)]
pub struct AgentActor {
    id: ActorId,
    kind: String,
    agent_id: AgentId,
    channel_id: ChannelId,
    agent_controller: Arc<AgentController>,
    actor_runtime: ActorRuntime,
    state_store: Arc<dyn StateStore>,

    // Components
    context_provider: Vec<ComponentImpl<dyn ContextProvider>>,
    perceive_in_processors: Vec<ComponentImpl<dyn PerceiveInProcessor>>,
    emit_out_processors: Vec<ComponentImpl<dyn EmitOutProcessor>>,
    tool_executor: Box<dyn ToolExecutor>,

    // State
    state: Mutex<AgentActorState>,
    processing_state: Arc<ProcessingStateHolder>,
    loop_count: AtomicI32,
    max_iterations: i32,
}

#[derive(Debug)]
struct AgentActorState {
    agent_id: AgentId,
    agent_channel_state: AgentChannelState,
    agent_state: AgentState,
}

impl AgentActorState {
    fn turn_context<'a>(&'a mut self) -> TurnContext<'a> {
        TurnContext::new(
            self.agent_id,
            self.agent_channel_state.turn_event_context.sender_name.clone(),
            &mut self.agent_state,
            &mut self.agent_channel_state,
        )
    }
}

impl AgentActor {
    pub async fn new(
        channel: &Channel,
        mut agent: Agent,
        actor_runtime: ActorRuntime,
    ) -> Result<Self> {
        agent.security_context.session_id = channel.session_id;
        agent.security_context.channel_id = channel.id;
        let security_context = agent.security_context.clone();
        let personality = actor_runtime
            .entity_store()
            .get_personality(&security_context, agent.personality_id)
            .await?;
        let agent_id = agent.id;
        let agent_controller = actor_runtime
            .controller_manager()
            .new_agent_controller(channel.session_id, agent)
            .await;

        let components = ComponentContainer::from_config(
            &personality.components,
            &actor_runtime.component_registry(),
            &ComponentContext::new(security_context.clone(), actor_runtime.secret_manager()),
        )
        .await?;
        let context_provider = components.get_all::<dyn ContextProvider>(None)?;
        let perceive_in_processors = components.get_all::<dyn PerceiveInProcessor>(None)?;
        let emit_out_processors = components.get_all::<dyn EmitOutProcessor>(None)?;
        let tool_providers = components.get_all::<dyn ToolProvider>(None)?;
        let tool_executor = actor_runtime.new_tool_executor(tool_providers)?;

        let state_store = actor_runtime.state_store();
        let mut agent_channel_state = state_store
            .get_agent_channel_state(agent_id, channel.id)
            .await?;
        agent_channel_state.turn_event_context.session_id = channel.session_id;
        agent_channel_state.turn_event_context.channel_id = channel.id;
        agent_channel_state.turn_event_context.sender_id = ActorId::Agent(agent_id);
        agent_channel_state.turn_event_context.sender_name = agent_controller.name();
        let agent_state = state_store.get_agent_state(agent_id).await?;
        let processing_state = agent_channel_state.processing_state.clone();
        let state = Mutex::new(AgentActorState {
            agent_id,
            agent_channel_state,
            agent_state,
        });

        Ok(AgentActor {
            id: ActorId::Agent(agent_id),
            kind: "agent".to_string(),
            agent_id,
            channel_id: channel.id,
            agent_controller,
            actor_runtime,
            state_store,
            context_provider,
            perceive_in_processors,
            emit_out_processors,
            tool_executor,
            state,
            processing_state,
            loop_count: AtomicI32::new(0),
            max_iterations: 25,
        })
    }

    async fn perceive<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        messages: Vec<Message>,
        event_context: &EventContext,
        security_context: &SecurityContext,
    ) {
        let agent_name = self.agent_controller.name();
        turn_context.next_turn(
            event_context.next(self.id, agent_name),
            security_context.next(self.id),
        );

        let original_messages = messages.clone();
        let perceived =
            Self::apply_stage_processors(&self.perceive_in_processors, messages, turn_context)
                .await
                .unwrap_or_else(|e| {
                    warn!("PerceiveIn stage failed: {}", e);
                    original_messages.clone()
                });

        let _ = perceived;
    }

    async fn respond_complete<'a>(
        &self,
        message: Message,
        turn_context: &mut TurnContext<'a>,
    ) -> Option<Message> {
        let mut output =
            Self::apply_stage_processors(&self.emit_out_processors, vec![message], turn_context)
                .await
                .ok()?;
        if output.is_empty() {
            return None;
        }

        let msg = output.remove(0);
        turn_context.push_message(msg.clone());

        Some(msg)
    }

    async fn respond_partial<'a>(
        &self,
        message: Message,
        turn_context: &mut TurnContext<'a>,
    ) -> Option<Message> {
        let mut output =
            Self::apply_stage_processors(&self.emit_out_processors, vec![message], turn_context)
                .await
                .ok()?;
        if output.is_empty() {
            return None;
        }
        Some(output.remove(0))
    }

    async fn next_turn<'a>(&self, turn_context: &mut TurnContext<'a>) -> Result<()> {
        for context_provider in &self.context_provider {
            match context_provider.facet.current_context(turn_context).await {
                Ok(_) => {}
                Err(e) => match context_provider.fail_policy {
                    ComponentFailPolicy::FailOpen => {
                        warn!(
                            "Component '{}' failed with fail-open policy: {}",
                            context_provider.id, e
                        );
                    }
                    ComponentFailPolicy::FailClosed => return Err(e),
                },
            }
        }

        Ok(())
    }

    /// Starts a new turn, transitioning to Planning. Dispatch already guarantees
    /// we're Idle, so the only thing worth checking is a cancel() that raced in
    /// while we were still parked in Idle waiting for this event.
    async fn request_response<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        event_context: &EventContext,
        event_security: &SecurityContext,
        output: &dyn EventSender,
    ) -> Result<()> {
        let agent_name = self.agent_controller.name();

        self.loop_count.store(0, Ordering::SeqCst);

        turn_context.next_turn(
            event_context.next(self.id, agent_name.clone()),
            event_security.next(self.id),
        );

        debug!("Starting generating messages for agent {} {}", self.id, agent_name);
        turn_context.send_event(ActorEvent::ResponseStarted, output).await?;

        // Initialize next turn
        self.next_turn(turn_context).await?;

        Ok(())
    }

    /// Calls the LLM for one turn step. `create_plan_tool` (when wired in)
    /// contributes its own planning nudge into `turn_context.context` as a
    /// `PromptProvider`, composed by `StateContextProvider` once per
    /// turn - nothing plan-specific needs handling here.
    async fn call_llm<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
    ) -> BoxStream<'static, crate::llm::LLMResult<MessageEvent>> {
        let llm_client = self.actor_runtime.llm_client();
        let context = turn_context.full_context();
        let tools = self.tool_executor.tools();
        llm_client
            .chat_completion(context.as_slice(), tools.as_slice())
            .await
    }

    async fn execute_tools<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
    ) -> Option<Message> {
        // Captured before running the calls: a `create_plan` call among them
        // sets the plan as a side effect of executing it, and the result
        // must be classified against the state that produced the call (same
        // as the assistant message that triggered it), not the state that
        // call itself just brought about.
        let had_plan_before = turn_context.has_plan();
        let mut tool_calls = turn_context.take_tool_calls();
        let mut complete = true;
        for c in &mut tool_calls {
            // TODO: Check for cancellation!

            // Try to execute tool if no result is available so far...
            // What the model is given for each outcome is decided in
            // `crate::tool` (issue 15): the tool's result, or a JSON object
            // saying what happened and what to do next.
            if c.result.is_none() {
                let outcome = match c.status {
                    ApprovalStatus::Approved => {
                        match self.tool_executor.execute(&c.tool_call, turn_context).await {
                            Ok(r) => Some((ToolResultStatus::Ok, r.content)),
                            Err(e) => {
                                warn!("Tool call '{}' failed: {}", c.tool_call.function_name, e);
                                let available: Vec<String> = self.tool_executor.tools().into_iter().map(|t| t.name).collect();
                                Some(e.to_result(&available))
                            }
                        }
                    }
                    ApprovalStatus::Denied => Some((ToolResultStatus::Denied, crate::tool::denied_result())),
                    ApprovalStatus::Timeout => Some((ToolResultStatus::TimedOut, crate::tool::timed_out_result())),
                    ApprovalStatus::Pending => {
                        complete = false;
                        None
                    }
                };
                if let Some((status, result)) = outcome {
                    c.result_status = status;
                    c.result = Some(result);
                }
            }
        }
        turn_context.set_tool_calls(tool_calls);

        if complete {
            let message_id = turn_context
                .last_message()
                .map(|m| m.id)
                .unwrap_or_default();

            let mut tool_results = Vec::new();
            for c in turn_context.take_tool_calls() {
                tool_results.push(ToolCallResult {
                    id: c.tool_call.id,
                    tool_name: c.tool_call.function_name,
                    content: c.result.unwrap(),
                    status: c.result_status,
                })
            }
            // A tool result is only ever "final" (shown as the real answer)
            // when it's not part of executing an in-progress plan - mirrors
            // the same internal/final rule applied to the assistant message
            // that triggered it.
            let internal = had_plan_before;
            let tool_message = Message {
                id: MessageId(uuid::Uuid::new_v4()),
                prev_id: message_id,
                sender_id: ActorId::Agent(self.agent_id),
                sender_name: self.agent_controller.name(),
                role: MessageRole::Tool,
                tool_results,
                content: None,
                timestamp: chrono::Utc::now(),
                internal,
                ..Message::default()
            };
            turn_context.push_message(tool_message.clone());
            Some(tool_message)
        } else {
            None
        }
    }

    fn plan_step_message(step_number: usize, total_steps: usize, step: &PlanStep) -> Message {
        Message {
            id: MessageId(uuid::Uuid::new_v4()),
            sender_id: ActorId::None,
            sender_name: "system".to_string(),
            role: MessageRole::System,
            content: Some(format!(
                "Step {} of {}: {}\n\nWork on this step now, using any tools you need. \
                 When you are finished, describe the outcome.",
                step_number, total_steps, step.description
            )),
            timestamp: chrono::Utc::now(),
            internal: true,
            ..Message::default()
        }
    }

    fn plan_finalize_message(plan: &Plan) -> Message {
        let recap = plan
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{}. {}", i + 1, s.description))
            .collect::<Vec<_>>()
            .join("\n");
        Message {
            id: MessageId(uuid::Uuid::new_v4()),
            sender_id: ActorId::None,
            sender_name: "system".to_string(),
            role: MessageRole::System,
            content: Some(format!(
                "You have completed every step of the plan:\n{}\n\nNow provide your final, \
                 complete answer to the user's original request, incorporating what you learned.",
                recap
            )),
            timestamp: chrono::Utc::now(),
            internal: true,
            ..Message::default()
        }
    }

    /// Pushes the instruction for `next_step_index` and sets progress to
    /// `Step(next_step_index)`, or - if there is no such step - pushes the
    /// finalize instruction and sets progress to `Finalizing`. Handles the
    /// empty-plan edge case (next_step_index == 0 immediately finalizes).
    /// Called once at approval time (next_step_index == 0) and once each
    /// time a step converges in `handle_planning`. Emits `PlanStepStarted`
    /// when a step instruction is actually pushed - finalization is not a
    /// step, so it gets no event of its own.
    async fn advance_plan<'a>(
        turn_context: &mut TurnContext<'a>,
        next_step_index: usize,
        output: &dyn EventSender,
    ) -> Result<()> {
        let Some(plan) = turn_context.plan().map(|p| p.plan.clone()) else { return Ok(()) };
        match plan.steps.get(next_step_index) {
            Some(step) => {
                turn_context.push_message(Self::plan_step_message(next_step_index + 1, plan.steps.len(), step));
                turn_context.set_plan_progress(PlanProgress::Step(next_step_index));
                turn_context.send_event(
                    ActorEvent::PlanStepStarted {
                        step_index: next_step_index,
                        total_steps: plan.steps.len(),
                        description: step.description.clone(),
                    },
                    output,
                ).await?;
            }
            None => {
                turn_context.push_message(Self::plan_finalize_message(&plan));
                turn_context.set_plan_progress(PlanProgress::Finalizing);
            }
        }
        Ok(())
    }

    /// `context` (this agent's own future-turn LLM input) keeps every message
    /// accumulated this turn, including internal deliberation/plan-execution
    /// scaffolding - it needs the full detail to stay coherent. `history`
    /// (the audit/user-facing trail - see `ChannelLoop::append_history` and
    /// `channel.history()`) only keeps genuinely final messages.
    async fn reflect<'a>(&self, turn_context: &mut TurnContext<'a>) {
        let messages = turn_context.messages().clone();
        turn_context.record_turn(messages);
    }

    fn final_messages(turn_context: &TurnContext) -> Vec<Message> {
        turn_context.messages().iter().filter(|m| !m.internal).cloned().collect()
    }

    async fn apply_stage_processors<'a, T>(
        processors: &[ComponentImpl<T>],
        mut input: Vec<Message>,
        context: &mut TurnContext<'a>,
    ) -> Result<Vec<Message>>
    where
        T: MessageProcessor + ?Sized + Send + Sync,
    {
        for stage_processor in processors {
            let current = mem::take(&mut input);
            match stage_processor
                .facet
                .process_messages(current, context)
                .await
            {
                Ok(output) => {
                    input = output;
                }
                Err(e) => match stage_processor.fail_policy {
                    ComponentFailPolicy::FailOpen => {
                        warn!(
                            "Component '{}' failed with fail-open policy: {}",
                            stage_processor.id, e
                        );
                    }
                    ComponentFailPolicy::FailClosed => return Err(e),
                },
            }
        }
        Ok(input)
    }

    async fn handle_global_event(
        &self,
        maybe_global: Option<GlobalEvent>,
        _tx: &dyn EventSender,
    ) {
        if let Some(global_event) = maybe_global {
            self.agent_controller
                .process_global_event(&global_event)
                .await;
        }
    }

    /// Idle just waits for the next event: `FullResponseGenerated` updates local
    /// context, `ResponseRequested` starts a turn. Dispatch already guarantees
    /// we're actually Idle, so no CAS is needed here.
    async fn handle_idle<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        rx: &mut Box<dyn EventReceiver>,
        global_rx: &mut EventSubscription,
        output: &dyn EventSender,
    ) -> Result<Option<ActorState>> {
        tokio::select! {
            maybe_event = rx.recv() => {
                match maybe_event {
                    Some(event) => {
                        let span = crate::logging::envelope_span(&event.context, event.event.kind());
                        let next = async {
                            debug!("Received {}", event.describe());
                            Ok::<_, anyhow::Error>(match event.event {
                                ActorEvent::FullResponseGenerated(messages) => {
                                    self.perceive(turn_context, messages, &event.context, &event.security).await;
                                    Some(ActorState::Idle)
                                }
                                ActorEvent::ResponseRequested => {
                                    match self.request_response(turn_context, &event.context, &event.security, output).await {
                                        Ok(()) => Some(ActorState::Planning),
                                        // E.g. a fail-closed context provider: the turn never starts.
                                        Err(e) => {
                                            self.fail_turn(turn_context, &e, output).await;
                                            Some(ActorState::Idle)
                                        }
                                    }
                                }
                                _ => Some(ActorState::Idle),
                            })
                        }.instrument(span).await?;
                        rx.commit().await?;
                        Ok(next)
                    }
                    // Channel is closed
                    None => Ok(None)
                }
            }
            maybe_global = global_rx.recv() => {
                self.handle_global_event(maybe_global, output).await;
                Ok(Some(ActorState::Idle))
            }
        }
    }

    /// Whether `state` belongs to a turn, i.e. an error in it aborts the turn
    /// rather than the actor.
    fn is_turn_state(state: ActorState) -> bool {
        matches!(
            state,
            ActorState::Planning
                | ActorState::ExecutingTools
                | ActorState::Reflecting
                | ActorState::Cancelling
                | ActorState::WaitingForApproval
        )
    }

    /// Ends the current turn after `error`: logs the full error, discards the
    /// turn's unfinished work (pending tool calls, plan, messages not yet
    /// sent as a final response) and tells the channel with `TurnFailed`, so
    /// that users see what went wrong instead of a turn that never ends.
    async fn fail_turn<'a>(&self, turn_context: &mut TurnContext<'a>, error: &anyhow::Error, output: &dyn EventSender) {
        let agent_name = self.agent_controller.name();
        tracing::error!("Turn of agent {} {} failed: {:#}", self.id, agent_name, error);

        turn_context.take_tool_calls();
        turn_context.take_plan();
        turn_context.finish_turn();

        let message = crate::error::user_message(error)
            .unwrap_or_else(|| "internal error - the details are in the server log".to_string());
        if let Err(e) = turn_context.send_event(ActorEvent::TurnFailed { message }, output).await {
            warn!("Agent {} {} cannot report its failed turn: {:#}", self.id, agent_name, e);
        }
    }

    /// The span one iteration of the run loop executes in. Waiting states get
    /// an `agent` span; the states of a turn get a `turn` span carrying the
    /// ids of the envelope that started the turn, so the turn can be found by
    /// the same `span_id` as the REST request and the channel dispatch.
    fn state_span(&self, state: ActorState, turn_context: &TurnContext<'_>) -> Span {
        let agent = self.agent_controller.name();
        match state {
            ActorState::Idle | ActorState::Sleeping => {
                tracing::info_span!("agent", agent_id = %self.agent_id, agent = %agent, state = ?state)
            }
            _ => {
                let ctx = turn_context.event_context();
                tracing::info_span!(
                    "turn",
                    agent_id = %self.agent_id,
                    agent = %agent,
                    state = ?state,
                    session_id = %ctx.session_id,
                    channel_id = %ctx.channel_id,
                    span_id = %ctx.span_id,
                    event_id = %ctx.event_id,
                    correlation_id = %ctx.correlation_id,
                    depth = ctx.span_depth,
                )
            }
        }
    }

    /// `Sleep` differs from `Idle` in that it deliberately does *not* react to
    /// `ResponseRequested` at all (only an external `wakeup()` call ends a sleep) -
    /// it only keeps local context current via `FullResponseGenerated`.
    async fn handle_sleep<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        rx: &mut Box<dyn EventReceiver>,
        global_rx: &mut EventSubscription,
        output: &dyn EventSender,
    ) -> Result<Option<ActorState>> {
        tokio::select! {
            maybe_event = rx.recv() => {
                match maybe_event {
                    Some(event) => {
                        if let ActorEvent::FullResponseGenerated(messages) = event.event {
                            self.perceive(turn_context, messages, &event.context, &event.security).await;
                        }
                        rx.commit().await?;
                        Ok(Some(self.state_after_sleep()))
                    }
                    // Channel is closed
                    None => Ok(None)
                }
            }
            maybe_global = global_rx.recv() => {
                self.handle_global_event(maybe_global, output).await;
                Ok(Some(self.state_after_sleep()))
            }
        }
    }

    /// The state to continue with after `handle_sleep` consumed an event. A
    /// `wakeup()` while `handle_sleep` was blocked on its inputs has already
    /// moved the processing state to `Idle`; returning a hard `Sleeping` would
    /// overwrite that and put the agent straight back to sleep - so a
    /// mentioned agent would perceive the message waking it, then ignore the
    /// `ResponseRequested` that follows.
    fn state_after_sleep(&self) -> ActorState {
        match self.processing_state.load() {
            ActorState::Idle => ActorState::Idle,
            _ => ActorState::Sleeping,
        }
    }

    /// `Silence` and `Cancelling` are momentary: the run loop dispatches to them
    /// once (so the reason is observable for a beat), and they immediately fall
    /// through to `Idle` without consuming any event.
    fn handle_stopping(&self) -> Result<ActorState> {
        info!("Agent {} {} stopped, not reflecting", self.id, self.agent_controller.name());
        Ok(ActorState::Reflecting)
    }

    async fn handle_cancelling<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        output: &dyn EventSender,
    ) -> Result<ActorState> {
        self.reflect(turn_context).await;

        // Always signal the end of the turn, even with no final messages -
        // a plan cancelled before any step produced a visible answer still
        // has to tell observers (e.g. the web UI) that generation stopped,
        // the same way handle_reflecting does unconditionally.
        turn_context.send_event(
            ActorEvent::FullResponseGenerated(Self::final_messages(turn_context)),
            output,
        ).await?;
        turn_context.finish_turn();

        info!("Agent {} {} finished cancelling", self.id, self.agent_controller.name());

        Ok(ActorState::Idle)
    }

    /// Handle a single approval event while in WaitingForApproval state.
    /// Records the decision; the run loop dispatches the actual tool execution
    /// on its next iteration via ExecutingTools.
    async fn handle_waiting_for_approval<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        rx: &mut Box<dyn EventReceiver>,
        global_rx: &mut EventSubscription,
        output: &dyn EventSender,
    ) -> Result<Option<ActorState>> {
        let is_approval_event = |env: &Envelope| {
            matches!(
                env.event,
                ActorEvent::ApprovalGranted(_)
                    | ActorEvent::ApprovalDenied(_)
                    | ActorEvent::ApprovalTimedOut(_)
            )
        };

        tokio::select! {
            maybe_event = rx.recv_matching(is_approval_event) => {
                match maybe_event {
                    Some(event) => {
                        let (req, status) = match &event.event {
                            ActorEvent::ApprovalGranted(r) => (r, ApprovalStatus::Approved),
                            ActorEvent::ApprovalDenied(r) => (r, ApprovalStatus::Denied),
                            ActorEvent::ApprovalTimedOut(r) => (r, ApprovalStatus::Timeout),
                            _ => unreachable!("handle_approval_event called with non-approval event"),
                        };

                        if !turn_context.set_tool_approval_status(req.id, status) {
                            warn!("Received approval for unknown approval id {}", req.id);
                        }

                        rx.commit().await?;
                        Ok(Some(ActorState::ExecutingTools))
                    }
                    // Channel is closed
                    None => Ok(None),
                }
            }
            maybe_global = global_rx.recv() => {
                self.handle_global_event(maybe_global, output).await;
                Ok(Some(ActorState::WaitingForApproval))
            }
        }
    }

    /// Registers tool calls for approval and requests approval for the ones that
    /// need it. Purely records data on `turn_context` - it never touches
    /// `processing_state`; `handle_planning`'s return value (always
    /// `ExecutingTools` when there are tool calls) and `handle_executing_tools`
    /// (which decides `Planning` vs `WaitingForApproval` once it knows which
    /// calls actually still need a decision) are the sole authority on state.
    async fn request_approvals<'a>(
        &self,
        tool_calls: &Vec<ToolCall>,
        turn_context: &mut TurnContext<'a>,
        output: &dyn EventSender,
    ) -> Result<()> {
        let tools = self.tool_executor.tools();
        let mut states: Vec<ToolCallState> = Vec::with_capacity(tool_calls.len());
        for tc in tool_calls {
            let tool = tools.iter().find(|t| t.name == tc.function_name);
            let approval = tool.map(|t| t.requires_approval).unwrap_or(false);
            let description = self.tool_executor.describe(tc).unwrap_or("No description".to_string());
            let mut state = ToolCallState {
                id: ApprovalId::new(),
                status: if approval { ApprovalStatus::Pending } else { ApprovalStatus::Approved },
                tool_call: tc.clone(),
                description,
                result: None,
                result_status: ToolResultStatus::Ok,
            };
            // A call the policy forbids is refused right away rather than
            // put to a user who could only approve it in vain. The status
            // then no longer matters: a call with a result is not executed.
            if !self.tool_executor.is_permitted(tc, turn_context).await {
                info!("Tool call '{}' by agent {} forbidden by the security manager", tc.function_name, self.id);
                state.status = ApprovalStatus::Approved;
                state.result = Some(crate::tool::forbidden_result());
                state.result_status = ToolResultStatus::Forbidden;
            }
            states.push(state);
        }
        let tool_calls = states;

        if tool_calls.is_empty() {
            return Ok(());
        }

        turn_context.set_tool_calls(tool_calls);

        for tc in turn_context.tool_calls() {
            if tc.status == ApprovalStatus::Pending {
                let request = ApprovalRequest {
                    id: tc.id,
                    session_id: turn_context.session_id,
                    channel_id: turn_context.channel_id,
                    agent_id: self.agent_id,
                    tool_call: tc.tool_call.clone(),
                    description: tc.description.clone(),
                };
                turn_context.send_event(ActorEvent::ApprovalRequested(request), output).await?;
            }
        }

        Ok(())
    }

    /// Calls the LLM and streams its response for a single turn step. Loops
    /// back to itself (via the run loop, after a trip through
    /// `ExecutingTools`) as long as the model keeps making tool calls, up to
    /// `max_iterations`.
    ///
    /// A message is "final" (shown to the user, kept in `history`) exactly
    /// when it isn't part of executing an in-progress plan (`executing_plan`,
    /// and not yet at the last, synthesizing step) - ordinary tool calls,
    /// pre-plan research, and the `create_plan` call itself are all shown
    /// like any other message, same as a personality with no planning
    /// capability at all. Anything internal instead gets
    /// `IntermediatePartialGenerated`/`IntermediateMessageGenerated` instead
    /// of the `Partial`/`Complete` variants, and is excluded from `history`
    /// in `reflect()`.
    async fn handle_llm_turn<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        output: &dyn EventSender,
    ) -> Result<ActorState> {
        let agent_name = self.agent_controller.name();

        if self.loop_count.load(Ordering::SeqCst) >= self.max_iterations {
            warn!("Too many iterations, stopping agent {} {}", self.id, agent_name);
            return Ok(ActorState::Reflecting);
        }
        self.loop_count.fetch_add(1, Ordering::SeqCst);

        // A plan can only ever exist here because its create_plan tool call
        // already went through the generic tool-call approval gate (see
        // CreatePlanToolProvider::execute) - once one is in play, its
        // messages must be distinguishable from a genuine final response
        // until the model stops calling tools.
        let executing_plan = turn_context.has_plan();
        // The dedicated synthesis call (after the last step converged) is
        // still `executing_plan`, but its streamed text *is* the final
        // answer and must reach the user live, not be swallowed as internal
        // plan scaffolding.
        let finalizing_plan = matches!(
            turn_context.plan().map(|p| p.progress),
            Some(PlanProgress::Finalizing)
        );
        let internal_message = executing_plan && !finalizing_plan;

        let mut response = self.call_llm(turn_context).await;
        use futures::stream::TryStreamExt;
        while let Some(event) = response
            .try_next()
            .await
            .with_context(|| "Error during LLM stream".to_string())?
        {
            if self.processing_state.load() == ActorState::Cancelling {
                info!("Stopping for cancelled agent {} {}", self.id, agent_name);
                return Ok(ActorState::Cancelling);
            }

            match event {
                MessageEvent::Partial(mut message) => {
                    message.sender_name = agent_name.clone();
                    message.sender_id = self.id;
                    message.internal = internal_message;
                    if let Some(msg) = self.respond_partial(message, turn_context).await {
                        let event = if msg.internal {
                            ActorEvent::IntermediatePartialGenerated(msg)
                        } else {
                            ActorEvent::PartialMessageGenerated(msg)
                        };
                        turn_context.send_event(event, output).await?;
                    }
                }
                MessageEvent::Complete(mut message) => {
                    message.sender_name = agent_name.clone();
                    message.sender_id = self.id;
                    message.internal = internal_message;

                    self.request_approvals(&message.tool_calls, turn_context, output).await?;
                    let has_tool_calls = !message.tool_calls.is_empty();

                    if executing_plan && !has_tool_calls {
                        match turn_context.plan().map(|p| p.progress) {
                            Some(PlanProgress::Finalizing) => {
                                // The dedicated synthesis call converged -
                                // this is the real, final response.
                                turn_context.take_plan();
                                if let Some(msg) = self.respond_complete(message, turn_context).await {
                                    turn_context.send_event(ActorEvent::CompleteMessageGenerated(msg), output).await?;
                                }
                            }
                            Some(PlanProgress::Step(idx)) => {
                                // This step converged - not final. Its result
                                // is already part of context (respond_complete
                                // pushes it); move on to the next step, or to
                                // finalization if this was the last one.
                                if let Some(msg) = self.respond_complete(message, turn_context).await {
                                    turn_context.send_event(ActorEvent::IntermediateMessageGenerated(msg), output).await?;
                                }
                                let finished_step = turn_context.plan()
                                    .map(|p| (p.plan.steps[idx].description.clone(), p.plan.steps.len()));
                                if let Some((description, total_steps)) = finished_step {
                                    turn_context.send_event(
                                        ActorEvent::PlanStepFinished { step_index: idx, total_steps, description },
                                        output,
                                    ).await?;
                                }
                                Self::advance_plan(turn_context, idx + 1, output).await?;
                                return Ok(ActorState::Planning);
                            }
                            None => unreachable!("executing_plan implies a plan is present"),
                        }
                    } else {
                        // Covers: the ordinary single-shot tool loop, pre-plan
                        // research/create_plan calls, and a direct answer
                        // before committing to a plan - all final/visible;
                        // mid-plan research/extension tool calls stay internal.
                        message.internal = internal_message;
                        if let Some(msg) = self.respond_complete(message, turn_context).await {
                            let event = if msg.internal {
                                ActorEvent::IntermediateMessageGenerated(msg)
                            } else {
                                ActorEvent::CompleteMessageGenerated(msg)
                            };
                            turn_context.send_event(event, output).await?;
                        }
                    }

                    if turn_context.has_tool_calls() {
                        return Ok(ActorState::ExecutingTools);
                    }
                }
                MessageEvent::EndOfTurn(_) => {}
            }
        }

        Ok(ActorState::Reflecting)
    }

    /// Runs any tool calls that are ready (Approved, or newly-approved on resume
    /// from WaitingForApproval). Loops back to Planning once all calls for this
    /// step have results, or parks in WaitingForApproval if some are still
    /// Pending.
    async fn handle_executing_tools<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        output: &dyn EventSender,
    ) -> Result<ActorState> {
        let progress_before = turn_context.plan().map(|p| p.progress);
        if let Some(tool_message) = self.execute_tools(turn_context).await {
            let event = if tool_message.internal {
                ActorEvent::IntermediateMessageGenerated(tool_message)
            } else {
                ActorEvent::CompleteMessageGenerated(tool_message)
            };
            turn_context.send_event(event, output).await?;

            // A plan was just created, or extended after already reaching
            // Finalizing, by a create_plan call in this batch (progress only
            // ever changes here as a result of that - a plain append to an
            // already-in-progress plan leaves progress untouched). Announce
            // the step it now points at (or finalize immediately for an
            // empty plan) now that the tool-result message reporting the
            // create_plan call is already in place - this must not happen
            // inside CreatePlanToolProvider::execute itself, since that
            // would land a message between the assistant's tool-call message
            // and its own tool result.
            let progress_after = turn_context.plan().map(|p| p.progress);
            if progress_after != progress_before {
                if let Some(PlanProgress::Step(idx)) = progress_after {
                    Self::advance_plan(turn_context, idx, output).await?;
                }
            }

            // Handle "Silent" requested state
            if turn_context.requested_state() == Some(ActorState::Stopping) {
                // "Stopping" is a one-time state, so we remove it
                turn_context.take_requested_state();
                Ok(ActorState::Stopping)
            }
            else {
                Ok(ActorState::Planning)
            }
        } else {
            Ok(ActorState::WaitingForApproval)
        }
    }

    /// Persists context/history and closes out the turn, then returns to
    /// whatever was requested during the turn (Sleep/Silence/Cancelling),
    /// defaulting to Idle.
    async fn handle_reflecting<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        output: &dyn EventSender,
    ) -> Result<ActorState> {
        let agent_name = self.agent_controller.name();
        self.reflect(turn_context).await;

        turn_context.send_event(
            ActorEvent::FullResponseGenerated(Self::final_messages(turn_context)),
            output,
        ).await?;
        turn_context.finish_turn();
        debug!("Finished generating messages for agent {} {}", self.id, agent_name);

        // Use requested state
        Ok(turn_context.take_requested_state().unwrap_or(ActorState::Idle))
    }
}


/// What `state` says the agent saw when it produced `message`.
fn context_of(state: &AgentChannelState, actor: ActorId, message: MessageId) -> Result<SeenContext, MessageContextError> {
    let reference = state.recorded_context_of(message)
        .ok_or(MessageContextError::NotRecorded { actor, message })?;
    state.resolve(&reference).ok_or(MessageContextError::Unavailable { actor, message })
}

#[async_trait]
impl Actor for AgentActor {
    fn id(&self) -> ActorId {
        self.id
    }
    fn kind(&self) -> String {
        self.kind.clone()
    }
    fn name(&self) -> String {
        self.agent_controller.name()
    }

    fn event_filter(&self) -> fn(&Envelope) -> bool {
        |event| {
            matches!(
                event.event,
                ActorEvent::ApprovalGranted(_)
                    | ActorEvent::ApprovalDenied(_)
                    | ActorEvent::ApprovalTimedOut(_)
                    | ActorEvent::FullResponseGenerated(_)
                    | ActorEvent::ResponseRequested
            )
        }
    }

    async fn run(
        &self,
        mut rx: Box<dyn EventReceiver>,
        output: Box<dyn EventSender>,
    ) -> Result<()> {
        let mut state = self
            .state
            .try_lock()
            .map_err(|_| anyhow!("AgentActor {} already running", self.id()))?;

        info!("AgentActor {} is starting", self.id());

        let mut global_rx = self
            .actor_runtime
            .event_bus()
            .subscribe_agent(self.agent_id);

        loop {
            let current_state = self.processing_state.load();
            let mut turn_context = state.turn_context();

            // Idle/WaitingForApproval/Sleep may consume no event before the actor's
            // input channel closes (Ok(None)) - that's the only way this loop ends.
            // Every other state always produces a next state.
            let span = self.state_span(current_state, &turn_context);
            let result = async { Ok::<_, anyhow::Error>(match current_state {
                ActorState::Idle => {
                    self.handle_idle(&mut turn_context, &mut rx, &mut global_rx, &*output).await?
                }
                ActorState::WaitingForApproval => {
                    self.handle_waiting_for_approval(&mut turn_context, &mut rx, &mut global_rx, &*output).await?
                }
                ActorState::Sleeping => {
                    self.handle_sleep(&mut turn_context, &mut rx, &mut global_rx, &*output).await?
                }
                ActorState::Stopping => Some(self.handle_stopping()?),
                ActorState::Cancelling => Some(self.handle_cancelling(&mut turn_context, &*output).await?),
                ActorState::Planning => Some(self.handle_llm_turn(&mut turn_context, &*output).await?),
                ActorState::ExecutingTools => Some(self.handle_executing_tools(&mut turn_context, &*output).await?),
                ActorState::Reflecting => Some(self.handle_reflecting(&mut turn_context, &*output).await?),
                ActorState::Active => unreachable!("AgentActor never assigns itself ActorState::Active"),
            }) }.instrument(span.clone()).await;

            // A failure inside a turn ends the turn, not the agent (issue 02).
            // Errors in the waiting states are the transport's and still end
            // the actor.
            let next_state = match result {
                Ok(next) => next,
                Err(e) if Self::is_turn_state(current_state) => {
                    self.fail_turn(&mut turn_context, &e, &*output).instrument(span).await;
                    Some(ActorState::Idle)
                }
                Err(e) => return Err(e),
            };

            turn_context.store_agent_state(&self.state_store).await;
            turn_context.store_channel_state(&self.state_store).await;

            match next_state {
                Some(next) => {
                    // If state has been switched to "Cancelling", but "current_state" was not
                    // "Cancelling", then ignore "next_state"
                    if current_state == ActorState::Cancelling || self.processing_state.load() != ActorState::Cancelling {
                        self.processing_state.store(next)
                    }
                },
                // Channel is closed, actor wants to quit
                None => break,
            }
        }

        info!("AgentActor {} has finished", self.id());
        Ok(())
    }

    fn state(&self) -> ActorState {
        self.processing_state.load()
    }

    async fn wakeup(&self) -> Result<()> {
        // Called for every agent on every user message, sleeping or not.
        debug!(
            "Waking up agent {} {}",
            self.id,
            self.agent_controller.name()
        );
        // Only a sleeping agent should be woken; forcing Idle from any other
        // state (e.g. mid-Planning) would corrupt an in-flight turn.
        self.processing_state.transition(|s| match s {
            ActorState::Sleeping => Some(ActorState::Idle),
            _ => None,
        });
        Ok(())
    }

    async fn message_context(&self, caller: &SecurityContext, message: MessageId) -> Result<SeenContext, MessageContextError> {
        // Read from the store, where the state is written after every step:
        // the running loop holds the in-memory state for as long as it runs.
        if !self.may_read_context(caller).await {
            info!("Refused reading the context of agent {} for {}", self.id, caller.initiator_id);
            return Err(MessageContextError::Forbidden { actor: self.id });
        }
        let state = self.state_store.get_agent_channel_state(self.agent_id, self.channel_id).await
            .map_err(|e| {
                warn!("Cannot read the state of agent {} in channel {}: {}", self.agent_id, self.channel_id, e);
                MessageContextError::Failed { actor: self.id, message }
            })?;
        context_of(&state, self.id, message)
    }

    async fn may_read_context(&self, caller: &SecurityContext) -> bool {
        let attributes = self.agent_controller.load().security_attributes.clone();
        self.actor_runtime.security_manager().check(caller, ActionType::ReadAgentContext(&attributes)).await
    }

    async fn cancel(&self) -> Result<()> {
        info!(
            "Cancelling agent {} {}",
            self.id,
            self.agent_controller.name()
        );
        self.processing_state.transition(|s| match s {
            ActorState::WaitingForApproval | ActorState::Active | ActorState::ExecutingTools | ActorState::Planning
                => Some(ActorState::Cancelling),
            _ => None,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;
    use crate::component_impl::context::{ContextProvider, PromptPosition, StateContextAppender, StateContextProvider, StaticPromptProvider};
    use crate::component_impl::history::StateHistoryWriter;
    use crate::component_impl::message::MessageProcessor;
    use caelumora_model::message::MessageRole;
    use caelumora_model::plan::{PlanId, PlanState};
    use chrono::Utc;
    use uuid::Uuid;

    fn make_message(content: &str) -> Message {
        Message {
            id: MessageId(Uuid::new_v4()),
            prev_id: MessageId::default(),
            sender_id: ActorId::None,
            sender_name: "tester".to_string(),
            role: MessageRole::User,
            content: Some(content.to_string()),
            tool_calls: Vec::new(),
            timestamp: Utc::now(),
            ..Message::default()
        }
    }

    #[tokio::test]
    async fn state_history_writer_is_passthrough_and_persists_history() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut agent_state = AgentState::default();
        let mut turn_context = TurnContext::new(
            AgentId::default(),
            String::new(),
            &mut agent_state,
            &mut agent_channel_state,
        );
        let processor = StateHistoryWriter {};
        let input = vec![make_message("hello")];

        let output = processor
            .process_messages(input.clone(), &mut turn_context)
            .await
            .expect("history processor should succeed");

        assert_eq!(output.len(), input.len());
        assert_eq!(output[0].id, input[0].id);

        let history = agent_channel_state.history.messages();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, input[0].id);
    }

    #[tokio::test]
    async fn state_context_appender_is_passthrough_and_updates_provider_context() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut agent_state = AgentState::default();
        let mut turn_context = TurnContext::new(
            AgentId::default(),
            String::new(),
            &mut agent_state,
            &mut agent_channel_state,
        );
        let appender = StateContextAppender {};
        let provider = StateContextProvider {
            prompt_providers: vec![],
        };
        let input = vec![make_message("ctx")];

        let output = appender
            .process_messages(input.clone(), &mut turn_context)
            .await
            .expect("context appender should succeed");

        assert_eq!(output.len(), input.len());
        assert_eq!(output[0].id, input[0].id);

        provider
            .current_context(&mut turn_context)
            .await
            .expect("context provider should succeed");
        assert_eq!(turn_context.context().len(), 1);
        assert_eq!(turn_context.context()[0].id, input[0].id);
    }

    #[tokio::test]
    async fn prompt_parts_go_where_their_position_says_and_only_the_conversation_is_stored() {
        let agent_id = AgentId::new();
        let mut agent_channel_state = AgentChannelState::default();
        let mut agent_state = AgentState::default();
        let provider = StateContextProvider {
            prompt_providers: vec![
                Arc::new(StaticPromptProvider { prompt: "Be brief.".to_string(), position: PromptPosition::Start }),
                Arc::new(StaticPromptProvider { prompt: "Notes: none.".to_string(), position: PromptPosition::BeforeTurnInput }),
            ],
        };
        let contents = |turn: &TurnContext| turn.context().iter()
            .map(|m| format!("{:?}:{}", m.role, m.content.clone().unwrap_or_default()))
            .collect::<Vec<_>>();

        let mut turn_context = TurnContext::new(agent_id, "agent".to_string(), &mut agent_state, &mut agent_channel_state);
        let mut earlier = make_message("earlier");
        earlier.role = MessageRole::User;
        let mut question = make_message("question");
        question.role = MessageRole::User;
        turn_context.extend_state_context(vec![earlier, question]);
        provider.current_context(&mut turn_context).await.unwrap();
        assert_eq!(contents(&turn_context), ["System:Be brief.", "User:earlier", "User:Notes: none.", "User:question"],
            "the system prompt first, the notes before the turn's input");
        assert_eq!(turn_context.state_context().len(), 2, "prompt parts are never stored in the context");

        // A turn without new input - the newest message is the agent's own.
        let mut own = make_message("my answer");
        own.sender_id = ActorId::Agent(agent_id);
        turn_context.extend_state_context(vec![own]);
        provider.current_context(&mut turn_context).await.unwrap();
        assert_eq!(contents(&turn_context).last().map(String::as_str), Some("User:Notes: none."), "then at the end");
    }

    fn make_plan_state(steps: Vec<&str>) -> PlanState {
        PlanState {
            progress: PlanProgress::Step(0),
            plan: Plan {
                id: PlanId::new(),
                summary: "do the thing".to_string(),
                steps: steps.into_iter().map(|d| PlanStep { description: d.to_string() }).collect(),
            },
        }
    }

    #[tokio::test]
    async fn advance_plan_moves_to_the_next_step_when_one_remains() {
        let mut agent_state = AgentState::default();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);
        turn_context.set_plan(make_plan_state(vec!["first", "second"]));
        let (tx, mut rx) = mpsc::channel(10);
        let output = crate::transport_impl::InMemoryEventSender::new(tx);

        AgentActor::advance_plan(&mut turn_context, 1, &output).await.unwrap();

        assert_eq!(turn_context.plan().unwrap().progress, PlanProgress::Step(1));
        let last = turn_context.messages().last().expect("a step message should have been pushed");
        assert_eq!(last.role, MessageRole::System);
        assert!(last.content.as_ref().unwrap().contains("second"));
        assert!(last.internal, "plan-step instructions are scaffolding, never final answers");

        let event = rx.try_recv().expect("a PlanStepStarted event should have been sent");
        match event.event {
            ActorEvent::PlanStepStarted { step_index, total_steps, description } => {
                assert_eq!(step_index, 1);
                assert_eq!(total_steps, 2);
                assert_eq!(description, "second");
            }
            other => panic!("expected PlanStepStarted, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn advance_plan_finalizes_once_steps_are_exhausted() {
        let mut agent_state = AgentState::default();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);
        turn_context.set_plan(make_plan_state(vec!["first", "second"]));
        let (tx, mut rx) = mpsc::channel(10);
        let output = crate::transport_impl::InMemoryEventSender::new(tx);

        AgentActor::advance_plan(&mut turn_context, 2, &output).await.unwrap();

        assert_eq!(turn_context.plan().unwrap().progress, PlanProgress::Finalizing);
        let last = turn_context.messages().last().expect("a finalize message should have been pushed");
        assert_eq!(last.role, MessageRole::System);
        assert!(last.content.as_ref().unwrap().contains("completed every step"));

        assert!(rx.try_recv().is_err(), "finalization is not a step, so no PlanStepStarted should be sent");
    }

    #[tokio::test]
    async fn advance_plan_finalizes_immediately_for_an_empty_plan() {
        let mut agent_state = AgentState::default();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);
        turn_context.set_plan(make_plan_state(vec![]));
        let (tx, _rx) = mpsc::channel(10);
        let output = crate::transport_impl::InMemoryEventSender::new(tx);

        AgentActor::advance_plan(&mut turn_context, 0, &output).await.unwrap();

        assert_eq!(turn_context.plan().unwrap().progress, PlanProgress::Finalizing);
    }

    #[test]
    fn final_messages_filters_out_internal_messages() {
        let mut agent_state = AgentState::default();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);

        let mut internal = make_message("working on it");
        internal.internal = true;
        let mut final_answer = make_message("here you go");
        final_answer.internal = false;

        turn_context.push_message(internal);
        turn_context.push_message(final_answer.clone());

        let final_messages = AgentActor::final_messages(&turn_context);
        assert_eq!(final_messages.len(), 1);
        assert_eq!(final_messages[0].id, final_answer.id);
    }

    #[test]
    fn a_message_context_is_found_only_for_what_the_agent_produced_and_can_resolve() {
        use caelumora_model::agent::SeenItem;
        use caelumora_model::digest::prompt;
        use caelumora_model::message::{ContextRef, MessageRole, PromptRef};

        let actor = ActorId::Agent(AgentId::new());
        let text = |content: &str| Message { id: MessageId::new(), content: Some(content.to_string()), ..Message::default() };
        let mut state = AgentChannelState::default();
        let question = text("question");
        state.context.push(question.clone());
        state.history.extend(vec![question.clone()]);
        let answer = text("answer");
        let before = state.context.push(answer.clone());
        state.history.push(answer.clone(), Some(ContextRef { context: before, prompts: Vec::new() }));

        let seen = context_of(&state, actor, answer.id).expect("the agent produced it");
        let ids: Vec<MessageId> = seen.items.iter().filter_map(|item| match item {
            SeenItem::Message(m) => Some(m.id),
            SeenItem::Prompt { .. } => None,
        }).collect();
        assert_eq!(ids, vec![question.id]);
        assert!(matches!(context_of(&state, actor, question.id), Err(MessageContextError::NotRecorded { .. })), "someone else wrote it");

        // A reference to a prompt that was never kept cannot be resolved.
        let lost = text("lost");
        let gone = PromptRef { text: prompt("gone"), role: MessageRole::System, after: None };
        state.history.push(lost.clone(), Some(ContextRef { context: before, prompts: vec![gone] }));
        assert!(matches!(context_of(&state, actor, lost.id), Err(MessageContextError::Unavailable { .. })));
    }
}
