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
use caelumora_model::security::SecurityContext;
use caelumora_model::store::StateStore;
use caelumora_model::tool::{ToolCall, ToolCallState, ToolResult};
use futures::stream::BoxStream;
use log::{debug, info, warn};
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
            actor_runtime.component_registry().as_ref(),
        )?;
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
            max_iterations: 10,
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
    ) -> BoxStream<'static, Result<MessageEvent>> {
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
                tool_results.push(ToolResult {
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
                                Some(ActorState::Planning)
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
                        turn_context.send_event(ActorEvent::PartialMessageGenerated(msg), output).await?;
                    }
                }
                MessageEvent::Complete(mut message) => {
                    message.sender_name = agent_name.clone();
                    message.sender_id = self.id;

                    // Check for tool approvals
                    self.request_approvals(&message.tool_calls, turn_context, output).await?;

                    if let Some(msg) = self.respond_complete(message, turn_context).await {
                        turn_context.send_event(ActorEvent::CompleteMessageGenerated(msg), output).await?;
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
                ActorState::Sleeping => {
                    self.handle_sleep(&mut turn_context, &mut rx, &mut global_rx, &*output).await?
                }
                ActorState::Stopping => Some(self.handle_stopping()?),
                ActorState::Cancelling => Some(self.handle_cancelling(&mut turn_context, &*output).await?),
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
            ActorState::WaitingForApproval | ActorState::Active | ActorState::ExecutingTools | ActorState::Planning => Some(ActorState::Cancelling),
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
}
