use crate::actor::{Actor, TurnContext};
use crate::component::{ComponentContainer, ComponentImpl};
use crate::component_impl::context::ContextProvider;
use crate::component_impl::message::{EmitOutProcessor, MessageProcessor, PerceiveInProcessor};
use crate::runtime::ActorRuntime;
use crate::tool::{ToolExecutor, ToolProvider};
use crate::transport::{EventReceiver, EventSender};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext, ProcessingStateHolder};
use caelumora_model::agent::{
    Agent, AgentChannelState, AgentController, AgentId, AgentState,
};
use caelumora_model::approval::{ApprovalId, ApprovalRequest, ApprovalStatus};
use caelumora_model::channel::Channel;
use caelumora_model::component::ComponentFailPolicy;
use caelumora_model::event::GlobalEvent;
use caelumora_model::message::{Message, MessageEvent, MessageId, MessageRole};
use caelumora_model::plan::{Plan, PlanApprovalRequest, PlanApprovalResponse, PlanId, PlanProgress, PlanState, PlanStep};
use caelumora_model::security::SecurityContext;
use caelumora_model::store::StateStore;
use caelumora_model::tool::{Tool, ToolCall, ToolCallResult, ToolCallState, ToolParameters};
use futures::stream::BoxStream;
use log::{debug, info, warn};
use serde::Deserialize;
use std::mem;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

#[derive(Debug)]
pub struct AgentActor {
    id: ActorId,
    kind: String,
    agent_id: AgentId,
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
    enable_extended_planning: bool,
}

const CREATE_PLAN_TOOL_NAME: &str = "create_plan";

const PLANNING_SYSTEM_PROMPT: &str = "Before responding, decide whether this request needs \
research or multiple steps to complete. If it does, gather any information you need using \
the available tools, then call `create_plan` once with a short summary and an ordered list \
of the steps you will take to fulfill the request. The plan will be shown to the user for \
approval before you execute it. For simple requests that need no research and no multiple \
steps, skip `create_plan` entirely and just answer directly.";

fn create_plan_tool() -> Tool {
    Tool {
        name: CREATE_PLAN_TOOL_NAME.to_string(),
        description: "Submit your structured multi-step plan for the user's request. Call \
            this once you have gathered enough information (using other tools if needed) to \
            describe how you will address the request. For simple requests that need no \
            research or multiple steps, just answer directly instead of calling this."
            .to_string(),
        parameters: ToolParameters::object(
            [
                ("summary", ToolParameters::string("One-sentence summary of the overall approach")),
                (
                    "steps",
                    ToolParameters::array(
                        ToolParameters::object(
                            [("description", ToolParameters::string("What this step will do"))],
                            ["description"],
                        ),
                        "Ordered list of steps",
                    ),
                ),
            ],
            ["summary", "steps"],
        ),
        requires_approval: false,
    }
}

#[derive(Deserialize)]
struct CreatePlanArgs {
    summary: String,
    steps: Vec<CreatePlanStepArg>,
}

#[derive(Deserialize)]
struct CreatePlanStepArg {
    description: String,
}

fn parse_plan(call: &ToolCall) -> Result<Plan> {
    let args: CreatePlanArgs = serde_json::from_value(call.arguments.clone())
        .context("Failed to parse create_plan arguments")?;

    Ok(Plan {
        id: PlanId::new(),
        summary: args.summary,
        steps: args
            .steps
            .into_iter()
            .map(|s| PlanStep { description: s.description })
            .collect(),
    })
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
            &actor_runtime,
            security_context.clone(),
        )
        .await?;
        let context_provider = components.get_all::<dyn ContextProvider>(None)?;
        let perceive_in_processors = components.get_all::<dyn PerceiveInProcessor>(None)?;
        let emit_out_processors = components.get_all::<dyn EmitOutProcessor>(None)?;
        let tool_providers = components.get_all::<dyn ToolProvider>(None)?;
        let tool_executor = actor_runtime
            .new_tool_executor(tool_providers.into_iter().map(|p| p.facet).collect());

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
            enable_extended_planning: personality.enable_extended_planning,
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

        info!("Starting generating messages for agent {} {}", self.id, agent_name);
        turn_context.send_event(ActorEvent::ResponseStarted, output).await?;

        // Initialize next turn
        self.next_turn(turn_context).await?;

        Ok(())
    }

    async fn plan<'a>(
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

    /// Like `plan()`, but for the deliberation step: appends an ephemeral
    /// (not persisted) planning-mode system message and offers the `create_plan`
    /// pseudo-tool alongside the agent's real tools.
    async fn deliberate<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
    ) -> BoxStream<'static, crate::llm::LLMResult<MessageEvent>> {
        let llm_client = self.actor_runtime.llm_client();

        let mut context = turn_context.full_context();
        context.push(Message {
            role: MessageRole::System,
            content: Some(PLANNING_SYSTEM_PROMPT.to_string()),
            sender_id: self.id,
            sender_name: self.agent_controller.name(),
            ..Message::default()
        });

        let mut tools = self.tool_executor.tools();
        tools.push(create_plan_tool());

        llm_client
            .chat_completion(context.as_slice(), tools.as_slice())
            .await
    }

    async fn execute_tools<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
    ) -> Option<Message> {
        let mut tool_calls = turn_context.take_tool_calls();
        let mut complete = true;
        for c in &mut tool_calls {
            // TODO: Check for cancellation!

            // Try to execute tool if no result is available so far...
            if c.result.is_none() {
                match c.status {
                    ApprovalStatus::Approved => {
                        match self.tool_executor.execute(&c.tool_call, turn_context).await {
                            Ok(r) => {
                                c.result = Some(r.content);
                            }
                            Err(e) => {
                                warn!("Tool execution failed: {}", e);
                                c.result = Some(format!("Error: {}", e));
                            }
                        }
                    }
                    ApprovalStatus::Denied => {
                        c.result = Some("Tool execution denied".to_string());
                    }
                    ApprovalStatus::Timeout => {
                        c.result = Some("Tool execution timed out".to_string());
                    }
                    ApprovalStatus::Pending => {
                        complete = false;
                    }
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
                })
            }
            let tool_message = Message {
                id: MessageId(uuid::Uuid::new_v4()),
                prev_id: message_id,
                sender_id: ActorId::Agent(self.agent_id),
                sender_name: self.agent_controller.name(),
                role: MessageRole::Tool,
                tool_results,
                content: None,
                timestamp: chrono::Utc::now(),
                ..Message::default()
            };
            turn_context.push_message(tool_message.clone());
            Some(tool_message)
        } else {
            None
        }
    }

    /// Builds the tool-result message that reports a plan approval/rejection
    /// decision back to the model, using the same protocol as any other tool
    /// result. This - not a fresh system message - is how the decision (and,
    /// via the still-intact `create_plan` call earlier in context, the plan's
    /// own steps) stays visible to the model going forward.
    fn plan_decision_message(agent_id: AgentId, agent_name: &str, tool_call_id: Option<String>, content: &str) -> Message {
        Message {
            id: MessageId(uuid::Uuid::new_v4()),
            sender_id: ActorId::Agent(agent_id),
            sender_name: agent_name.to_string(),
            role: MessageRole::Tool,
            tool_results: vec![ToolCallResult {
                id: tool_call_id,
                tool_name: CREATE_PLAN_TOOL_NAME.to_string(),
                content: content.to_string(),
            }],
            timestamp: chrono::Utc::now(),
            ..Message::default()
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
            ..Message::default()
        }
    }

    /// Pushes the instruction for `next_step_index` and sets progress to
    /// `Step(next_step_index)`, or - if there is no such step - pushes the
    /// finalize instruction and sets progress to `Finalizing`. Handles the
    /// empty-plan edge case (next_step_index == 0 immediately finalizes).
    /// Called once at approval time (next_step_index == 0) and once each
    /// time a step converges in `handle_planning`.
    fn advance_plan(turn_context: &mut TurnContext, next_step_index: usize) {
        let Some(plan) = turn_context.plan().map(|p| p.plan.clone()) else { return };
        match plan.steps.get(next_step_index) {
            Some(step) => {
                turn_context.push_message(Self::plan_step_message(next_step_index + 1, plan.steps.len(), step));
                turn_context.set_plan_progress(PlanProgress::Step(next_step_index));
            }
            None => {
                turn_context.push_message(Self::plan_finalize_message(&plan));
                turn_context.set_plan_progress(PlanProgress::Finalizing);
            }
        }
    }

    async fn reflect<'a>(&self, turn_context: &mut TurnContext<'a>) {
        turn_context.extend_state_context(turn_context.messages().clone());
        turn_context.extend_state_history(turn_context.messages().clone());
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
        global_rx: &mut mpsc::Receiver<GlobalEvent>,
        output: &dyn EventSender,
    ) -> Result<Option<ActorState>> {
        tokio::select! {
            maybe_event = rx.recv() => {
                match maybe_event {
                    Some(event) => {
                        let next = match event.event {
                            ActorEvent::FullResponseGenerated(messages) => {
                                self.perceive(turn_context, messages, &event.context, &event.security).await;
                                Some(ActorState::Idle)
                            }
                            ActorEvent::ResponseRequested => {
                                self.request_response(turn_context, &event.context, &event.security, output).await?;
                                if self.enable_extended_planning {
                                    Some(ActorState::Deliberating)
                                } else {
                                    Some(ActorState::Planning)
                                }
                            }
                            _ => Some(ActorState::Idle),
                        };
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

    /// `Sleep` differs from `Idle` in that it deliberately does *not* react to
    /// `ResponseRequested` at all (only an external `wakeup()` call ends a sleep) -
    /// it only keeps local context current via `FullResponseGenerated`.
    async fn handle_sleep<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        rx: &mut Box<dyn EventReceiver>,
        global_rx: &mut mpsc::Receiver<GlobalEvent>,
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
                        Ok(Some(ActorState::Sleeping))
                    }
                    // Channel is closed
                    None => Ok(None)
                }
            }
            maybe_global = global_rx.recv() => {
                self.handle_global_event(maybe_global, output).await;
                Ok(Some(ActorState::Sleeping))
            }
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

        // Send final message
        if !turn_context.messages().is_empty() {
            turn_context.send_event(
                ActorEvent::FullResponseGenerated(turn_context.messages().clone()),
                output,
            ).await?;
        }
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
        global_rx: &mut mpsc::Receiver<GlobalEvent>,
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

    /// Handle a plan-approval decision while in WaitingForPlanApproval state.
    /// On approval, resumes via Planning (which will now execute the plan).
    /// On denial/timeout, folds the rejection into turn messages and returns
    /// to Deliberating so the model gets one bounded chance to self-correct
    /// (mirrors how tool-call ApprovalDenied is handled: recorded, fed back
    /// into context, the turn continues rather than dead-ending).
    async fn handle_waiting_for_plan_approval<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        rx: &mut Box<dyn EventReceiver>,
        global_rx: &mut mpsc::Receiver<GlobalEvent>,
        output: &dyn EventSender,
    ) -> Result<Option<ActorState>> {
        let agent_name = self.agent_controller.name();
        let is_plan_approval_event = |env: &Envelope| {
            matches!(
                env.event,
                ActorEvent::PlanApprovalGranted(_)
                    | ActorEvent::PlanApprovalDenied(_)
                    | ActorEvent::PlanApprovalTimedOut(_)
            )
        };

        tokio::select! {
            maybe_event = rx.recv_matching(is_plan_approval_event) => {
                match maybe_event {
                    Some(event) => {
                        let (req, status) = match &event.event {
                            ActorEvent::PlanApprovalGranted(r) => (r, ApprovalStatus::Approved),
                            ActorEvent::PlanApprovalDenied(r) => (r, ApprovalStatus::Denied),
                            ActorEvent::PlanApprovalTimedOut(r) => (r, ApprovalStatus::Timeout),
                            _ => unreachable!("handle_waiting_for_plan_approval called with non-plan-approval event"),
                        };

                        if !turn_context.set_plan_status(req.id, status) {
                            warn!("Received plan approval for unknown approval id {}", req.id);
                        }

                        rx.commit().await?;

                        // Report the decision back as a tool result for the
                        // create_plan call itself - the same protocol the model
                        // already understands from every other tool call, and
                        // the only place the plan's steps (its arguments) are
                        // still recoverable from context once we move on.
                        let tool_call_id = turn_context.plan().and_then(|p| p.tool_call_id.clone());
                        let next = if status == ApprovalStatus::Approved {
                            turn_context.push_message(Self::plan_decision_message(
                                self.agent_id,
                                &agent_name,
                                tool_call_id,
                                "Plan approved by the user. Proceed with execution.",
                            ));
                            Self::advance_plan(turn_context, 0);
                            ActorState::Planning
                        } else {
                            turn_context.take_plan();
                            turn_context.push_message(Self::plan_decision_message(
                                self.agent_id,
                                &agent_name,
                                tool_call_id,
                                "Plan rejected by the user. Do not execute it. Either answer \
                                 directly without it, ask a clarifying question, or propose a \
                                 revised plan.",
                            ));
                            ActorState::Deliberating
                        };
                        Ok(Some(next))
                    }
                    // Channel is closed
                    None => Ok(None),
                }
            }
            maybe_global = global_rx.recv() => {
                self.handle_global_event(maybe_global, output).await;
                Ok(Some(ActorState::WaitingForPlanApproval))
            }
        }
    }

    /// Analyzes the request, calling the LLM (and any real research tools it
    /// chooses) until it either proposes a plan via `create_plan` or answers
    /// directly (trivial requests, or after a rejected plan). Shares the
    /// same iteration budget as `handle_planning` via `loop_count`.
    async fn handle_deliberating<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        output: &dyn EventSender,
    ) -> Result<ActorState> {
        let agent_name = self.agent_controller.name();

        if self.loop_count.load(Ordering::SeqCst) >= self.max_iterations {
            info!("Too many iterations, stop deliberating agent {} {}", self.id, agent_name);
            return Ok(ActorState::Reflecting);
        }
        self.loop_count.fetch_add(1, Ordering::SeqCst);

        let mut response = self.deliberate(turn_context).await;
        use futures::stream::TryStreamExt;
        while let Some(event) = response
            .try_next()
            .await
            .context("Error during deliberation stream")?
        {
            if self.processing_state.load() == ActorState::Cancelling {
                info!("Stopping for cancelled agent {} {}", self.id, agent_name);
                return Ok(ActorState::Cancelling);
            }

            match event {
                MessageEvent::Partial(mut message) => {
                    message.sender_name = agent_name.clone();
                    message.sender_id = self.id;
                    if let Some(msg) = self.respond_partial(message, turn_context).await {
                        turn_context.send_event(ActorEvent::IntermediatePartialGenerated(msg), output).await?;
                    }
                }
                MessageEvent::Complete(mut message) => {
                    message.sender_name = agent_name.clone();
                    message.sender_id = self.id;

                    let plan_call = message
                        .tool_calls
                        .iter()
                        .find(|tc| tc.function_name == CREATE_PLAN_TOOL_NAME)
                        .cloned();

                    if let Some(plan_call) = plan_call {
                        if message.tool_calls.len() > 1 {
                            warn!(
                                "Agent {} called create_plan alongside other tool calls; discarding the others",
                                self.id
                            );
                        }
                        let plan = parse_plan(&plan_call)?;
                        // Keep the create_plan call itself in the pushed message
                        // (rather than stripping it) - its arguments ARE the
                        // plan, so leaving it in context is how the model still
                        // knows the plan's steps once execution begins.
                        message.tool_calls = vec![plan_call.clone()];

                        if let Some(msg) = self.respond_complete(message, turn_context).await {
                            turn_context.send_event(ActorEvent::IntermediateMessageGenerated(msg), output).await?;
                        }

                        let approval_id = ApprovalId::new();
                        turn_context.set_plan(PlanState {
                            id: approval_id,
                            status: ApprovalStatus::Pending,
                            tool_call_id: plan_call.id,
                            plan: plan.clone(),
                            progress: PlanProgress::Step(0),
                        });
                        let request = PlanApprovalRequest {
                            id: approval_id,
                            session_id: turn_context.session_id,
                            channel_id: turn_context.channel_id,
                            agent_id: self.agent_id,
                            plan,
                        };
                        turn_context.send_event(ActorEvent::PlanApprovalRequested(request), output).await?;
                        return Ok(ActorState::WaitingForPlanApproval);
                    }

                    // Real (research) tool calls - reuse the existing approval
                    // pipeline verbatim.
                    self.request_approvals(&message.tool_calls, turn_context, output).await?;
                    let has_tool_calls = !message.tool_calls.is_empty();

                    if has_tool_calls {
                        if let Some(msg) = self.respond_complete(message, turn_context).await {
                            turn_context.send_event(ActorEvent::IntermediateMessageGenerated(msg), output).await?;
                        }
                        if turn_context.has_tool_calls() {
                            return Ok(ActorState::ExecutingTools);
                        }
                    } else {
                        // No plan proposed, no tools called - a trivial
                        // request answered directly. This IS the real
                        // response, so it uses the unchanged final-answer event.
                        if let Some(msg) = self.respond_complete(message, turn_context).await {
                            turn_context.send_event(ActorEvent::CompleteMessageGenerated(msg), output).await?;
                        }
                    }
                }
                MessageEvent::EndOfTurn(_) => {}
            }
        }

        Ok(ActorState::Reflecting)
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
        let tool_calls: Vec<ToolCallState> = tool_calls.iter().map(|tc| {
            let tool = tools.iter().find(|t| t.name == tc.function_name);
            let approval = tool.map(|t| t.requires_approval).unwrap_or(false);
            let description = self.tool_executor.describe(tc).unwrap_or("No description".to_string());
            ToolCallState {
                id: ApprovalId::new(),
                status: if approval { ApprovalStatus::Pending } else { ApprovalStatus::Approved },
                tool_call: tc.clone(),
                description,
                result: None,
            }
        }).collect();

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

    /// Calls the LLM and streams its response for a single planning step. Loops
    /// back to itself (via the run loop, after a trip through ExecutingTools) as
    /// long as the model keeps making tool calls, up to `max_iterations`.
    async fn handle_planning<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        output: &dyn EventSender,
    ) -> Result<ActorState> {
        let agent_name = self.agent_controller.name();

        if self.loop_count.load(Ordering::SeqCst) >= self.max_iterations {
            info!("Too many iterations, stop planning agent {} {}", self.id, agent_name);
            return Ok(ActorState::Reflecting);
        }
        self.loop_count.fetch_add(1, Ordering::SeqCst);

        // If a plan is in play, it has already been approved by the time we
        // ever reach this state (see handle_waiting_for_plan_approval) - this
        // is now the execution loop for that plan, and its messages must be
        // distinguishable from a genuine final response until the model
        // stops calling tools.
        let executing_plan = turn_context.is_executing_approved_plan();

        // --- PHASE 2: PLAN ---
        let mut response = self.plan(turn_context).await;
        use futures::stream::TryStreamExt;
        while let Some(event) = response
            .try_next()
            .await
            .context("Error during planning stream")?
        {
            if self.processing_state.load() == ActorState::Cancelling {
                info!("Stopping for cancelled agent {} {}", self.id, agent_name);
                return Ok(ActorState::Cancelling);
            }

            match event {
                MessageEvent::Partial(mut message) => {
                    message.sender_name = agent_name.clone();
                    message.sender_id = self.id;
                    if let Some(msg) = self.respond_partial(message, turn_context).await {
                        let event = if executing_plan {
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

                    // Check for tool approvals
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
                                Self::advance_plan(turn_context, idx + 1);
                                return Ok(ActorState::Planning);
                            }
                            None => unreachable!("executing_plan implies a plan is present"),
                        }
                    } else {
                        if let Some(msg) = self.respond_complete(message, turn_context).await {
                            let event = if executing_plan {
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
    /// step have results (Planning's own top-of-function check will notice if a
    /// stop/bye/silence tool just ran), or parks in WaitingForApproval if some
    /// are still Pending.
    async fn handle_executing_tools<'a>(
        &self,
        turn_context: &mut TurnContext<'a>,
        output: &dyn EventSender,
    ) -> Result<ActorState> {
        if let Some(tool_message) = self.execute_tools(turn_context).await {
            turn_context.send_event(ActorEvent::CompleteMessageGenerated(tool_message), output).await?;

            // Handle "Silent" requested state
            if turn_context.requested_state() == Some(ActorState::Stopping) {
                // "Stopping" is a one-time state, so we remove it
                turn_context.take_requested_state();
                Ok(ActorState::Stopping)
            }
            else {
                // A plan not yet Approved means we're still researching ahead
                // of proposing/re-proposing it - go back to Deliberating.
                // No plan, or an already-Approved plan being executed, both
                // fall through to Planning (today's exact behavior when no
                // plan is in play).
                let still_deliberating = turn_context
                    .plan()
                    .map(|p| p.status != ApprovalStatus::Approved)
                    .unwrap_or(false);
                if still_deliberating {
                    Ok(ActorState::Deliberating)
                } else {
                    Ok(ActorState::Planning)
                }
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
            ActorEvent::FullResponseGenerated(turn_context.messages().clone()),
            output,
        ).await?;
        turn_context.finish_turn();
        info!("Finished generating messages for agent {} {}", self.id, agent_name);

        // Use requested state
        Ok(turn_context.take_requested_state().unwrap_or(ActorState::Idle))
    }
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
                    | ActorEvent::PlanApprovalGranted(_)
                    | ActorEvent::PlanApprovalDenied(_)
                    | ActorEvent::PlanApprovalTimedOut(_)
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
            let next_state = match current_state {
                ActorState::Idle => {
                    self.handle_idle(&mut turn_context, &mut rx, &mut global_rx, &*output).await?
                }
                ActorState::WaitingForApproval => {
                    self.handle_waiting_for_approval(&mut turn_context, &mut rx, &mut global_rx, &*output).await?
                }
                ActorState::WaitingForPlanApproval => {
                    self.handle_waiting_for_plan_approval(&mut turn_context, &mut rx, &mut global_rx, &*output).await?
                }
                ActorState::Sleeping => {
                    self.handle_sleep(&mut turn_context, &mut rx, &mut global_rx, &*output).await?
                }
                ActorState::Stopping => Some(self.handle_stopping()?),
                ActorState::Cancelling => Some(self.handle_cancelling(&mut turn_context, &*output).await?),
                ActorState::Deliberating => Some(self.handle_deliberating(&mut turn_context, &*output).await?),
                ActorState::Planning => Some(self.handle_planning(&mut turn_context, &*output).await?),
                ActorState::ExecutingTools => Some(self.handle_executing_tools(&mut turn_context, &*output).await?),
                ActorState::Reflecting => Some(self.handle_reflecting(&mut turn_context, &*output).await?),
                ActorState::Active => unreachable!("AgentActor never assigns itself ActorState::Active"),
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
        info!(
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

    async fn cancel(&self) -> Result<()> {
        info!(
            "Cancelling agent {} {}",
            self.id,
            self.agent_controller.name()
        );
        self.processing_state.transition(|s| match s {
            ActorState::WaitingForApproval | ActorState::Active | ActorState::ExecutingTools | ActorState::Planning
                | ActorState::Deliberating | ActorState::WaitingForPlanApproval => Some(ActorState::Cancelling),
            _ => None,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component_impl::context::{ContextProvider, StateContextAppender, StateContextProvider};
    use crate::component_impl::history::StateHistoryWriter;
    use crate::component_impl::message::MessageProcessor;
    use caelumora_model::message::MessageRole;
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

    #[test]
    fn parse_plan_extracts_summary_and_steps() {
        let call = ToolCall {
            id: Some("call_1".to_string()),
            function_name: CREATE_PLAN_TOOL_NAME.to_string(),
            arguments: serde_json::json!({
                "summary": "Investigate and fix the bug",
                "steps": [
                    { "description": "Reproduce the issue" },
                    { "description": "Find the root cause" },
                    { "description": "Apply and verify the fix" },
                ]
            }),
        };

        let plan = parse_plan(&call).expect("valid create_plan arguments should parse");
        assert_eq!(plan.summary, "Investigate and fix the bug");
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.steps[0].description, "Reproduce the issue");
        assert_eq!(plan.steps[2].description, "Apply and verify the fix");
    }

    #[test]
    fn parse_plan_rejects_malformed_arguments() {
        let call = ToolCall {
            id: Some("call_1".to_string()),
            function_name: CREATE_PLAN_TOOL_NAME.to_string(),
            arguments: serde_json::json!({ "summary": "missing steps field" }),
        };

        assert!(parse_plan(&call).is_err());
    }

    #[test]
    fn create_plan_tool_is_never_marked_as_requiring_approval() {
        // create_plan is intercepted directly in handle_deliberating and must
        // never reach the tool-call approval pipeline.
        assert!(!create_plan_tool().requires_approval);
        assert_eq!(create_plan_tool().name, CREATE_PLAN_TOOL_NAME);
    }

    #[test]
    fn plan_decision_message_reports_the_decision_against_the_original_tool_call_id() {
        let agent_id = AgentId::new();
        let message = AgentActor::plan_decision_message(
            agent_id,
            "Sarah",
            Some("call_42".to_string()),
            "Plan approved by the user. Proceed with execution.",
        );

        assert_eq!(message.role, MessageRole::Tool);
        assert_eq!(message.tool_results.len(), 1);
        assert_eq!(message.tool_results[0].id, Some("call_42".to_string()));
        assert_eq!(message.tool_results[0].tool_name, CREATE_PLAN_TOOL_NAME);
        assert_eq!(message.tool_results[0].content, "Plan approved by the user. Proceed with execution.");
    }

    fn make_plan_state(steps: Vec<&str>) -> PlanState {
        PlanState {
            id: ApprovalId::new(),
            status: ApprovalStatus::Approved,
            tool_call_id: Some("call_1".to_string()),
            progress: PlanProgress::Step(0),
            plan: Plan {
                id: PlanId::new(),
                summary: "do the thing".to_string(),
                steps: steps.into_iter().map(|d| PlanStep { description: d.to_string() }).collect(),
            },
        }
    }

    #[test]
    fn advance_plan_moves_to_the_next_step_when_one_remains() {
        let mut agent_state = AgentState::default();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);
        turn_context.set_plan(make_plan_state(vec!["first", "second"]));

        AgentActor::advance_plan(&mut turn_context, 1);

        assert_eq!(turn_context.plan().unwrap().progress, PlanProgress::Step(1));
        let last = turn_context.messages().last().expect("a step message should have been pushed");
        assert_eq!(last.role, MessageRole::System);
        assert!(last.content.as_ref().unwrap().contains("second"));
    }

    #[test]
    fn advance_plan_finalizes_once_steps_are_exhausted() {
        let mut agent_state = AgentState::default();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);
        turn_context.set_plan(make_plan_state(vec!["first", "second"]));

        AgentActor::advance_plan(&mut turn_context, 2);

        assert_eq!(turn_context.plan().unwrap().progress, PlanProgress::Finalizing);
        let last = turn_context.messages().last().expect("a finalize message should have been pushed");
        assert_eq!(last.role, MessageRole::System);
        assert!(last.content.as_ref().unwrap().contains("completed every step"));
    }

    #[test]
    fn advance_plan_finalizes_immediately_for_an_empty_plan() {
        let mut agent_state = AgentState::default();
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn_context = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);
        turn_context.set_plan(make_plan_state(vec![]));

        AgentActor::advance_plan(&mut turn_context, 0);

        assert_eq!(turn_context.plan().unwrap().progress, PlanProgress::Finalizing);
    }
}
