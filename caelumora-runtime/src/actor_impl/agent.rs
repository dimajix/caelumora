use super::{apply_stage_processors, event_for};
use crate::actor::Actor;
use crate::agent_run_control::{Interrupt, RunControl};
use crate::agent_turn::{CommittedTurn, Turn, MAX_MODEL_CALLS};
use crate::view::ViewsFactory;
use crate::component::{ComponentContainer, ComponentContext, ComponentImpl};
use crate::context::ContextProvider;
use crate::llm::LLMClient;
use crate::message::{EmitOutProcessor, PerceiveInProcessor};
use crate::runtime::ActorRuntime;
use crate::tool::{GuardedToolExecutor, ToolExecutor};
use crate::transport::{EventReceiver, EventSender};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use crate::agent_state::AgentStateHandle;
use caelumora_model::agent::{AgentChannelState, AgentId};
use crate::controller::AgentController;
use caelumora_model::approval::ApprovalStatus;
use caelumora_model::channel::Channel;
use caelumora_model::confidentiality::{Classification, Visibility};
use caelumora_model::message::{Message, MessageEvent};
use caelumora_model::transcript::GivenUpReason;
use caelumora_model::plan::PlanProgress;
use caelumora_model::security::{AccessQuestions, SecurityContext};
use caelumora_model::tool::{ToolCall, ToolCallResult, ToolResultStatus};
use futures::stream::BoxStream;
use tracing::{debug, info, warn, Instrument};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug)]
pub struct AgentActor {
    agent_id: AgentId,
    agent_controller: Arc<AgentController>,
    actor_runtime: ActorRuntime,

    // Components
    /// Assembles the model's context: exactly one (issue 98).
    context_provider: ComponentImpl<dyn ContextProvider>,
    /// The model it talks to: exactly one (issue 130).
    llm_client: ComponentImpl<dyn LLMClient>,
    perceive_in_processors: Vec<ComponentImpl<dyn PerceiveInProcessor>>,
    emit_out_processors: Vec<ComponentImpl<dyn EmitOutProcessor>>,
    /// Runs the agent's tool calls: its `ToolExecutor` component, behind
    /// the runtime's permission checks (issue 127).
    tool_executor: GuardedToolExecutor,
    /// Who sees its reasoning at least (issues 61, 133), from the
    /// personality.
    reasoning_visibility: Visibility,
    /// What its turns navigate (issue 163), as the agent.
    views: ViewsFactory,

    // State
    /// The agent's own state, shared with its actors in other channels and
    /// sessions (issue 62).
    agent_state: Arc<AgentStateHandle>,
    /// Its state in this channel, held by the run loop for as long as it
    /// runs.
    state: Mutex<AgentChannelState>,
    /// Where the run loop stands, and what `cancel`, `stop` and `wakeup`
    /// asked of it.
    control: RunControl,
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
            // Its components get nothing of its runtime (issue 128); what they
            // read of the session they read through their turn (issue 163).
            &ComponentContext::new(security_context.clone(), actor_runtime.security_manager(), actor_runtime.secret_manager()),
        )
        .await?;
        let context_provider = components.one::<dyn ContextProvider>().await?;
        let llm_client = components.one::<dyn LLMClient>().await?;
        let perceive_in_processors = components.get_all::<dyn PerceiveInProcessor>(None);
        let emit_out_processors = components.get_all::<dyn EmitOutProcessor>(None);
        let tool_executor = GuardedToolExecutor::new(actor_runtime.security_manager(), components.one::<dyn ToolExecutor>().await?);

        let mut agent_channel_state = actor_runtime.load_channel_state().await?;
        let not_executed = prepare_resume(&mut agent_channel_state);
        if not_executed > 0 {
            warn!("Agent {} in channel {}: {} approved tool calls were not executed before the restart; the model is told", agent_id, channel.id, not_executed);
        }
        let agent_state = actor_runtime.agent_state(attributes).await?;
        let control = RunControl::new(agent_channel_state.processing_state);
        let views = actor_runtime.views(security_context);

        Ok(AgentActor {
            agent_id,
            agent_controller,
            actor_runtime,
            context_provider,
            llm_client,
            perceive_in_processors,
            emit_out_processors,
            tool_executor,
            reasoning_visibility: personality.reasoning_visibility,
            views,
            agent_state,
            state: Mutex::new(agent_channel_state),
            control,
        })
    }

    /// Takes in what others said: the perceive processors append it to the
    /// transcript and the history. No turn: nothing here is a turn's work,
    /// and nothing is asked of the policy (issues 85, 90).
    async fn perceive(&self, turn: &mut Turn<'_>, messages: Vec<Message>) {
        // The processors act on the agent's state (transcript, history);
        // what they return is not used.
        if let Err(e) = apply_stage_processors(&self.perceive_in_processors, messages, turn).await {
            warn!("PerceiveIn stage failed: {}", e);
        }
    }

    /// A complete message as it leaves the agent - through the emit
    /// processors - and becomes part of the turn. `None` if the processors
    /// dropped it.
    async fn respond_complete(
        &self,
        message: Message,
        turn: &mut Turn<'_>,
    ) -> Option<Message> {
        let msg = self.respond_partial(message, turn).await?;
        turn.push_message(msg.clone());
        Some(msg)
    }

    /// A message as it leaves the agent, through the emit processors; a
    /// partial one is only shown, not kept. `None` if the processors
    /// dropped it.
    async fn respond_partial(
        &self,
        message: Message,
        turn: &mut Turn<'_>,
    ) -> Option<Message> {
        let mut output =
            apply_stage_processors(&self.emit_out_processors, vec![message], turn)
                .await
                .ok()?;
        if output.is_empty() {
            return None;
        }
        Some(output.remove(0))
    }

    /// Starts a new turn in answer to a `ResponseRequested` - whose envelope
    /// has `event_context` and `security_context` - with the channel as it
    /// is now: begins it, announces `ResponseStarted` and builds the
    /// context. The run loop then goes on in `Planning`. Fails without a
    /// channel, which happens only while it is being torn down.
    async fn request_response(
        &self,
        turn: &mut Turn<'_>,
        event_context: &EventContext,
        security_context: &SecurityContext,
        output: &dyn EventSender,
    ) -> Result<()> {
        let classification = self.actor_runtime.channel_loop().map(|c| c.classification())
            .ok_or_else(|| anyhow!("Agent {} has no channel to take a turn in", self.id()))?;
        turn.next_turn(
            event_context.next(self.id(), self.agent_controller.name()),
            security_context.next(self.id()),
            classification,
            self.tool_executor.tools(),
        );
        self.control.begin_turn();

        debug!("Starting generating messages for agent {} {}", self.id(), turn.agent_name());
        turn.send_event(ActorEvent::ResponseStarted, output).await?;
        // The agent's `ContextProvider` assembles what the model is given,
        // whatever its fail policy: without a context there is nothing to
        // give the model. Parts that may be left out are the prompt
        // providers', each by its own policy.
        self.context_provider.facet.current_context(turn).await
            .map_err(|e| e.context(format!("component '{}' could not assemble the context", self.context_provider.id)))
    }

    /// Calls the LLM for one turn step. `create_plan_tool` (when wired in)
    /// contributes its own planning nudge to the turn's context as a
    /// `PromptProvider`, composed by `StateContextProvider` once per
    /// turn - nothing plan-specific needs handling here. The model is
    /// offered the tools the turn began with (issue 137). Fails if the
    /// turn's context cannot be resolved.
    async fn call_llm(
        &self,
        turn: &mut Turn<'_>,
    ) -> Result<BoxStream<'static, crate::llm::LLMResult<MessageEvent>>> {
        let context = turn.full_context()?;
        Ok(self.llm_client.facet
            .chat_completion(&context.messages, &context.tools)
            .await)
    }

    /// Stores the agent's state in its channel as `turn` holds it, with
    /// where the actor stands now. A failure is logged: the actor carries
    /// on with the state in memory.
    async fn store_channel_state(&self, turn: &mut Turn<'_>) {
        turn.set_processing_state(self.control.state());
        if let Err(e) = self.actor_runtime.store_channel_state(turn.agent_channel_state()).await {
            warn!("Cannot store agent channel state of agent {}: {:#}", self.agent_id, e);
        }
    }

    /// Moves the actor to `state` and stores its state in the channel.
    async fn settle(&self, turn: &mut Turn<'_>, state: ActorState) {
        self.control.go_to(state);
        self.store_channel_state(turn).await;
    }

    /// Takes the turn over and ends it - after `Reflecting`, or after a
    /// cancel - and returns the state the agent lands in: the one a tool
    /// requested (the stop tool), or `Idle`; a cancelled turn lands `Idle`
    /// whatever was requested.
    ///
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
    /// stored; a change of its own state by its `AgentStateHandle`. The end
    /// is announced even with no final messages - a plan cancelled before
    /// any step produced a visible answer still has to tell observers (e.g.
    /// the web UI) that generation stopped.
    async fn end_turn(&self, turn: &mut Turn<'_>, cancelled: bool, output: &dyn EventSender) -> Result<ActorState> {
        // Read before the turn is taken over, which ends it.
        let next = if cancelled { ActorState::Idle } else { turn.requested_state().unwrap_or(ActorState::Idle) };
        let CommittedTurn { final_messages, channel_state_changed, ended } = turn.commit_turn(cancelled).await;
        self.store_channel_state(turn).await;
        if channel_state_changed {
            self.actor_runtime.announce_channel_state_changed();
        }
        ended.send_event(ActorEvent::FullResponseGenerated(final_messages), output).await?;
        debug!("Agent {} {} ended its turn{}", self.id(), self.agent_controller.name(), if cancelled { ", cancelled" } else { "" });
        Ok(next)
    }

    /// Idle and Sleeping wait for the next event alike: a
    /// `FullResponseGenerated` is perceived, a `ResponseRequested` starts a
    /// turn - unless `asleep`, where only `wakeup()` ends the sleep. `None`
    /// once the actor's input is closed.
    async fn handle_waiting(
        &self,
        asleep: bool,
        turn: &mut Turn<'_>,
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
        Ok(Some(if started { ActorState::Planning } else { self.control.state() }))
    }

    /// Ends the current turn after `error`: logs the full error, gives the
    /// turn up (issue 145) - its messages stay as a branch no longer active,
    /// nothing it staged is applied - and tells the channel with
    /// `TurnFailed`, so that users see what went wrong instead of a turn that
    /// never ends.
    async fn fail_turn(&self, turn: &mut Turn<'_>, error: &anyhow::Error, output: &dyn EventSender) {
        let agent_name = self.agent_controller.name();
        tracing::error!("Turn of agent {} {} failed: {:#}", self.id(), agent_name, error);

        let message = crate::error::user_message(error)
            .unwrap_or_else(|| "internal error - the details are in the server log".to_string());
        self.give_up(turn, GivenUpReason::Failed { message }, output).await;
    }

    /// Gives `turn` up for `reason` (issue 145) - failed, lowered, stopped:
    /// its messages stay as a branch no longer active, and the calls of its
    /// tools that ran are told on the active branch, in full while the turn
    /// still flows into the channel, else by name, classified as the channel
    /// is now (issue 141). The channel's users are told with `TurnFailed`,
    /// unless the agent was stopped: it left the channel.
    async fn give_up(&self, turn: &mut Turn<'_>, reason: GivenUpReason, output: &dyn EventSender) {
        let names_only = match self.turn_flow(turn).await {
            TurnFlow::Flows => None,
            TurnFlow::Lowered(channel) => Some(channel),
            TurnFlow::NoChannel => Some(turn.classification()),
        };
        let agent_name = self.agent_controller.name();
        let told = match &reason {
            GivenUpReason::Failed { message } => Some(message.clone()),
            GivenUpReason::Lowered { channel } => Some(format!("The channel's classification was lowered to {channel} while {agent_name} was answering; the answer was discarded.")),
            GivenUpReason::Stopped => None,
        };
        let ended = turn.give_up_turn(reason, names_only);
        let Some(message) = told else { return };
        if let Err(e) = ended.send_event(ActorEvent::TurnFailed { message }, output).await {
            warn!("Agent {} {} cannot report its given-up turn: {:#}", self.id(), agent_name, e);
        }
    }

    /// Whether what `turn` saw may still flow into the channel as it is
    /// now (`AccessQuestions::may_publish`, in the agent's name, as its tools'
    /// results are admitted) - which it may not once the channel was
    /// lowered below the classification the turn began with (issue 141).
    async fn turn_flow(&self, turn: &Turn<'_>) -> TurnFlow {
        let Some(channel) = self.actor_runtime.channel_loop().map(|c| c.classification()) else {
            return TurnFlow::NoChannel;
        };
        let context = turn.security_context();
        if self.actor_runtime.security_manager().may_publish(context, context.caller_id, &turn.classification(), &channel).await {
            TurnFlow::Flows
        } else {
            TurnFlow::Lowered(channel)
        }
    }

    /// Handle a single approval event while in WaitingForApproval state.
    /// Records the decision; the run loop dispatches the actual tool execution
    /// on its next iteration via ExecutingTools.
    async fn handle_waiting_for_approval(
        &self,
        turn: &mut Turn<'_>,
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

        // A cancel or a stop ends the waiting, not only an approval: the run
        // loop then finds what it was told to do.
        let cancelled = self.control.token();
        let received = tokio::select! {
            received = rx.recv_matching(is_approval_event) => received,
            _ = cancelled.cancelled() => return Ok(Some(ActorState::WaitingForApproval)),
        };
        match received {
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

    /// Takes the model's `tool_calls` into the turn as the tool guard admits
    /// them (`GuardedToolExecutor::admit`) and asks for the approvals they
    /// wait for. Only records them: whether the turn goes on to run them or
    /// waits is `handle_executing_tools`'s to decide.
    async fn request_approvals(
        &self,
        tool_calls: &[ToolCall],
        turn: &mut Turn<'_>,
        output: &dyn EventSender,
    ) -> Result<()> {
        if tool_calls.is_empty() {
            return Ok(());
        }
        let mut states = Vec::with_capacity(tool_calls.len());
        for call in tool_calls {
            states.push(self.tool_executor.admit(call, turn).await.state(call.clone()));
        }
        for request in turn.tool_calls().register(states) {
            turn.send_event(ActorEvent::ApprovalRequested(request), output).await?;
        }
        Ok(())
    }

    /// Calls the LLM and streams its response for a single turn step. Loops
    /// back to itself (via the run loop, after a trip through
    /// `ExecutingTools`) as long as the model keeps making tool calls, up to
    /// `MAX_MODEL_CALLS` in a turn.
    ///
    /// A message is "final" (shown to the user, kept in `history`) exactly
    /// when it isn't part of executing an in-progress plan
    /// (`Turn::in_plan_step`: not yet at the final answer) - ordinary tool calls,
    /// pre-plan research, and the `create_plan` call itself are all shown
    /// like any other message, same as a personality with no planning
    /// capability at all. Anything internal instead gets
    /// `IntermediatePartialGenerated`/`IntermediateMessageGenerated` instead
    /// of the `Partial`/`Complete` variants, and is excluded from `history`
    /// in `end_turn`.
    async fn handle_llm_turn(
        &self,
        turn: &mut Turn<'_>,
        output: &dyn EventSender,
    ) -> Result<ActorState> {
        let agent_name = self.agent_controller.name();

        if !turn.count_model_call() {
            warn!("Agent {} {} asked its model {} times in one turn; the turn ends", self.id(), agent_name, MAX_MODEL_CALLS);
            return Ok(ActorState::Reflecting);
        }

        let cancelled = self.control.token();
        let mut response = self.call_llm(turn).await?;
        use futures::stream::TryStreamExt;
        loop {
            // A cancel wins over the next chunk, and returning drops the
            // stream - and with it the request to the model.
            let next = tokio::select! {
                biased;
                _ = cancelled.cancelled() => {
                    info!("Stopping for cancelled agent {} {}", self.id(), agent_name);
                    return Ok(ActorState::Cancelling);
                }
                next = response.try_next() => next.with_context(|| "Error during LLM stream".to_string())?,
            };
            let Some(event) = next else { break };

            match event {
                MessageEvent::Partial(mut message) => {
                    stamp(&mut message, turn, self.reasoning_visibility, false);
                    if let Some(msg) = self.respond_partial(message, turn).await {
                        turn.send_event(event_for(msg, true), output).await?;
                    }
                }
                MessageEvent::Complete(mut message) => {
                    stamp(&mut message, turn, self.reasoning_visibility, true);
                    self.request_approvals(&message.tool_calls, turn, output).await?;
                    let converged = message.tool_calls.is_empty();
                    if let Some(msg) = self.respond_complete(message, turn).await {
                        turn.send_event(event_for(msg, false), output).await?;
                    }
                    // An answer without tool calls ends a step of a plan -
                    // the next one begins with another model call - or the
                    // plan itself.
                    if converged {
                        for event in turn.plan_converged() {
                            turn.send_event(event, output).await?;
                        }
                        if turn.plan_progress().is_some() {
                            return Ok(ActorState::Planning);
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
    async fn handle_executing_tools(
        &self,
        turn: &mut Turn<'_>,
        output: &dyn EventSender,
    ) -> Result<ActorState> {
        // Captured before running the calls: a `create_plan` call among them
        // sets the plan as a side effect of executing it, and the result
        // must be classified against the state that produced the call (same
        // as the assistant message that triggered it), not the state that
        // call itself just brought about.
        let progress_before = turn.plan_progress();
        for (id, call) in turn.tool_calls().to_run() {
            let result = self.tool_executor.execute(&call, turn).await;
            turn.tool_calls().record(id, result);
        }
        // Once every call has its result, the turn keeps the tool message for
        // the model (issue 83). A tool result is only ever "final" (shown as
        // the real answer) when it's not part of executing an in-progress
        // plan - mirrors the same internal/final rule applied to the
        // assistant message that triggered it.
        if let Some(tool_message) = turn.tool_calls().collect(progress_before.is_some()) {
            turn.send_event(event_for(tool_message, false), output).await?;

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
            let progress_after = turn.plan_progress();
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
}

/// Whether a turn may still flow into its channel (`AgentActor::turn_flow`).
enum TurnFlow {
    Flows,
    /// It may not: the channel is classified so now.
    Lowered(Classification),
    /// The channel is gone.
    NoChannel,
}

/// Makes `message` of the model the agent's: its sender, its labels
/// (`label`), and whether it is internal - part of executing a plan
/// (`Turn::in_plan_step`); the final answer after the last step is
/// not, and must reach the user live. `reasoning_visibility` and
/// `complete` as for `label`.
fn stamp(message: &mut Message, turn: &Turn, reasoning_visibility: Visibility, complete: bool) {
    message.sender_name = turn.agent_name().to_string();
    message.sender_id = ActorId::Agent(turn.agent_id());
    label(message, turn, reasoning_visibility, complete);
    message.internal = turn.in_plan_step();
}

/// Labels `message` and its parts (issues 54, 61, 133). The message is
/// classified as the turn's channel - every message of the agent's
/// model passes here, partial and complete, before it goes anywhere.
/// Every tool call gets the labels its tool declares now, so that the call
/// and its result are published by what applied at the time of the call; a
/// call of an unknown tool is public: it fails, and its failure hides
/// nothing.
///
/// The reasoning is the owner's (`Visibility::Owner`) if the personality
/// says so (`reasoning_visibility`), once the turn saw the agent's private
/// knowledge, or if the content of a call it leads to is - those of the
/// message once it is `complete`, and while it streams those of every
/// tool the agent could still call, since reasoning comes before the
/// call it leads to. The message carries the turn's label, as
/// everything the agent derives does (issues 133, 168): the join of what
/// the turn has seen, knowledge read from other channels included, at
/// most the channel's classification. The tools are those the turn began
/// with, which the model was offered (issue 137).
fn label(message: &mut Message, turn: &Turn, reasoning_visibility: Visibility, complete: bool) {
    let tools = turn.tools();
    let declared = |name: &str| tools.iter().find(|t| t.name == name).map(|t| t.visibility).unwrap_or_default();
    for call in &mut message.tool_calls {
        call.visibility = declared(&call.function_name);
    }
    // What the model produces is derived from what the turn has seen: it
    // carries the turn's label, at most the channel's classification as
    // the turn began with it (issue 168).
    message.classification = turn.label();

    let owners_call = if complete {
        message.tool_calls.iter().any(|c| !c.visibility.content.is_channel())
    } else {
        tools.iter().any(|t| !t.visibility.content.is_channel())
    };
    let owners = !reasoning_visibility.is_channel() || turn.saw_private_knowledge() || owners_call;
    message.reasoning_visibility = if owners { Visibility::Owner } else { Visibility::Channel };
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
    if state.processing_state.in_turn() && state.turn.is_none() {
        state.processing_state = ActorState::Idle;
    }
    let Some(turn) = state.turn.as_mut().filter(|_| state.processing_state == ActorState::ExecutingTools) else {
        return 0;
    };
    let mut answered = 0;
    let classification = turn.classification.clone();
    for call in &mut turn.tool_calls {
        if call.status == ApprovalStatus::Approved && call.result.is_none() {
            call.result = Some(ToolCallResult::of(&call.tool_call, ToolResultStatus::NotExecuted, crate::tool::not_executed_result(), classification.clone()));
            answered += 1;
        }
    }
    answered
}

#[async_trait]
impl Actor for AgentActor {
    fn id(&self) -> ActorId {
        ActorId::Agent(self.agent_id)
    }
    fn kind(&self) -> String {
        "agent".to_string()
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
        if self.control.state().in_turn() {
            info!("AgentActor {} resumes a turn in state {:?}", self.id(), self.control.state());
        }

        loop {
            let current_state = self.control.state();
            // The name as the agent's controller has it now.
            let mut turn = Turn::new(
                self.agent_id,
                self.agent_controller.name(),
                &mut state,
                self.agent_state.clone(),
                self.llm_client.facet.profile(),
            )
            .with_views(self.views.clone());

            // Idle/WaitingForApproval/Sleep may consume no event before the actor's
            // input channel closes (Ok(None)) - that's the only way this loop ends.
            // Every other state always produces a next state.
            let span = crate::logging::agent_span(self.agent_id, &self.agent_controller.name(), current_state, turn.event_context());

            // Before every step of a turn: a turn that no longer flows into
            // its channel - lowered meanwhile - is given up, whatever it was
            // about to do; nothing of it is run or taken over (issue 141).
            if current_state.in_turn() {
                if let TurnFlow::Lowered(channel) = self.turn_flow(&turn).await {
                    info!("Agent {} {} gives up its turn: the channel was lowered to {}", self.id(), self.agent_controller.name(), channel);
                    self.give_up(&mut turn, GivenUpReason::Lowered { channel }, &*output).instrument(span).await;
                    self.settle(&mut turn, ActorState::Idle).await;
                    continue;
                }
            }

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
                ActorState::Cancelling => Some(self.end_turn(&mut turn, true, &*output).await?),
                ActorState::Planning => Some(self.handle_llm_turn(&mut turn, &*output).await?),
                ActorState::ExecutingTools => Some(self.handle_executing_tools(&mut turn, &*output).await?),
                ActorState::Reflecting => Some(self.end_turn(&mut turn, false, &*output).await?),
            }) }.instrument(span.clone()).await;

            // A failure inside a turn ends the turn, not the agent (issue 02).
            // Errors in the waiting states are the transport's and still end
            // the actor.
            let next_state = match result {
                Ok(next) => next,
                Err(e) if current_state.in_turn() => {
                    self.fail_turn(&mut turn, &e, &*output).instrument(span).await;
                    Some(ActorState::Idle)
                }
                Err(e) => return Err(e),
            };

            // What `cancel` or `stop` asked for is decided here, after the
            // step it interrupted, and nowhere else (issue 152). Told to
            // stop, the actor is leaving the channel: a turn under way is
            // not resumed later, so it is given up rather than stored to be
            // resumed (issues 75, 145).
            let interrupt = self.control.take_interrupt();
            if interrupt == Some(Interrupt::Stop) {
                self.give_up(&mut turn, GivenUpReason::Stopped, &*output).await;
                self.settle(&mut turn, ActorState::Idle).await;
                break;
            }

            // The state goes on first, then it is stored: what a crash leaves
            // in the store is the step that comes next, not the one that is
            // done - resuming it neither repeats a model call whose answer is
            // already in the turn nor asks the model before a tool result is
            // in (issue 85).
            if let Some(next) = next_state {
                // A cancel ends a turn still under way, whatever its step
                // returned; one that came as the turn ended finds nothing
                // left to end.
                let cancelled = interrupt == Some(Interrupt::Cancel) && next.in_turn();
                self.control.go_to(if cancelled { ActorState::Cancelling } else { next });
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
        self.control.state()
    }

    fn wakeup(&self) {
        // Called for every agent on every user message, sleeping or not.
        debug!(
            "Waking up agent {} {}",
            self.id(),
            self.agent_controller.name()
        );
        self.control.wakeup();
    }

    fn cancel(&self) {
        info!(
            "Cancelling agent {} {}",
            self.id(),
            self.agent_controller.name()
        );
        self.control.cancel();
    }

    fn stop(&self) {
        info!("Stopping agent {} {}: it left the channel", self.id(), self.agent_controller.name());
        self.control.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use caelumora_model::confidentiality::Confidentiality;
    use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
    use caelumora_model::message::{MessageId, MessageRole};
    use caelumora_model::tool::{Tool, ToolParameters, ToolVisibility};

    fn tool(name: &str, visibility: ToolVisibility) -> Tool {
        Tool {
            name: name.to_string(),
            description: format!("does {name}"),
            parameters: ToolParameters::object(Vec::<(String, ToolParameters)>::new(), Vec::<String>::new()),
            requires_approval: false,
            visibility,
            classification: None,
        }
    }

    fn call(name: &str) -> ToolCall {
        // Labelled as the model can never label a call: `label` must set it.
        ToolCall { id: None, function_name: name.to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::HIDDEN }
    }

    fn answer(calls: &[&str]) -> Message {
        Message {
            id: MessageId::new(),
            role: MessageRole::Assistant,
            content: Some("answer".to_string()),
            reasoning: Some("thinking".to_string()),
            tool_calls: calls.iter().map(|name| call(name)).collect(),
            ..Message::new(Classification::general(Confidentiality::Public))
        }
    }

    #[test]
    fn the_model_s_message_is_the_agent_s_classified_as_what_its_turn_has_seen_and_its_calls_as_their_tools_declare() {
        let agent = AgentId::new();
        let channel = Classification::general(Confidentiality::Confidential);
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::new(agent, "agent".to_string(), &mut agent_channel_state, AgentStateHandle::detached(), Default::default());
        turn.next_turn(Default::default(), Default::default(), channel.clone(), vec![tool("shown", ToolVisibility::SHOWN), tool("call_only", ToolVisibility::CALL_ONLY)]);

        let mut message = answer(&["shown", "call_only", "unknown"]);
        stamp(&mut message, &turn, Visibility::Channel, true);

        assert_eq!((message.sender_id, message.sender_name.as_str()), (ActorId::Agent(agent), "agent"));
        assert!(!message.internal, "no plan under way");
        assert_eq!(message.classification, Classification::PUBLIC, "the turn has seen nothing yet");
        turn.see(&Classification::general(Confidentiality::Internal));
        stamp(&mut message, &turn, Visibility::Channel, true);
        assert_eq!(message.classification, Classification::general(Confidentiality::Internal), "as what it has seen");
        turn.see(&Classification::general(Confidentiality::StrictlyConfidential));
        stamp(&mut message, &turn, Visibility::Channel, true);
        assert_eq!(message.classification, channel, "never above the channel the turn began in");
        let labels: Vec<_> = message.tool_calls.iter().map(|c| c.visibility).collect();
        assert_eq!(labels, [ToolVisibility::SHOWN, ToolVisibility::CALL_ONLY, ToolVisibility::SHOWN],
            "as their tools declare; an unknown tool's call fails and hides nothing");
    }

    #[test]
    fn the_reasoning_is_the_owner_s_where_the_personality_what_the_turn_saw_or_the_call_it_leads_to_says_so() {
        let agent = AgentId::new();
        let both = || vec![tool("shown", ToolVisibility::SHOWN), tool("call_only", ToolVisibility::CALL_ONLY)];
        let reasoning = |tools: Vec<Tool>, personality: Visibility, private_knowledge: bool, calls: &[&str], complete: bool| {
            let mut agent_channel_state = AgentChannelState::default();
            let mut turn = Turn::new(agent, "agent".to_string(), &mut agent_channel_state, AgentStateHandle::detached(), Default::default());
            turn.next_turn(Default::default(), Default::default(), Classification::CHANNEL_DEFAULT, tools);
            if private_knowledge {
                turn.record_knowledge(KnowledgeRef { channel: turn.channel(), source: KnowledgeSource::PrivateNotes { agent, digest: None }, classification: None });
            }
            let mut message = answer(calls);
            label(&mut message, &turn, personality, complete);
            message.reasoning_visibility
        };

        assert_eq!(reasoning(both(), Visibility::Channel, false, &["shown"], true), Visibility::Channel, "nothing says otherwise");
        assert_eq!(reasoning(both(), Visibility::Owner, false, &["shown"], true), Visibility::Owner, "the personality says so");
        assert_eq!(reasoning(both(), Visibility::Channel, true, &["shown"], true), Visibility::Owner, "the turn saw private notes");
        assert_eq!(reasoning(both(), Visibility::Channel, false, &["call_only"], true), Visibility::Owner, "it leads to a call whose content is the owner's");
        assert_eq!(reasoning(both(), Visibility::Channel, false, &["shown"], false), Visibility::Owner,
            "while it streams, the agent could still call a tool whose content is the owner's");
        assert_eq!(reasoning(vec![tool("shown", ToolVisibility::SHOWN)], Visibility::Channel, false, &[], false), Visibility::Channel,
            "but not if it has no such tool");
    }

    #[test]
    fn a_resumed_turn_answers_unfinished_tool_calls_as_not_executed_instead_of_running_them() {
        use caelumora_model::tool::{ToolCallState, ToolVisibility};
        let call = |status: ApprovalStatus, result: Option<&str>| {
            let tool_call = ToolCall { id: None, function_name: "note".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN };
            match (status, result) {
                (ApprovalStatus::Pending, _) => ToolCallState::pending(tool_call, String::new()),
                (_, Some(r)) => {
                    let result = ToolCallResult::of(&tool_call, ToolResultStatus::Ok, r.to_string(), caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT);
                    ToolCallState::answered(tool_call, result)
                }
                _ => ToolCallState::approved(tool_call),
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
        assert_eq!(prepare_resume(&mut executing), 1, "one approved call had no result");
        let executing = calls(&executing);
        let [unfinished, finished, pending] = executing.as_slice() else { panic!("three calls") };
        let unfinished = unfinished.result.as_ref().expect("answered");
        assert_eq!(unfinished.status, ToolResultStatus::NotExecuted);
        assert!(unfinished.content.contains("not executed"), "the model is told: {}", unfinished.content);
        assert_eq!(finished.result.as_ref().map(|r| r.content.as_str()), Some("done"), "a stored result stays");
        assert!(pending.result.is_none(), "a call nobody approved is still waiting");

        // Every other state resumes as it is.
        for other in [ActorState::Planning, ActorState::WaitingForApproval, ActorState::Reflecting] {
            let mut state = state_in(other);
            assert_eq!(prepare_resume(&mut state), 0, "{other:?}");
            assert!(calls(&state)[0].result.is_none());
            assert_eq!(state.processing_state, other);
        }

        // A turn state without the turn cannot be resumed.
        let mut lost = AgentChannelState { processing_state: ActorState::Planning, ..AgentChannelState::default() };
        prepare_resume(&mut lost);
        assert_eq!(lost.processing_state, ActorState::Idle);
    }
}
