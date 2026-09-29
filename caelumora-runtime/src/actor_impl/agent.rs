use crate::actor::{Actor, MessageContextError};
use crate::agent_turn::{CommittedTurn, Turn};
use crate::component::{ComponentContainer, ComponentContext, ComponentImpl};
use crate::context::ContextProvider;
use crate::message::{EmitOutProcessor, MessageProcessor, PerceiveInProcessor};
use crate::runtime::ActorRuntime;
use crate::tool::{ToolError, ToolExecutor, ToolProvider};
use crate::transport::{EventReceiver, EventSender};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext, ProcessingStateHolder};
use crate::agent_state::AgentStateHandle;
use caelumora_model::agent::{AgentChannelState, AgentId, SeenContext};
use crate::controller::AgentController;
use caelumora_model::approval::{ApprovalId, ApprovalStatus};
use caelumora_model::channel::{Channel, ChannelId};
use caelumora_model::component::ComponentFailPolicy;
use caelumora_model::confidentiality::{ChannelClassification, Confidentiality, Publication};
use caelumora_model::message::{Message, MessageEvent, MessageId};
use caelumora_model::plan::PlanProgress;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, SecurityContext};
use caelumora_model::tool::{Tool, ToolCall, ToolCallResult, ToolCallState, ToolResultStatus};
use futures::stream::BoxStream;
use tracing::{debug, info, warn, Instrument, Span};
use std::mem;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub struct AgentActor {
    id: ActorId,
    kind: String,
    agent_id: AgentId,
    channel_id: ChannelId,
    agent_controller: Arc<AgentController>,
    actor_runtime: ActorRuntime,

    // Components
    context_provider: Vec<ComponentImpl<dyn ContextProvider>>,
    perceive_in_processors: Vec<ComponentImpl<dyn PerceiveInProcessor>>,
    emit_out_processors: Vec<ComponentImpl<dyn EmitOutProcessor>>,
    tool_executor: Box<dyn ToolExecutor>,
    /// The least confidential label its reasoning gets (issue 61), from the
    /// personality.
    reasoning_confidentiality: Confidentiality,

    // State
    state: Mutex<AgentActorState>,
    processing_state: Arc<ProcessingStateHolder>,
    /// Cancelled by `cancel` while a turn runs, replaced when the next one
    /// begins: the streaming loop waits on it next to the model's stream, so
    /// that a cancel drops the request instead of waiting for its next
    /// chunk. A `std` mutex, because `cancel` is synchronous.
    turn_cancel: std::sync::Mutex<CancellationToken>,
    /// Set by `stop`: the actor left the channel, and its run loop ends at
    /// the next step boundary, abandoning a turn under way.
    stopping: AtomicBool,
    loop_count: AtomicI32,
    max_iterations: i32,
}

#[derive(Debug)]
struct AgentActorState {
    agent_id: AgentId,
    agent_channel_state: AgentChannelState,
    /// The agent's own state, shared with its actors in other channels and
    /// sessions (issue 62).
    agent_state: Arc<AgentStateHandle>,
}

impl AgentActorState {
    /// The handle to the agent's state for one step of the run loop, for
    /// the agent named `agent_name` - as its controller has it now.
    fn turn<'a>(&'a mut self, agent_name: String) -> Turn<'a> {
        Turn::new(self.agent_id, agent_name, &mut self.agent_channel_state, self.agent_state.clone())
    }
}

impl AgentActor {
    /// The actor of the agent `agent_controller` holds - the session's
    /// controller of it, shared with its actors in the session's other
    /// channels (issue 120).
    pub async fn new(
        channel: &Channel,
        agent_controller: Arc<AgentController>,
        actor_runtime: ActorRuntime,
    ) -> Result<Self> {
        let agent = agent_controller.load();
        // The agent acts on behalf of its owner, in this channel (issue 87).
        let security_context = SecurityContext::new(agent.security_attributes.owner_id, channel.session_id, channel.id)
            .next(ActorId::Agent(agent.id));
        let personality = actor_runtime.personality(&security_context, agent.personality_id).await?;
        let agent_id = agent.id;
        let attributes = agent.security_attributes.clone();
        drop(agent);

        let components = ComponentContainer::from_config(
            &personality.components,
            &actor_runtime.component_registry(),
            // Its components get the agent's runtime, and read the session's
            // other channels through it (issues 17, 69).
            &ComponentContext::new(security_context.clone(), actor_runtime.secret_manager(), Some(actor_runtime.clone())),
        )
        .await?;
        let context_provider = components.get_all::<dyn ContextProvider>(None);
        let perceive_in_processors = components.get_all::<dyn PerceiveInProcessor>(None);
        let emit_out_processors = components.get_all::<dyn EmitOutProcessor>(None);
        let tool_providers = components.get_all::<dyn ToolProvider>(None);
        let tool_executor = actor_runtime.new_tool_executor(tool_providers)?;

        let mut agent_channel_state = actor_runtime.load_channel_state().await?;
        let not_executed = Self::prepare_resume(&mut agent_channel_state);
        if not_executed > 0 {
            warn!("Agent {} in channel {}: {} approved tool calls were not executed before the restart; the model is told", agent_id, channel.id, not_executed);
        }
        let agent_state = actor_runtime.agent_state(attributes).await?;
        let processing_state = Arc::new(ProcessingStateHolder::new(agent_channel_state.processing_state));
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
            context_provider,
            perceive_in_processors,
            emit_out_processors,
            tool_executor,
            reasoning_confidentiality: personality.reasoning_confidentiality,
            state,
            processing_state,
            turn_cancel: std::sync::Mutex::new(CancellationToken::new()),
            stopping: AtomicBool::new(false),
            loop_count: AtomicI32::new(0),
            max_iterations: 25,
        })
    }

    /// Takes in what others said: the perceive processors append it to the
    /// transcript and the history. No turn: nothing here is a turn's work,
    /// and nothing is asked of the policy (issues 85, 90).
    async fn perceive<'a>(&self, turn: &mut Turn<'a>, messages: Vec<Message>) {
        // The processors act on the agent's state (transcript, history);
        // what they return is not used.
        if let Err(e) = Self::apply_stage_processors(&self.perceive_in_processors, messages, turn).await {
            warn!("PerceiveIn stage failed: {}", e);
        }
    }

    /// A complete message as it leaves the agent - through the emit
    /// processors - and becomes part of the turn. `None` if the processors
    /// dropped it.
    async fn respond_complete<'a>(
        &self,
        message: Message,
        turn: &mut Turn<'a>,
    ) -> Option<Message> {
        let msg = self.respond_partial(message, turn).await?;
        turn.push_message(msg.clone());
        Some(msg)
    }

    /// A message as it leaves the agent, through the emit processors; a
    /// partial one is only shown, not kept. `None` if the processors
    /// dropped it.
    async fn respond_partial<'a>(
        &self,
        message: Message,
        turn: &mut Turn<'a>,
    ) -> Option<Message> {
        let mut output =
            Self::apply_stage_processors(&self.emit_out_processors, vec![message], turn)
                .await
                .ok()?;
        if output.is_empty() {
            return None;
        }
        Some(output.remove(0))
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

    async fn next_turn<'a>(&self, turn: &mut Turn<'a>) -> Result<()> {
        for context_provider in &self.context_provider {
            match context_provider.facet.current_context(turn).await {
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

    /// Starts a new turn in answer to a `ResponseRequested`: begins it,
    /// announces `ResponseStarted` and builds the context. The run loop then
    /// goes on in `Planning`.
    async fn request_response<'a>(
        &self,
        turn: &mut Turn<'a>,
        event_context: &EventContext,
        event_security: &SecurityContext,
        output: &dyn EventSender,
    ) -> Result<()> {
        self.loop_count.store(0, Ordering::SeqCst);
        self.begin_turn(turn, event_context, event_security).await?;

        debug!("Starting generating messages for agent {} {}", self.id, turn.agent_name);
        turn.send_event(ActorEvent::ResponseStarted, output).await?;

        // Initialize next turn
        self.next_turn(turn).await?;

        Ok(())
    }

    /// Calls the LLM for one turn step. `create_plan_tool` (when wired in)
    /// contributes its own planning nudge into `turn.context` as a
    /// `PromptProvider`, composed by `StateContextProvider` once per
    /// turn - nothing plan-specific needs handling here.
    async fn call_llm<'a>(
        &self,
        turn: &mut Turn<'a>,
    ) -> BoxStream<'static, crate::llm::LLMResult<MessageEvent>> {
        let llm_client = self.actor_runtime.llm_client();
        let context = turn.full_context();
        let tools = self.tool_executor.tools();
        llm_client
            .chat_completion(context.as_slice(), tools.as_slice())
            .await
    }

    /// Labels the parts of `message` (issues 54, 61). Every tool call gets
    /// the labels its tool declares now, so that the call and its result are
    /// published by what applied at the time of the call; a call of an
    /// unknown tool is public: it fails, and its failure hides nothing.
    ///
    /// The reasoning gets the most confidential of: the personality's
    /// default; `Private` once the turn saw the agent's private knowledge;
    /// and the content of the calls it leads to - those of the message once
    /// it is `complete`, and while it streams those of every tool the agent
    /// could still call, since reasoning comes before the call it leads to.
    /// Knowledge read from other channels adds nothing: a channel reads only
    /// what reaches no further than its own contents (rule 2), so the
    /// channel admits it anyway. `tools` are the agent's tools, fetched once
    /// per model call rather than per streamed chunk.
    fn label(&self, message: &mut Message, turn: &Turn, tools: &[Tool], complete: bool) {
        let declared = |name: &str| tools.iter().find(|t| t.name == name).map(|t| t.confidentiality).unwrap_or_default();
        for call in &mut message.tool_calls {
            call.confidentiality = declared(&call.function_name);
        }

        let mut label = self.reasoning_confidentiality;
        if turn.saw_private_knowledge() {
            label = Confidentiality::Private;
        }
        let calls = if complete {
            message.tool_calls.iter().map(|c| c.confidentiality.content).max()
        } else {
            tools.iter().map(|t| t.confidentiality.content).max()
        };
        message.reasoning_confidentiality = label.max(calls.unwrap_or(Confidentiality::Public));
    }

    /// Stores the agent's state in its channel as `turn` holds it.
    /// A failure is logged: the actor carries on with the state in memory.
    /// Stores the agent's state in the channel, with where the actor stands
    /// now.
    async fn store_channel_state(&self, turn: &mut Turn<'_>) {
        turn.set_processing_state(self.processing_state.load());
        if let Err(e) = self.actor_runtime.store_channel_state(turn.agent_channel_state()).await {
            warn!("Cannot store agent channel state of agent {}: {:#}", self.agent_id, e);
        }
    }

    /// Makes a loaded state fit to run from (issue 75). A state stored
    /// between two steps of a turn is a turn to resume - the server
    /// stopped or crashed in the middle of it - stored with the step that
    /// was to come next (issue 85), and most states resume as they are:
    /// `Planning` asks the model with the turn so far, `Reflecting` takes
    /// the finished turn over, `WaitingForApproval` keeps waiting,
    /// `Cancelling` ends the turn. A deliberate end never leaves a turn in the
    /// store (`stop`, `cancel`, the stop tool).
    ///
    /// `ExecutingTools` is the exception: a tool may have run without its
    /// result being stored, and running it again would repeat its effect
    /// (issue 20). Every approved call without a result is answered as not
    /// executed, so that the next step builds the tool message and the
    /// model decides again; results already stored stay. Returns how many
    /// calls were answered so.
    ///
    /// A state in the middle of a turn without the turn's working state
    /// cannot be resumed; it starts idle.
    fn prepare_resume(state: &mut AgentChannelState) -> usize {
        if Self::is_turn_state(state.processing_state) && state.turn.is_none() {
            state.processing_state = ActorState::Idle;
        }
        let Some(turn) = state.turn.as_mut().filter(|_| state.processing_state == ActorState::ExecutingTools) else {
            return 0;
        };
        let mut answered = 0;
        for call in &mut turn.tool_calls {
            if call.status == ApprovalStatus::Approved && call.result.is_none() {
                call.result = Some(ToolCallResult::of(&call.tool_call, ToolResultStatus::Failed, crate::tool::not_executed_result()));
                answered += 1;
            }
        }
        answered
    }

    /// Begins a turn in answer to the envelope with `event_context` and
    /// `security_context`: with the channel as it is now and what the
    /// `SecurityManager` lets the agent publish there. Fails without a
    /// channel, which happens only while it is being torn down.
    async fn begin_turn<'a>(&self, turn: &mut Turn<'a>, event_context: &EventContext, security_context: &SecurityContext) -> Result<()> {
        let classification = self.actor_runtime.classification()
            .ok_or_else(|| anyhow!("Agent {} has no channel to take a turn in", self.id))?;
        let publication = self.publication(&classification).await;
        turn.next_turn(
            event_context.next(self.id, self.agent_controller.name()),
            security_context.next(self.id),
            classification,
            publication,
        );
        *self.turn_cancel.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = CancellationToken::new();
        Ok(())
    }

    /// The token of the current turn, cancelled by `cancel`.
    fn turn_token(&self) -> CancellationToken {
        self.turn_cancel.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// What the agent may publish to its channel in the turn beginning now
    /// (issue 61): the `SecurityManager` is asked for each level a channel
    /// can have, with the agent's channel and in its owner's name.
    async fn publication(&self, classification: &ChannelClassification) -> Publication {
        let owner = self.agent_controller.load().security_attributes.owner_id;
        let context = SecurityContext::new(owner, classification.session_id, classification.channel_id);
        let security = self.actor_runtime.security_manager();
        let allows = |label| {
            let (security, context) = (security.clone(), context.clone());
            async move { security.may_publish(&context, label, self.id, classification).await }
        };
        Publication::new(allows(Confidentiality::Session).await, allows(Confidentiality::Channel).await)
    }

    /// Runs the approved calls of the step that have no result yet - the
    /// executor is the actor's - and, once every call has its result, returns
    /// the tool message the turn keeps for the model (issue 83).
    async fn execute_tools<'a>(
        &self,
        turn: &mut Turn<'a>,
    ) -> Option<Message> {
        // Captured before running the calls: a `create_plan` call among them
        // sets the plan as a side effect of executing it, and the result
        // must be classified against the state that produced the call (same
        // as the assistant message that triggered it), not the state that
        // call itself just brought about.
        let had_plan_before = turn.has_plan();
        for (id, call) in turn.tool_calls().to_run() {
            let result = self.tool_executor.execute(&call, turn).await;
            turn.tool_calls().record(id, result);
        }
        // A tool result is only ever "final" (shown as the real answer) when
        // it's not part of executing an in-progress plan - mirrors the same
        // internal/final rule applied to the assistant message that
        // triggered it.
        turn.tool_calls().collect(had_plan_before)
    }

    /// `context` (this agent's own future-turn LLM input) keeps every message
    /// accumulated this turn, including internal deliberation/plan-execution
    /// scaffolding - it needs the full detail to stay coherent. `history`
    /// (the audit/user-facing trail - see `ChannelLoop::append_history` and
    /// `channel.history()`) only keeps genuinely final messages.
    ///
    /// The turn is taken over and stored before its `FullResponseGenerated`
    /// announces its end (issue 65): whoever reacts to it - another agent
    /// whose turn it starts, a user interface reloading notes - reads the
    /// state as the turn left it. A change of the agent's state in the
    /// channel is announced as `GlobalEvent::AgentChannelStateChanged` once
    /// stored; a change of its own state by its `AgentStateHandle`.
    async fn reflect<'a>(&self, turn: &mut Turn<'a>, output: &dyn EventSender) -> Result<()> {
        let CommittedTurn { final_messages, channel_state_changed, ended } = turn.commit_turn().await;
        self.store_channel_state(turn).await;
        if channel_state_changed {
            self.actor_runtime.announce_channel_state_changed();
        }
        ended.send_event(ActorEvent::FullResponseGenerated(final_messages), output).await?;
        Ok(())
    }

    async fn apply_stage_processors<'a, T>(
        processors: &[ComponentImpl<T>],
        mut input: Vec<Message>,
        context: &mut Turn<'a>,
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

    /// Idle and Sleeping wait for the next event alike: a
    /// `FullResponseGenerated` is perceived, a `ResponseRequested` starts a
    /// turn - unless `asleep`, where only `wakeup()` ends the sleep. `None`
    /// once the actor's input is closed.
    async fn handle_waiting<'a>(
        &self,
        asleep: bool,
        turn: &mut Turn<'a>,
        rx: &mut Box<dyn EventReceiver>,
        output: &dyn EventSender,
    ) -> Result<Option<ActorState>> {
        let Some(event) = rx.recv().await else { return Ok(None) };
        let span = crate::logging::envelope_span(&event.context, event.event.kind());
        let started = async {
            debug!("Received {}", event.describe());
            Ok::<_, anyhow::Error>(match event.event {
                ActorEvent::FullResponseGenerated(messages) => {
                    self.perceive(turn, messages).await;
                    false
                }
                ActorEvent::ResponseRequested if !asleep => {
                    match self.request_response(turn, &event.context, &event.security, output).await {
                        Ok(()) => true,
                        // E.g. a fail-closed context provider: the turn never starts.
                        Err(e) => {
                            self.fail_turn(turn, &e, output).await;
                            false
                        }
                    }
                }
                _ => false,
            })
        }.instrument(span).await?;
        rx.commit().await?;
        Ok(Some(if started { ActorState::Planning } else { self.processing_state.load() }))
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
    async fn fail_turn<'a>(&self, turn: &mut Turn<'a>, error: &anyhow::Error, output: &dyn EventSender) {
        let agent_name = self.agent_controller.name();
        tracing::error!("Turn of agent {} {} failed: {:#}", self.id, agent_name, error);

        let ended = turn.abandon_turn();

        let message = crate::error::user_message(error)
            .unwrap_or_else(|| "internal error - the details are in the server log".to_string());
        if let Err(e) = ended.send_event(ActorEvent::TurnFailed { message }, output).await {
            warn!("Agent {} {} cannot report its failed turn: {:#}", self.id, agent_name, e);
        }
    }

    /// The span one iteration of the run loop executes in. Waiting states get
    /// an `agent` span; the states of a turn get a `turn` span carrying the
    /// ids of the envelope that started the turn, so the turn can be found by
    /// the same `span_id` as the REST request and the channel dispatch.
    fn state_span(&self, state: ActorState, turn: &Turn<'_>) -> Span {
        let agent = self.agent_controller.name();
        match state {
            ActorState::Idle | ActorState::Sleeping => {
                tracing::info_span!("agent", agent_id = %self.agent_id, agent = %agent, state = ?state)
            }
            _ => {
                let ctx = turn.event_context();
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
                    span_depth = ctx.span_depth,
                )
            }
        }
    }

    async fn handle_cancelling<'a>(
        &self,
        turn: &mut Turn<'a>,
        output: &dyn EventSender,
    ) -> Result<ActorState> {
        // Always signals the end of the turn, even with no final messages -
        // a plan cancelled before any step produced a visible answer still
        // has to tell observers (e.g. the web UI) that generation stopped,
        // the same way handle_reflecting does unconditionally.
        self.reflect(turn, output).await?;

        info!("Agent {} {} finished cancelling", self.id, self.agent_controller.name());

        Ok(ActorState::Idle)
    }

    /// Handle a single approval event while in WaitingForApproval state.
    /// Records the decision; the run loop dispatches the actual tool execution
    /// on its next iteration via ExecutingTools.
    async fn handle_waiting_for_approval<'a>(
        &self,
        turn: &mut Turn<'a>,
        rx: &mut Box<dyn EventReceiver>,
    ) -> Result<Option<ActorState>> {
        let is_approval_event = |env: &Envelope| {
            matches!(
                env.event,
                ActorEvent::ApprovalGranted(_)
                    | ActorEvent::ApprovalDenied(_)
                    | ActorEvent::ApprovalTimedOut(_)
            )
        };

        match rx.recv_matching(is_approval_event).await {
            Some(event) => {
                let (req, status) = match &event.event {
                    ActorEvent::ApprovalGranted(r) => (r, ApprovalStatus::Approved),
                    ActorEvent::ApprovalDenied(r) => (r, ApprovalStatus::Denied),
                    ActorEvent::ApprovalTimedOut(r) => (r, ApprovalStatus::Timeout),
                    _ => unreachable!("handle_approval_event called with non-approval event"),
                };

                if !turn.tool_calls().decide(req.id, status) {
                    warn!("Received approval for unknown approval id {}", req.id);
                }

                rx.commit().await?;
                Ok(Some(ActorState::ExecutingTools))
            }
            // Channel is closed
            None => Ok(None),
        }
    }

    /// Registers tool calls for approval and requests approval for the ones that
    /// need it. Purely records data on `turn` - it never touches
    /// `processing_state`; `handle_planning`'s return value (always
    /// `ExecutingTools` when there are tool calls) and `handle_executing_tools`
    /// (which decides `Planning` vs `WaitingForApproval` once it knows which
    /// calls actually still need a decision) are the sole authority on state.
    async fn request_approvals<'a>(
        &self,
        tool_calls: &Vec<ToolCall>,
        turn: &mut Turn<'a>,
        output: &dyn EventSender,
    ) -> Result<()> {
        let tools = self.tool_executor.tools();
        let mut states: Vec<ToolCallState> = Vec::with_capacity(tool_calls.len());
        for tc in tool_calls {
            let tool = tools.iter().find(|t| t.name == tc.function_name);
            let approval = tool.map(|t| t.requires_approval).unwrap_or(false);
            let mut state = ToolCallState {
                id: ApprovalId::new(),
                status: if approval { ApprovalStatus::Pending } else { ApprovalStatus::Approved },
                tool_call: tc.clone(),
                description: String::new(),
                result: None,
            };
            // A call that cannot succeed is answered right away rather than
            // put to a user who could only approve it in vain - in the order
            // `execute` would find out: first the policy, then the call
            // itself (issue 118). The status then no longer matters: a call
            // with a result is not executed.
            let answer = if !self.tool_executor.is_permitted(tc, turn).await {
                info!("Tool call '{}' by agent {} forbidden by the security manager", tc.function_name, self.id);
                Some((ToolResultStatus::Forbidden, crate::tool::forbidden_result()))
            } else {
                match self.tool_executor.describe(tc) {
                    Ok(description) => {
                        state.description = description;
                        None
                    }
                    Err(e @ (ToolError::InvalidParameter(_) | ToolError::UnknownTool(_))) => {
                        info!("Tool call '{}' by agent {} cannot run: {}", tc.function_name, self.id, e);
                        let available: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
                        Some(e.to_result(&available))
                    }
                    Err(e) => {
                        warn!("Cannot describe tool call '{}' for approval: {}", tc.function_name, e);
                        state.description = "No description".to_string();
                        None
                    }
                }
            };
            if let Some((status, content)) = answer {
                state.status = ApprovalStatus::Approved;
                state.result = Some(ToolCallResult::of(tc, status, content));
            }
            states.push(state);
        }
        if states.is_empty() {
            return Ok(());
        }
        for request in turn.tool_calls().register(states) {
            turn.send_event(ActorEvent::ApprovalRequested(request), output).await?;
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
        turn: &mut Turn<'a>,
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
        let executing_plan = turn.has_plan();
        // The dedicated synthesis call (after the last step converged) is
        // still `executing_plan`, but its streamed text *is* the final
        // answer and must reach the user live, not be swallowed as internal
        // plan scaffolding.
        let finalizing_plan = matches!(
            turn.plan().map(|p| p.progress),
            Some(PlanProgress::Finalizing)
        );
        let internal_message = executing_plan && !finalizing_plan;

        let cancelled = self.turn_token();
        let mut response = self.call_llm(turn).await;
        let tools = self.tool_executor.tools();
        use futures::stream::TryStreamExt;
        loop {
            // A cancel wins over the next chunk, and returning drops the
            // stream - and with it the request to the model.
            let next = tokio::select! {
                biased;
                _ = cancelled.cancelled() => {
                    info!("Stopping for cancelled agent {} {}", self.id, agent_name);
                    return Ok(ActorState::Cancelling);
                }
                next = response.try_next() => next.with_context(|| "Error during LLM stream".to_string())?,
            };
            let Some(event) = next else { break };

            match event {
                MessageEvent::Partial(mut message) => {
                    message.sender_name = agent_name.clone();
                    message.sender_id = self.id;
                    self.label(&mut message, turn, &tools, false);
                    message.internal = internal_message;
                    if let Some(msg) = self.respond_partial(message, turn).await {
                        turn.send_event(Self::event_for(msg, true), output).await?;
                    }
                }
                MessageEvent::Complete(mut message) => {
                    message.sender_name = agent_name.clone();
                    message.sender_id = self.id;
                    self.label(&mut message, turn, &tools, true);
                    message.internal = internal_message;

                    self.request_approvals(&message.tool_calls, turn, output).await?;
                    let has_tool_calls = !message.tool_calls.is_empty();

                    if executing_plan && !has_tool_calls {
                        match turn.plan().map(|p| p.progress) {
                            Some(PlanProgress::Finalizing) => {
                                // The dedicated synthesis call converged -
                                // this is the real, final response.
                                turn.take_plan();
                                if let Some(msg) = self.respond_complete(message, turn).await {
                                    turn.send_event(ActorEvent::CompleteMessageGenerated(msg), output).await?;
                                }
                            }
                            Some(PlanProgress::Step(idx)) => {
                                // This step converged - not final. Its result
                                // is already part of context (respond_complete
                                // pushes it); move on to the next step, or to
                                // finalization if this was the last one.
                                if let Some(msg) = self.respond_complete(message, turn).await {
                                    turn.send_event(ActorEvent::IntermediateMessageGenerated(msg), output).await?;
                                }
                                let finished_step = turn.plan()
                                    .map(|p| (p.plan.steps[idx].description.clone(), p.plan.steps.len()));
                                if let Some((description, total_steps)) = finished_step {
                                    turn.send_event(
                                        ActorEvent::PlanStepFinished { step_index: idx, total_steps, description },
                                        output,
                                    ).await?;
                                }
                                if let Some(started) = turn.advance_plan(idx + 1) {
                                    turn.send_event(started, output).await?;
                                }
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
                        if let Some(msg) = self.respond_complete(message, turn).await {
                            turn.send_event(Self::event_for(msg, false), output).await?;
                        }
                    }

                    if !turn.tool_calls().is_empty() {
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
        turn: &mut Turn<'a>,
        output: &dyn EventSender,
    ) -> Result<ActorState> {
        let progress_before = turn.plan().map(|p| p.progress);
        if let Some(tool_message) = self.execute_tools(turn).await {
            turn.send_event(Self::event_for(tool_message, false), output).await?;

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
            let progress_after = turn.plan().map(|p| p.progress);
            if progress_after != progress_before {
                if let Some(PlanProgress::Step(idx)) = progress_after {
                    if let Some(started) = turn.advance_plan(idx) {
                        turn.send_event(started, output).await?;
                    }
                }
            }

            // A tool asked for the turn to end (the stop tool): it ends
            // with this tool step, without asking the model again; the
            // state it asked for is taken up when reflecting.
            if turn.requested_state().is_some() {
                Ok(ActorState::Reflecting)
            } else {
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
        turn: &mut Turn<'a>,
        output: &dyn EventSender,
    ) -> Result<ActorState> {
        let agent_name = self.agent_controller.name();
        // Read before the turn is taken over, which ends it.
        let requested = turn.requested_state();
        self.reflect(turn, output).await?;
        debug!("Finished generating messages for agent {} {}", self.id, agent_name);
        Ok(requested.unwrap_or(ActorState::Idle))
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
        if Self::is_turn_state(self.processing_state.load()) {
            info!("AgentActor {} resumes a turn in state {:?}", self.id(), self.processing_state.load());
        }

        loop {
            let current_state = self.processing_state.load();
            let mut turn = state.turn(self.agent_controller.name());

            // Idle/WaitingForApproval/Sleep may consume no event before the actor's
            // input channel closes (Ok(None)) - that's the only way this loop ends.
            // Every other state always produces a next state.
            let span = self.state_span(current_state, &turn);
            let result = async { Ok::<_, anyhow::Error>(match current_state {
                ActorState::Idle => {
                    self.handle_waiting(false, &mut turn, &mut rx, &*output).await?
                }
                ActorState::WaitingForApproval => {
                    self.handle_waiting_for_approval(&mut turn, &mut rx).await?
                }
                ActorState::Sleeping => {
                    self.handle_waiting(true, &mut turn, &mut rx, &*output).await?
                }
                ActorState::Cancelling => Some(self.handle_cancelling(&mut turn, &*output).await?),
                ActorState::Planning => Some(self.handle_llm_turn(&mut turn, &*output).await?),
                ActorState::ExecutingTools => Some(self.handle_executing_tools(&mut turn, &*output).await?),
                ActorState::Reflecting => Some(self.handle_reflecting(&mut turn, &*output).await?),
            }) }.instrument(span.clone()).await;

            // A failure inside a turn ends the turn, not the agent (issue 02).
            // Errors in the waiting states are the transport's and still end
            // the actor.
            let next_state = match result {
                Ok(next) => next,
                Err(e) if Self::is_turn_state(current_state) => {
                    self.fail_turn(&mut turn, &e, &*output).instrument(span).await;
                    Some(ActorState::Idle)
                }
                Err(e) => return Err(e),
            };

            // Told to stop, the actor is leaving the channel: a turn under
            // way is not resumed later, so it is abandoned rather than
            // stored (issue 75).
            if self.stopping.load(Ordering::SeqCst) {
                turn.abandon_turn();
                self.processing_state.store(ActorState::Idle);
                self.store_channel_state(&mut turn).await;
                break;
            }

            // The state goes on first, then it is stored: what a crash leaves
            // in the store is the step that comes next, not the one that is
            // done - resuming it neither repeats a model call whose answer is
            // already in the turn nor asks the model before a tool result is
            // in (issue 85).
            if let Some(next) = next_state {
                // If state has been switched to "Cancelling", but "current_state" was not
                // "Cancelling", then ignore "next_state"
                if current_state == ActorState::Cancelling || self.processing_state.load() != ActorState::Cancelling {
                    self.processing_state.store(next)
                }
            }
            self.store_channel_state(&mut turn).await;
            // Channel is closed, actor wants to quit
            if next_state.is_none() {
                break;
            }
        }

        info!("AgentActor {} has finished", self.id());
        Ok(())
    }

    fn state(&self) -> ActorState {
        self.processing_state.load()
    }

    fn wakeup(&self) {
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
    }

    async fn message_context(&self, caller: &SecurityContext, message: MessageId) -> Result<SeenContext, MessageContextError> {
        // Read from the store, where the state is written after every step:
        // the running loop holds the in-memory state for as long as it runs.
        let attributes = self.agent_controller.load().security_attributes.clone();
        self.actor_runtime.security_manager()
            .require(caller, ActionType::ReadAgentContext(&attributes), || MessageContextError::Forbidden { actor: self.id }).await?;
        let state = self.actor_runtime.load_channel_state().await
            .map_err(|e| {
                warn!("Cannot read the state of agent {} in channel {}: {}", self.agent_id, self.channel_id, e);
                MessageContextError::Failed { actor: self.id, message }
            })?;
        context_of(&state, self.id, message)
    }

    fn cancel(&self) {
        info!(
            "Cancelling agent {} {}",
            self.id,
            self.agent_controller.name()
        );
        // The state first, so that the run loop keeps `Cancelling` whatever
        // the interrupted step returns; then the token, which interrupts a
        // model call in flight. Outside a turn there is nothing to cancel.
        let state = self.processing_state.transition(|s| match s {
            ActorState::WaitingForApproval | ActorState::ExecutingTools | ActorState::Planning
                => Some(ActorState::Cancelling),
            _ => None,
        });
        if state == ActorState::Cancelling {
            self.turn_token().cancel();
        }
    }

    fn stop(&self) {
        info!("Stopping agent {} {}: it left the channel", self.id, self.agent_controller.name());
        self.stopping.store(true, Ordering::SeqCst);
        // Interrupts a model call in flight; the run loop then sees the
        // flag and abandons the turn instead of reflecting on it.
        self.turn_token().cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{ContextProvider, PromptPosition};
    use crate::component_impl::transcript_appender::TranscriptAppender;
    use crate::component_impl::state_context_provider::StateContextProvider;
    use crate::component_impl::state_history_appender::StateHistoryAppender;
    use crate::component_impl::static_system_prompt_provider::StaticPromptProvider;
    use crate::message::MessageProcessor;
    use caelumora_model::message::MessageRole;
    use chrono::Utc;
    use uuid::Uuid;

    #[test]
    fn a_resumed_turn_answers_unfinished_tool_calls_as_not_executed_instead_of_running_them() {
        use caelumora_model::approval::ApprovalId;
        use caelumora_model::tool::ToolConfidentiality;
        let call = |status: ApprovalStatus, result: Option<&str>| {
            let tool_call = ToolCall { id: None, function_name: "note".to_string(), arguments: serde_json::json!({}), confidentiality: ToolConfidentiality::PUBLIC };
            ToolCallState {
                id: ApprovalId::new(),
                status,
                result: result.map(|r| ToolCallResult::of(&tool_call, ToolResultStatus::Ok, r.to_string())),
                tool_call,
                description: String::new(),
            }
        };
        let state_in = |actor_state: ActorState| AgentChannelState {
            processing_state: actor_state,
            turn: Some(caelumora_model::agent::TurnState {
                tool_calls: vec![call(ApprovalStatus::Approved, None), call(ApprovalStatus::Approved, Some("done")), call(ApprovalStatus::Pending, None)],
                ..Default::default()
            }),
            ..AgentChannelState::default()
        };
        let calls = |state: &AgentChannelState| state.turn.as_ref().map(|t| t.tool_calls.clone()).unwrap_or_default();

        let mut executing = state_in(ActorState::ExecutingTools);
        assert_eq!(AgentActor::prepare_resume(&mut executing), 1, "one approved call had no result");
        let executing = calls(&executing);
        let [unfinished, finished, pending] = executing.as_slice() else { panic!("three calls") };
        let unfinished = unfinished.result.as_ref().expect("answered");
        assert_eq!(unfinished.status, ToolResultStatus::Failed);
        assert!(unfinished.content.contains("not executed"), "the model is told: {}", unfinished.content);
        assert_eq!(finished.result.as_ref().map(|r| r.content.as_str()), Some("done"), "a stored result stays");
        assert!(pending.result.is_none(), "a call nobody approved is still waiting");

        // Every other state resumes as it is.
        for other in [ActorState::Planning, ActorState::WaitingForApproval, ActorState::Reflecting] {
            let mut state = state_in(other);
            assert_eq!(AgentActor::prepare_resume(&mut state), 0, "{other:?}");
            assert!(calls(&state)[0].result.is_none());
            assert_eq!(state.processing_state, other);
        }

        // A turn state without the turn cannot be resumed.
        let mut lost = AgentChannelState { processing_state: ActorState::Planning, ..AgentChannelState::default() };
        AgentActor::prepare_resume(&mut lost);
        assert_eq!(lost.processing_state, ActorState::Idle);
    }

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
    async fn state_history_appender_is_passthrough_and_persists_history() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::new(
            AgentId::default(),
            String::new(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached());
        let processor = StateHistoryAppender {};
        let input = vec![make_message("hello")];

        let output = processor
            .process_messages(input.clone(), &mut turn)
            .await
            .expect("history processor should succeed");

        assert_eq!(output.len(), input.len());
        assert_eq!(output[0].id, input[0].id);

        let history = agent_channel_state.history.messages();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, input[0].id);
    }

    #[tokio::test]
    async fn transcript_appender_is_passthrough_and_extends_the_transcript() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::new(
            AgentId::default(),
            String::new(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached());
        turn.begin();
        let appender = TranscriptAppender {};
        let provider = StateContextProvider {
            prompt_providers: vec![],
        };
        let input = vec![make_message("ctx")];

        let output = appender
            .process_messages(input.clone(), &mut turn)
            .await
            .expect("context appender should succeed");

        assert_eq!(output.len(), input.len());
        assert_eq!(output[0].id, input[0].id);

        provider
            .current_context(&mut turn)
            .await
            .expect("context provider should succeed");
        assert_eq!(turn.context().len(), 1);
        assert_eq!(turn.context()[0].id, input[0].id);
    }

    #[tokio::test]
    async fn prompt_parts_go_where_their_position_says_and_only_the_conversation_is_stored() {
        let agent_id = AgentId::new();
        let mut agent_channel_state = AgentChannelState::default();
        let provider = StateContextProvider {
            prompt_providers: vec![
                Arc::new(StaticPromptProvider { prompt: "Be brief.".to_string(), position: PromptPosition::Start }),
                Arc::new(StaticPromptProvider { prompt: "Notes: none.".to_string(), position: PromptPosition::BeforeTurnInput }),
            ],
        };
        let contents = |turn: &Turn| turn.context().iter()
            .map(|m| format!("{:?}:{}", m.role, m.content.clone().unwrap_or_default()))
            .collect::<Vec<_>>();

        let mut turn = Turn::new(agent_id, "agent".to_string(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached());
        turn.begin();
        let mut earlier = make_message("earlier");
        earlier.role = MessageRole::User;
        let mut question = make_message("question");
        question.role = MessageRole::User;
        turn.extend_transcript(vec![earlier, question]);
        provider.current_context(&mut turn).await.unwrap();
        assert_eq!(contents(&turn), ["System:Be brief.", "User:earlier", "User:Notes: none.", "User:question"],
            "the system prompt first, the notes before the turn's input");
        assert_eq!(turn.transcript().len(), 2, "prompt parts are never stored in the context");

        // A turn without new input - the newest message is the agent's own.
        let mut own = make_message("my answer");
        own.sender_id = ActorId::Agent(agent_id);
        turn.extend_transcript(vec![own]);
        provider.current_context(&mut turn).await.unwrap();
        assert_eq!(contents(&turn).last().map(String::as_str), Some("User:Notes: none."), "then at the end");
    }

    #[tokio::test]
    async fn committing_a_turn_answers_with_its_final_messages_and_clears_its_work() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::new(AgentId::default(), String::new(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached());
        turn.begin();

        let mut internal = make_message("working on it");
        internal.internal = true;
        let mut final_answer = make_message("here you go");
        final_answer.internal = false;

        turn.push_message(internal);
        turn.push_message(final_answer.clone());

        let committed = turn.commit_turn().await;
        assert_eq!(committed.final_messages.len(), 1, "internal steps are not part of the answer");
        assert_eq!(committed.final_messages[0].id, final_answer.id);
        assert_eq!(turn.agent_channel_state().transcript.messages().len(), 2, "but of the transcript");
        assert!(turn.messages().is_empty() && turn.context().is_empty(), "nothing of the turn's work is stored with the state");
    }

    #[test]
    fn a_message_context_is_found_only_for_what_the_agent_produced_and_can_resolve() {
        use caelumora_model::digest::prompt;
        use caelumora_model::message::{ContextItem, ContextRef, MessageRole, PromptRef};

        let actor = ActorId::Agent(AgentId::new());
        let text = |content: &str| Message { id: MessageId::new(), content: Some(content.to_string()), ..Message::default() };
        let mut state = AgentChannelState::default();
        let question = text("question");
        state.transcript.push(question.clone());
        state.history.extend(vec![question.clone()]);
        let answer = text("answer");
        let before = state.transcript.push(answer.clone());
        state.history.push(answer.clone(), Some(ContextRef { transcript: before, prompts: Vec::new(), knowledge: Vec::new() }));

        let seen = context_of(&state, actor, answer.id).expect("the agent produced it");
        let ids: Vec<MessageId> = seen.items.iter().filter_map(|item| match item {
            ContextItem::Conversation { message, .. } => Some(message.id),
            ContextItem::Prompt { .. } => None,
        }).collect();
        assert_eq!(ids, vec![question.id]);
        assert!(matches!(context_of(&state, actor, question.id), Err(MessageContextError::NotRecorded { .. })), "someone else wrote it");

        // A reference to a prompt that was never kept cannot be resolved.
        let lost = text("lost");
        let gone = PromptRef { text: prompt("gone"), role: MessageRole::System, after: None };
        state.history.push(lost.clone(), Some(ContextRef { transcript: before, prompts: vec![gone], knowledge: Vec::new() }));
        assert!(matches!(context_of(&state, actor, lost.id), Err(MessageContextError::Unavailable { .. })));
    }
}
