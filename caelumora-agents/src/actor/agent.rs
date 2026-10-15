use super::body::AgentBody;
use super::body::retry_target;
use crate::error::failure_message;
use crate::message::apply_stage_processors;
use caelumora_runtime::actor::{Actor, Answering};
use super::run_control::{interruption, Interrupt};
use crate::turn::{CommittedTurn, Turn, MAX_MODEL_CALLS};
use crate::command::AgentCommandProvider;
use crate::compaction::{CompactionPolicy, CompactionStrategy};
use caelumora_runtime::component::ComponentImpl;
use crate::prompt::ContextProvider;
use crate::llm::LLMClient;
use caelumora_runtime::logging::{agent_span, envelope_span};
use crate::message::{EmitOutProcessor, PerceiveInProcessor};
use caelumora_runtime::runtime::ActorRuntime;
use crate::tool::{not_executed_result, ToolExecutor, ToolProvider};
use caelumora_runtime::transport::{EventReceiver, EventSender};
use anyhow::{anyhow, Context, Result, Error};
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, RetryTarget};
use caelumora_model::agent::{AgentChannelState, Personality, RetryRefused};
use caelumora_model::command::Command;
use caelumora_model::component::ComponentConfig;
use caelumora_model::turn::TurnState;
use caelumora_model::work::TurnId;
use caelumora_runtime::controller::AgentController;
use caelumora_model::approval::{ApprovalId, ApprovalStatus};
use caelumora_model::confidentiality::{Classification, Visibility};
use caelumora_model::message::{Message, MessageEvent};
use caelumora_model::turn::GivenUpReason;
use caelumora_model::plan::PlanProgress;
use caelumora_model::security::SecurityContext;
use caelumora_model::tool::{ToolCall, ToolCallState, ToolCallStatus, ToolSet};
use caelumora_model::type_map::TypeKey;
use caelumora_model::value::name::Name;
use tracing::{debug, info, warn, Instrument};
use std::sync::Arc;
use chrono::Utc;

#[derive(Debug)]
pub struct AgentActor {
    /// What it is in the channel, as every agent is (issue 355): who it is,
    /// in whose name it acts, its state there, how it hears, its commands,
    /// and where its run loop stands (issue 363).
    body: AgentBody,

    // Components
    /// The model it talks to: exactly one (issue 130).
    llm_client: ComponentImpl<dyn LLMClient>,
    /// When it compacts its context on its own: exactly one (issue 284).
    compaction_policy: ComponentImpl<dyn CompactionPolicy>,
    /// How its context is compacted: exactly one (issues 16, 284).
    compaction: ComponentImpl<dyn CompactionStrategy>,
    emit_out_processors: Vec<ComponentImpl<dyn EmitOutProcessor>>,
    /// Runs the agent's tool calls with the tools of its components, behind
    /// the runtime's permission checks (issues 127, 280).
    tool_executor: ToolExecutor,
    /// Who sees its reasoning at least (issues 61, 133), from the
    /// personality.
    reasoning_visibility: Visibility,
}

impl AgentActor {
    /// The facets it asks its components for (issue 337) - its commands
    /// through its body.
    pub const COMPONENT_FACETS: &[TypeKey] = &[
        TypeKey::of::<dyn ContextProvider>(),
        TypeKey::of::<dyn LLMClient>(),
        TypeKey::of::<dyn CompactionPolicy>(),
        TypeKey::of::<dyn CompactionStrategy>(),
        TypeKey::of::<dyn PerceiveInProcessor>(),
        TypeKey::of::<dyn EmitOutProcessor>(),
        TypeKey::of::<dyn ToolProvider>(),
        TypeKey::of::<dyn AgentCommandProvider>(),
    ];

    /// The actor of the agent `agent_controller` holds - the session's
    /// controller of it, shared with its actors in the session's other
    /// channels (issue 120) - built from its `personality`, as the channel
    /// read it for the agent, in the channel of `actor_runtime`.
    pub async fn new(agent_controller: Arc<AgentController>, personality: Personality, actor_runtime: ActorRuntime) -> Result<Self> {
        let channel_id = actor_runtime.channel_controller().id();
        let agent_id = agent_controller.id();
        // A turn stored under way is resumed (issue 75).
        let resume = |state: &mut AgentChannelState| {
            let not_executed = state.prepare_resume(&not_executed_result());
            if not_executed > 0 {
                warn!("Agent {} in channel {}: {} approved tool calls were not executed before the restart; the model is told", agent_id, channel_id, not_executed);
            }
        };
        let (body, components) = AgentBody::build(agent_controller, &personality, Self::COMPONENT_FACETS, &ComponentConfig::default(), actor_runtime, resume).await?;
        let llm_client = components.one::<dyn LLMClient>().await?;
        let compaction_policy = components.one::<dyn CompactionPolicy>().await?;
        let compaction = components.one::<dyn CompactionStrategy>().await?;
        let emit_out_processors = components.get_all::<dyn EmitOutProcessor>(None)?;
        let tool_executor = ToolExecutor::new(components.get_all::<dyn ToolProvider>(None)?, body.runtime.clone())?;

        Ok(AgentActor {
            body: body.with_profile(llm_client.profile()),
            llm_client,
            compaction_policy,
            compaction,
            emit_out_processors,
            tool_executor,
            reasoning_visibility: personality.reasoning_visibility,
        })
    }

    /// A message as it leaves the agent, through the emit processors - a
    /// complete one to be said, a partial one only shown. `None` if the
    /// processors dropped it.
    async fn respond(
        &self,
        message: Message,
        turn: &mut Turn<'_>,
    ) -> Option<Message> {
        let mut output =
            apply_stage_processors(&self.emit_out_processors, vec![message], turn.security_context())
                .await
                .ok()?;
        if output.is_empty() {
            return None;
        }
        Some(output.remove(0))
    }

    /// The turn under way in `state`, as its steps work with it; `None`
    /// between turns.
    fn turn<'s>(&self, state: &'s mut AgentChannelState) -> Option<Turn<'s>> {
        self.body.turn(state)
    }

    /// The turn under way in `state`, for a step of a turn. Without one -
    /// `prepare_resume` makes sure a state in a turn has its working state -
    /// the step fails as a turn's step does.
    fn turn_under_way<'s>(&self, state: &'s mut AgentChannelState) -> Result<Turn<'s>> {
        self.turn(state).ok_or_else(|| anyhow!("Agent {} has no turn under way in {:?}", self.id(), self.body.control.state()))
    }

    /// Starts a new turn in answer to the `ResponseRequested` in `request`,
    /// in its contexts continued in the agent's name, with the channel as it
    /// is now: begins it on `state`, announces `ResponseStarted` and builds
    /// the context. The run loop then goes on in `Planning`. Fails without a
    /// channel, which happens only while it is being torn down.
    async fn request_response(
        &self,
        state: &mut AgentChannelState,
        request: &Envelope,
        output: &dyn EventSender,
    ) -> Result<()> {
        let classification = self.body.runtime.channel_classification();
        let Answering { event_context, security_context } = Answering::to(request, self.id(), self.body.agent.name(), &self.body.security_context());
        // Judged for whoever asked, in the channel as the turn begins.
        let tool_sets = self.tool_executor.tool_sets(&security_context, &classification).await;
        let turn_id = state.begin_turn(
            event_context,
            security_context,
            classification,
            tool_sets,
            self.llm_client.profile(),
        );
        self.body.control.begin_turn(turn_id);
        let mut turn = self.turn_under_way(state)?;

        debug!("Starting generating messages for agent {} {} in turn {}", self.id(), turn.agent_name(), turn_id);
        turn.send_event(ActorEvent::ResponseStarted { turn_id }, output).await?;
        let (parts, knowledge) = self.body.current_context(&turn.work()).await?;
        turn.set_context(parts, knowledge);
        Ok(())
    }

    /// Moves the actor to `next` and stores its state in the channel.
    async fn settle(&self, state: &mut AgentChannelState, next: ActorState) {
        self.body.control.go_to(next);
        self.body.store(state).await;
    }

    /// Ends the turn under way in `state`, which came to its end: the step
    /// for what the agent does once a turn is done, before it is taken over.
    async fn handle_reflecting(&self, state: &mut AgentChannelState, output: &dyn EventSender) -> Result<ActorState> {
        self.end_turn(state, false, output).await
    }

    /// Ends the turn under way in `state`, which was cancelled: nothing of
    /// it is reflected on.
    async fn handle_cancelling(&self, state: &mut AgentChannelState, output: &dyn EventSender) -> Result<ActorState> {
        self.end_turn(state, true, output).await
    }

    /// Takes the turn under way in `state` over and ends it - reflected on
    /// (`handle_reflecting`), or cancelled (`handle_cancelling`) - and returns the state the agent
    /// lands in: the one a tool requested (the stop tool), or `Idle`; a
    /// cancelled turn lands `Idle` whatever was requested.
    ///
    /// `context` (this agent's own future-turn LLM input) keeps every message
    /// accumulated this turn, including internal deliberation/plan-execution
    /// scaffolding - it needs the full detail to stay coherent. `history`
    /// (the audit/user-facing trail - see `ChannelLoop::append_history` and
    /// `channel.history()`) only keeps genuinely final messages.
    ///
    /// The turn is taken over and stored before its `TurnFinished`
    /// announces its end (issue 65): whoever reacts to it - another agent
    /// whose turn it starts, a user interface reloading notes - reads the
    /// state as the turn left it. A change of the agent's state in the
    /// channel is announced as the channel's `channel::AGENT_STATE_CHANGED`
    /// once stored; a change of its own state by its `AgentStateHandle`. The end
    /// is announced even with no final messages - a plan cancelled before
    /// any step produced a visible answer still has to tell observers (e.g.
    /// the web UI) that generation stopped.
    async fn end_turn(&self, state: &mut AgentChannelState, cancelled: bool, output: &dyn EventSender) -> Result<ActorState> {
        let turn = self.turn_under_way(state)?;
        // Read before the turn is taken over, which ends it.
        let next = if cancelled { ActorState::Idle } else { turn.requested_state().unwrap_or(ActorState::Idle) };
        let turn_id = turn.turn_id();
        let CommittedTurn { final_messages, channel_state_changed, answering, abandoned_calls } = turn.commit(cancelled, output).await;
        self.body.store(state).await;
        if channel_state_changed {
            self.body.runtime.announce_channel_state_changed();
        }
        withdraw_requests(&answering, &abandoned_calls, output).await?;
        answering.send(ActorEvent::TurnFinished { turn_id, messages: final_messages }, output).await?;
        debug!("Agent {} {} ended its turn{}", self.id(), self.body.agent.name(), if cancelled { ", cancelled" } else { "" });
        Ok(next)
    }

    /// Idle and Sleeping wait for the next event alike, and take it in with
    /// everything queued behind it (`AgentBody::hear`, issue 222): what others said
    /// is perceived - no turn, nothing asked of the policy (issues 85, 90) -
    /// and at most one turn starts, unless the agent sleeps, where only
    /// `wakeup()` ends the sleep. The channel woke the agent for the
    /// requests to answer again (issues 234, 235): one of them is judged
    /// (`retry`) - the one of the turn `given_up_for` it, whose answer is
    /// gone already, else the last - and the others are rejected as asked
    /// again. A retry accepted and a request to respond in the same batch
    /// are one turn, answering the conversation as it stands, in the
    /// contexts of the request; a retry alone begins one in its own. A
    /// command is carried out where it came in the batch, asleep or not
    /// (issue 269). `None` once the actor's input is closed.
    async fn handle_waiting(
        &self,
        state: &mut AgentChannelState,
        rx: &mut Box<dyn EventReceiver>,
        given_up_for: &mut Option<TurnId>,
        output: &dyn EventSender,
    ) -> Result<Option<ActorState>> {
        // Woken by the channel without a message - `/wakeup` - it says so
        // and waits on (issue 243).
        let received = loop {
            tokio::select! {
                received = rx.recv() => break received,
                () = self.body.control.woken() => self.body.announce_state(output).await,
            }
        };
        let Some(received) = received else { return Ok(None) };
        // It gives its transcript nowhere else: of a change it does not hold on
        // its path there is nobody to tell.
        let taken = self.body.hear(state, received, rx, output).await;
        let (request, mut retries) = (taken.request, taken.retries);
        // Read once taken in, not before waiting: the channel wakes an agent
        // before it delivers the message that asks it, and the request comes
        // in the same batch.
        let asleep = self.body.control.state() == ActorState::Sleeping;
        let request = request.filter(|_| !asleep);

        let of_given_up = given_up_for.map(RetryTarget::Turn);
        let judged = retries.iter().rposition(|retry| retry_target(retry) == of_given_up).or(retries.len().checked_sub(1));
        let judged = judged.map(|at| retries.remove(at));
        self.body.reject_retries(retries, RetryRefused::AskedAgain, output).await;
        let mut accepted = None;
        if let Some(retry) = judged {
            let span = envelope_span(&retry.context, retry.event.kind());
            match self.retry(state, &retry, *given_up_for, output).instrument(span).await {
                Ok(true) => accepted = Some(retry),
                Ok(false) => {}
                Err(e) => warn!("Agent {} cannot answer a request to answer again: {:#}", self.id(), e),
            }
        }
        let started = match request.as_ref().or(accepted.as_ref()) {
            Some(answered) => self.begin_answer(state, answered, output).await,
            None => false,
        };
        if started {
            *given_up_for = None;
        }
        rx.commit().await?;
        Ok(Some(if started { ActorState::Planning } else { self.body.control.state() }))
    }

    /// Begins a turn in answer to `request` (`request_response`), and
    /// whether it began: an error is the turn's, which then fails.
    async fn begin_answer(&self, state: &mut AgentChannelState, request: &Envelope, output: &dyn EventSender) -> bool {
        let span = envelope_span(&request.context, request.event.kind());
        match self.request_response(state, request, output).instrument(span.clone()).await {
            Ok(()) => true,
            // E.g. a fail-closed context provider: the turn never starts.
            Err(e) => {
                self.fail_turn(state, &e, output).instrument(span).await;
                false
            }
        }
    }

    /// Answers the `RetryRequested` in `request` (issues 234, 235), in its
    /// contexts continued in the agent's name. The turn `given_up_for` it
    /// can be answered again, its answer being gone already; another turn
    /// is over (`Ended`); a message can be if it is the agent's last answer
    /// (`AgentChannelState::retried`). If it can, the agent says so with
    /// `RetryAccepted`, takes back what the retry replaces - in its own
    /// state, and announced with `MessageRevoked` for what its history
    /// holds, the rest being internal steps nobody else saw - and returns
    /// `true`: a turn is to begin, given the transcript as it stands.
    /// Otherwise it says why with `RetryRejected`.
    async fn retry(&self, state: &mut AgentChannelState, request: &Envelope, given_up_for: Option<TurnId>, output: &dyn EventSender) -> Result<bool> {
        let Some(target) = retry_target(request) else { return Ok(false) };
        let answering = Answering::to(request, self.id(), self.body.agent.name(), &self.body.security_context());
        let can = match target {
            RetryTarget::Turn(turn) if Some(turn) == given_up_for => Ok(Vec::new()),
            // Its last turn given up: answered anew after it, nothing taken
            // back (issue 259).
            RetryTarget::Turn(turn) => state.answers_again_after(turn).map(|()| Vec::new()),
            RetryTarget::Message(message) => state.retried(message, self.id()),
        };
        let taken_back = match can {
            Ok(taken_back) => taken_back,
            Err(refused) => {
                answering.send(ActorEvent::RetryRejected { target, reason: refused }, output).await?;
                return Ok(false);
            }
        };
        answering.send(ActorEvent::RetryAccepted { target }, output).await?;
        let at = Utc::now();
        for id in taken_back {
            let said = state.history.message(id).is_some();
            state.history.revoke(id, at);
            state.transcript.revoke(id, at);
            if said {
                answering.send(ActorEvent::MessageRevoked { message: id, at }, output).await?;
            }
        }
        Ok(true)
    }

    /// Ends the turn under way in `state` after `error`: logs the full
    /// error, gives the turn up (issue 145) - its messages stay, nothing it
    /// staged is applied - and tells the channel with `TurnGivenUp`, so that
    /// users see what went wrong instead of a turn that never ends. An error
    /// before a turn began is logged only: there is no turn to end, nor a
    /// request to answer in.
    async fn fail_turn(&self, state: &mut AgentChannelState, error: &Error, output: &dyn EventSender) {
        let agent_name = self.body.agent.name();
        tracing::error!("Turn of agent {} {} failed: {:#}", self.id(), agent_name, error);

        self.give_up(state, GivenUpReason::Failed { message: failure_message(error) }, output).await;
    }

    /// Gives the turn under way in `state` up for `reason` (issue 145) -
    /// failed, lowered, stopped, asked for again: its messages stay where
    /// they were said, followed by its end (issue 258) - recorded as lowered
    /// once the turn no longer flows into the channel (issue 141), whatever
    /// ended it, so that its model is given only which tools it called.
    /// The approval requests it still waited for are withdrawn (issue 148),
    /// and the channel's users are told with `TurnGivenUp`, with the reason
    /// recorded - also by an agent that was stopped: it is leaving the
    /// channel, which takes what it says until its task has ended (issue
    /// 262). A turn asked for again is followed by `RetryAccepted` and the
    /// new answer (issue 235). Between turns there is nothing to give up.
    async fn give_up(&self, state: &mut AgentChannelState, reason: GivenUpReason, output: &dyn EventSender) {
        let Some(under_way) = state.turn.as_ref() else { return };
        // A turn that no longer flows into its channel is recorded as
        // lowered, whatever ended it: its model is then given only which
        // tools it called (issue 258). What its users are told follows from
        // what ended it.
        let recorded = match self.lowered(under_way) {
            Some(channel) => GivenUpReason::Lowered { channel },
            None => reason.clone(),
        };
        let turn_id = under_way.work.turn_id;
        let agent_name = self.body.agent.name();
        let Some(turn) = self.turn(state) else { return };
        let (answering, abandoned_calls) = turn.give_up(recorded.clone(), output).await;
        if let Err(e) = withdraw_requests(&answering, &abandoned_calls, output).await {
            warn!("Agent {} {} cannot withdraw the approval requests of its given-up turn: {:#}", self.id(), agent_name, e);
        }
        if let Err(e) = answering.send(ActorEvent::TurnGivenUp { turn_id, reason: recorded }, output).await {
            warn!("Agent {} {} cannot report its given-up turn: {:#}", self.id(), agent_name, e);
        }
    }

    /// The channel's classification as it is now, if what the turn
    /// `under_way` saw may no longer flow into it
    /// (`ConfidentialityPolicy::may_publish`, in the agent's name, as its
    /// tools' results are admitted) - once the channel was lowered below the
    /// classification the turn began with (issue 141); `None` while it flows.
    fn lowered(&self, under_way: &TurnState) -> Option<Classification> {
        let channel = self.body.runtime.channel_classification();
        let flows = self.body.runtime.confidentiality().may_publish(&under_way.work.security_context, &under_way.work.classification, &channel);
        (!flows).then_some(channel)
    }

    /// Waits for a person while in `WaitingForUser`, and records what one
    /// event of theirs settles: a decision on an approval, or the answers to
    /// questions, or that they were declined (issue 165). What the decision
    /// leads to - questions an approved call now asks, the closing of
    /// questions answered - is sent at once; the run loop runs what may run
    /// via `ExecutingTools`. Answers that do not count - from someone not
    /// asked, to questions no longer open, not of the kind asked - are left
    /// out, logged without what they say, and the turn waits on.
    async fn handle_waiting_for_user(
        &self,
        turn: &mut Turn<'_>,
        rx: &mut Box<dyn EventReceiver>,
        output: &dyn EventSender,
    ) -> Result<Option<ActorState>> {
        let for_the_waiting = |env: &Envelope| {
            matches!(
                env.event,
                ActorEvent::ApprovalGranted(_)
                    | ActorEvent::ApprovalDenied(_)
                    | ActorEvent::ApprovalTimedOut(_)
                    | ActorEvent::QuestionAnswered { .. }
                    | ActorEvent::QuestionDeclined { .. }
            )
        };

        // A cancel or a stop ends the waiting, not only an approval: the run
        // loop then finds what it was told to do.
        let cancelled = self.body.control.token();
        let received = tokio::select! {
            received = rx.recv_matching(for_the_waiting) => received,
            _ = cancelled.cancelled() => return Ok(Some(ActorState::WaitingForUser)),
        };
        // Channel is closed
        let Some(envelope) = received else { return Ok(None) };
        let sender = envelope.context.sender_id;
        let settled = match envelope.event {
            ActorEvent::ApprovalGranted(r) => self.decided(turn, r.id, ApprovalStatus::Approved),
            ActorEvent::ApprovalDenied(r) => self.decided(turn, r.id, ApprovalStatus::Denied),
            ActorEvent::ApprovalTimedOut(r) => self.decided(turn, r.id, ApprovalStatus::Timeout),
            ActorEvent::QuestionAnswered { id, responses, .. } => turn.tool_calls().answer(id, sender, envelope.context.sender_name.clone(), responses)
                .map(|closed| vec![closed])
                .inspect_err(|e| info!("Agent {} leaves out an answer of {} to questions {}: {}", self.id(), sender, id, e))
                .ok(),
            ActorEvent::QuestionDeclined { id, .. } => turn.tool_calls().decline(id, sender, envelope.context.sender_name.clone())
                .map(|closed| vec![closed])
                .inspect_err(|e| info!("Agent {} leaves out that {} declined questions {}: {}", self.id(), sender, id, e))
                .ok(),
            _ => None,
        };
        rx.commit().await?;
        let Some(events) = settled else { return Ok(Some(ActorState::WaitingForUser)) };
        for event in events {
            turn.send_event(event, output).await?;
        }
        Ok(Some(ActorState::ExecutingTools))
    }

    /// Records the approver's `decision` on the approval `id`, and returns
    /// the requests it leads to; `None` if no call waits for it.
    fn decided(&self, turn: &mut Turn<'_>, id: ApprovalId, decision: ApprovalStatus) -> Option<Vec<ActorEvent>> {
        let events = turn.tool_calls().decide(id, decision);
        if events.is_none() {
            warn!("Received approval for unknown approval id {}", id);
        }
        events
    }

    /// Takes the model's `tool_calls` into the turn as the tool guard admits
    /// them (`ToolExecutor::admit`) and asks for the approvals they
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
            turn.send_event(request, output).await?;
        }
        Ok(())
    }

    /// Asks again for the approvals a resumed turn waits for (issue 172):
    /// the turn is their record, the approver holds them only in memory and
    /// starts empty after a restart. The same ids as before, so that the
    /// approver, should it still hold them, only announces them again.
    async fn request_pending(&self, turn: &mut Turn<'_>, output: &dyn EventSender) -> Result<()> {
        for request in turn.tool_calls().requests() {
            turn.send_event(request, output).await?;
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
    async fn handle_planning(
        &self,
        turn: &mut Turn<'_>,
        output: &dyn EventSender,
    ) -> Result<ActorState> {
        let agent_name = self.body.agent.name();

        if self.compaction_due(turn) {
            return Ok(ActorState::Busy);
        }
        if !turn.count_model_call() {
            warn!("Agent {} {} asked its model {} times in one turn; the turn ends", self.id(), agent_name, MAX_MODEL_CALLS);
            return Ok(ActorState::Reflecting);
        }

        let cancelled = self.body.control.token();
        // The turn's context - prompt parts, a planning nudge among them if
        // `create_plan_tool` is wired in, and the tools the turn began with
        // (issue 137); it fails if it cannot be resolved.
        let (messages, tools) = turn.full_context()?;
        let mut response = self.llm_client.chat_completion(&messages, &tools).await;
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
                    if let Some(msg) = self.respond(message, turn).await {
                        turn.show(msg, output).await?;
                    }
                }
                MessageEvent::Complete(mut message) => {
                    stamp(&mut message, turn, self.reasoning_visibility, true);
                    self.request_approvals(&message.tool_calls, turn, output).await?;
                    let converged = message.tool_calls.is_empty();
                    if let Some(msg) = self.respond(message, turn).await {
                        turn.say(msg, output).await?;
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

    /// Whether the context of `turn` is to be compacted before its next model
    /// call (issue 284), as its policy says - asked once before each call.
    fn compaction_due(&self, turn: &Turn<'_>) -> bool {
        turn.may_compact() && self.compaction_policy.is_due(&turn.work())
    }

    /// Compacts the context of `turn` before its next model call (issue
    /// 284), as its strategy does, telling the users that it does so
    /// (`ActivityStarted`) and what came of it (`ActivityFinished`). Tried
    /// once before the call; the call follows whatever came of it. A cancel
    /// ends it, as it ends a call; a failure ends the turn - the call it
    /// was to make room for would not fit -, the transcript as it was.
    async fn handle_busy(&self, turn: &mut Turn<'_>, output: &dyn EventSender) -> Result<ActorState> {
        const ACTIVITY: &str = "compacting its context";
        turn.try_compaction();
        let turn_id = turn.turn_id();
        turn.send_event(ActorEvent::ActivityStarted { turn_id, activity: ACTIVITY.to_string() }, output).await?;
        let cancelled = self.body.control.token();
        let compacted = tokio::select! {
            biased;
            _ = cancelled.cancelled() => {
                info!("Stopping compacting for cancelled agent {} {}", self.id(), turn.agent_name());
                return Ok(ActorState::Cancelling);
            }
            compacted = self.compaction.compact(turn) => compacted,
        };
        let compaction = compacted.with_context(|| format!("component '{}' could not compact the context", self.compaction.id))?;
        let said = match compaction {
            Some(compaction) => compaction.describe(turn.agent_name()),
            None => format!("{} found nothing to compact: it keeps the newest part of its context as it is.", turn.agent_name()),
        };
        turn.send_event(ActorEvent::ActivityFinished { turn_id, activity: ACTIVITY.to_string(), output: said }, output).await?;
        Ok(ActorState::Planning)
    }

    /// Runs any tool calls that are ready (Approved, or newly-approved on resume
    /// from WaitingForUser). Loops back to Planning once all calls for this
    /// step have results, or parks in WaitingForUser if some are still
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
        // The calls are internal as the message that made them is (`stamp`):
        // a step of a plan, not its final answer - one made while
        // finalizing is not, as its message is not.
        let internal = turn.in_plan_step();
        // A cancel or stop interrupts the call under way, and no further call
        // is started (issue 161); those left without a result are abandoned
        // as the turn ends. What happens to the turn is the run loop's: it
        // takes the interrupt after this step. A user may interrupt one call
        // alone, running or still to run, and the step goes on (issue 276).
        let cancelled = self.body.control.token();
        let to_run = turn.tool_calls().to_run();
        let mut by_user = self.body.control.begin_calls(to_run.iter().filter_map(|(_, call, _)| call.id.clone()));
        for (index, call, answers) in to_run {
            if cancelled.is_cancelled() {
                return Ok(ActorState::ExecutingTools);
            }
            let mut interrupt = call.id.as_ref().and_then(|id| by_user.remove(id));
            if let Some(by) = interrupt.as_mut().and_then(|interrupt| interrupt.try_recv().ok()) {
                turn.tool_calls().skip(index, &by);
                continue;
            }
            let result = self.tool_executor.execute(&call, answers.as_ref(), turn, interruption(cancelled.clone(), interrupt)).await;
            turn.tool_calls().record(index, result);
        }
        // Once every call has its result, the turn keeps the tool message for
        // the model (issue 83). A tool result is only ever "final" (shown as
        // the real answer) when it's not part of executing an in-progress
        // plan - mirrors the same internal/final rule applied to the
        // assistant message that triggered it.
        if let Some(tool_message) = turn.tool_calls().collect(internal) {
            turn.say(tool_message, output).await?;

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
            Ok(ActorState::WaitingForUser)
        }
    }
}

/// Withdraws the requests of the calls a turn `abandoned` as it ended, in
/// its contexts (`answering`), so that nobody is asked any longer to decide
/// on, or answer for, a call that will not run (issue 148) - the counterpart of
/// `AgentActor::request_approvals`. Every abandoned call that waited for
/// an approval - the approver ignores what it does not hold - and every one
/// that waited for answers, whose questions are closed for those they were
/// shown to (issue 165).
async fn withdraw_requests(answering: &Answering, abandoned: &[ToolCallState], output: &dyn EventSender) -> Result<()> {
    for call in abandoned {
        let withdrawn = match &call.status {
            ToolCallStatus::WaitingApproval { id, .. } => ActorEvent::ApprovalRequestWithdrawn { id: *id },
            ToolCallStatus::WaitingUser { id, questions } => ActorEvent::QuestionClosed { id: *id, shown_to: questions.shown_to() },
            _ => continue,
        };
        answering.send(withdrawn, output).await?;
    }
    Ok(())
}

/// Makes `message` of the model the agent's: its sender, its turn
/// (issue 235) - partial ones too, which the turn never keeps - its labels
/// (`label`), and whether it is internal - part of executing a plan
/// (`Turn::in_plan_step`); the final answer after the last step is
/// not, and must reach the user live. `reasoning_visibility` and
/// `complete` as for `label`.
fn stamp(message: &mut Message, turn: &Turn, reasoning_visibility: Visibility, complete: bool) {
    message.sender_name = turn.agent_name().to_string();
    message.sender_id = ActorId::Agent(turn.agent_id());
    message.turn_id = Some(turn.turn_id());
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
/// knowledge, or if the arguments of a call it leads to are - those of
/// the message once it is `complete`, and while it streams those of every
/// tool the agent could still call, since reasoning comes before the
/// call it leads to and may hold what goes into it, but never what comes
/// back (issue 281). The message carries the turn's label, as
/// everything the agent derives does (issues 133, 168): the join of what
/// the turn has seen, knowledge read from other channels included, at
/// most the channel's classification. The tools are those the turn began
/// with, which the model was offered (issue 137).
fn label(message: &mut Message, turn: &Turn, reasoning_visibility: Visibility, complete: bool) {
    let tools = turn.tools();
    for call in &mut message.tool_calls {
        let declared = tools.iter().find(|t| t.name == call.function_name);
        call.visibility = declared.map(|t| t.visibility).unwrap_or_default();
        call.presentation = declared.map(|t| t.presentation).unwrap_or_default();
    }
    // What the model produces is derived from what the turn has seen: it
    // carries the turn's label, at most the channel's classification as
    // the turn began with it (issue 168).
    message.classification = turn.label();

    let owners_call = if complete {
        message.tool_calls.iter().any(|c| !c.visibility.arguments.is_channel())
    } else {
        tools.iter().any(|t| !t.visibility.arguments.is_channel())
    };
    let owners = !reasoning_visibility.is_channel() || turn.saw_private_knowledge() || owners_call;
    message.reasoning_visibility = if owners { Visibility::Owner } else { Visibility::Channel };
}

#[async_trait]
impl Actor for AgentActor {
    fn id(&self) -> ActorId {
        self.body.id()
    }
    fn name(&self) -> Name {
        self.body.name()
    }

    fn event_filter(&self) -> fn(&Envelope) -> bool {
        |event| {
            matches!(
                event.event,
                ActorEvent::ApprovalGranted(_)
                    | ActorEvent::ApprovalDenied(_)
                    | ActorEvent::ApprovalTimedOut(_)
                    | ActorEvent::QuestionAnswered { .. }
                    | ActorEvent::QuestionDeclined { .. }
                    | ActorEvent::CompleteMessageGenerated(_)
                    | ActorEvent::MessageModified(_)
                    | ActorEvent::MessageRevoked { .. }
                    | ActorEvent::MessageReclassified { .. }
                    | ActorEvent::RetryRequested { .. }
                    | ActorEvent::ResponseRequested
                    | ActorEvent::CommandRequested { .. }
            )
        }
    }

    fn components(&self) -> ComponentConfig {
        self.body.components()
    }

    async fn commands(&self, issuer: &SecurityContext) -> Vec<Command> {
        self.body.visible_commands(issuer).await
    }

    /// Takes in what it missed as what it hears (issues 190, 196): all of
    /// it into its history, what passes its perceive processors into its
    /// transcript.
    async fn catch_up(&self, visible: Vec<Message>) -> Result<()> {
        self.body.catch_up(visible).await
    }

    async fn run(
        &self,
        mut rx: Box<dyn EventReceiver>,
        output: Box<dyn EventSender>,
    ) -> Result<()> {
        let mut state = self
            .body
            .state
            .try_lock()
            .map_err(|_| anyhow!("AgentActor {} already running", self.id()))?;

        info!("AgentActor {} is starting", self.id());
        // Whoever watches already learns where it starts.
        self.body.announce(self.body.control.state(), &*output).await;
        if self.body.control.state().in_turn() {
            info!("AgentActor {} resumes a turn in state {:?}", self.id(), self.body.control.state());
        }
        // Whatever step it resumes at, a turn may wait for approvals: one
        // stored right after asking resumes executing its tools, then waits.
        if self.body.control.state().in_turn() && self.body.control.state() != ActorState::Cancelling {
            if let Some(mut turn) = self.turn(&mut state) {
                if let Err(e) = self.request_pending(&mut turn, &*output).await {
                    warn!("AgentActor {} cannot ask again for the approvals and answers it waits for: {:#}", self.id(), e);
                }
            }
        }

        // The turn given up because its answer was asked for again (issue
        // 235), until a turn begins: the request that is on its way finds
        // it so.
        let mut given_up_for: Option<TurnId> = None;
        loop {
            // Every step announces what the steps before moved it to.
            self.body.announce_state(&*output).await;
            let current_state = self.body.control.state();

            // Idle/WaitingForUser/Sleep may consume no event before the actor's
            // input channel closes (Ok(None)) - that's the only way this loop ends.
            // Every other state always produces a next state.
            let span = agent_span(self.body.agent_id(), &self.body.agent.name(), current_state, state.turn.as_ref().map(|t| &t.work.event_context));

            // Before every step of a turn: a turn that no longer flows into
            // its channel - lowered meanwhile - is given up, whatever it was
            // about to do; nothing of it is run or taken over (issue 141).
            if let Some(under_way) = state.turn.as_ref().filter(|_| current_state.in_turn()) {
                if let Some(channel) = self.lowered(under_way) {
                    info!("Agent {} {} gives up its turn: the channel was lowered to {}", self.id(), self.body.agent.name(), channel);
                    self.give_up(&mut state, GivenUpReason::Lowered { channel }, &*output).instrument(span).await;
                    self.settle(&mut state, ActorState::Idle).await;
                    continue;
                }
            }

            // Between turns the actor works on its state; a step of a turn
            // on the turn under way.
            let result = async { Ok::<_, Error>(match current_state {
                // Carrying out a command is a part of waiting, and ends
                // within it; found here, it was cut short.
                ActorState::Idle | ActorState::Sleeping | ActorState::CarryingOutCommand => self.handle_waiting(&mut state, &mut rx, &mut given_up_for, &*output).await?,
                ActorState::WaitingForUser => {
                    self.handle_waiting_for_user(&mut self.turn_under_way(&mut state)?, &mut rx, &*output).await?
                }
                ActorState::Planning => Some(self.handle_planning(&mut self.turn_under_way(&mut state)?, &*output).await?),
                ActorState::ExecutingTools => Some(self.handle_executing_tools(&mut self.turn_under_way(&mut state)?, &*output).await?),
                ActorState::Busy => Some(self.handle_busy(&mut self.turn_under_way(&mut state)?, &*output).await?),
                ActorState::Cancelling => Some(self.handle_cancelling(&mut state, &*output).await?),
                ActorState::Reflecting => Some(self.handle_reflecting(&mut state, &*output).await?),
            }) }.instrument(span.clone()).await;

            // A failure inside a turn ends the turn, not the agent (issue 02).
            // Errors in the waiting states are the transport's and still end
            // the actor.
            let next_state = match result {
                Ok(next) => next,
                Err(e) if current_state.in_turn() => {
                    self.fail_turn(&mut state, &e, &*output).instrument(span.clone()).await;
                    Some(ActorState::Idle)
                }
                Err(e) => return Err(e),
            };

            // What `cancel` or `stop` asked for is decided here, after the
            // step it interrupted, and nowhere else (issue 152). Told to
            // stop, the actor is leaving the channel: a turn under way is
            // not resumed later, so it is given up rather than stored to be
            // resumed (issues 75, 145), and the channel is told - its
            // farewell, which the channel takes until its task has ended
            // (issue 262).
            let interrupt = self.body.control.take_interrupt();
            if interrupt == Some(Interrupt::Stop) {
                self.give_up(&mut state, GivenUpReason::Stopped, &*output).await;
                self.settle(&mut state, ActorState::Idle).await;
                break;
            }
            // Its answer asked for again (issue 235): the turn is given up,
            // nothing of it having reached the channel's history, and the
            // request that is on its way begins the next one. One that came
            // as the turn ended finds nothing left to give up; the request
            // then finds its turn over.
            if interrupt == Some(Interrupt::Retry) && next_state.is_some_and(|next| next.in_turn()) {
                given_up_for = state.turn.as_ref().map(|t| t.work.turn_id);
                self.give_up(&mut state, GivenUpReason::Retried, &*output).instrument(span.clone()).await;
                self.settle(&mut state, ActorState::Idle).await;
                continue;
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
                self.body.control.go_to(if cancelled { ActorState::Cancelling } else { next });
            }
            self.body.store(&mut state).await;
            // Channel is closed, actor wants to quit
            if next_state.is_none() {
                break;
            }
        }

        info!("AgentActor {} has finished", self.id());
        Ok(())
    }

    fn state(&self) -> ActorState {
        self.body.control.state()
    }

    async fn tool_sets(&self) -> Vec<ToolSet> {
        // As a turn would be offered them now, in the agent's own name:
        // who will ask is not known yet.
        let channel = self.body.runtime.channel_classification();
        self.tool_executor.tool_sets(&self.body.security_context(), &channel).await
    }

    fn wakeup(&self) {
        // Called for every agent on every user message, sleeping or not.
        debug!(
            "Waking up agent {} {}",
            self.id(),
            self.body.agent.name()
        );
        self.body.control.wakeup();
    }

    fn turn_under_way(&self) -> Option<TurnId> {
        self.body.control.turn_under_way()
    }

    fn retry_turn(&self, turn: TurnId) -> bool {
        let hit = self.body.control.retry(turn);
        match hit {
            true => info!("Agent {} {} gives its turn {} up: its answer is asked for again", self.id(), self.body.agent.name(), turn),
            false => debug!("Agent {} {}: turn {} is not under way, nothing to answer again", self.id(), self.body.agent.name(), turn),
        }
        hit
    }

    fn interrupt_tool_call(&self, turn: TurnId, call: &str, by: String) -> bool {
        let hit = self.body.control.interrupt_call(turn, call, by);
        match hit {
            true => info!("Agent {} {}: tool call {} of turn {} is interrupted on its own", self.id(), self.body.agent.name(), call, turn),
            false => debug!("Agent {} {}: tool call {} of turn {} does not run, nothing to interrupt", self.id(), self.body.agent.name(), call, turn),
        }
        hit
    }

    fn cancel(&self) {
        match self.body.control.turn_under_way() {
            Some(turn) => info!("Cancelling agent {} {} in turn {}", self.id(), self.body.agent.name(), turn),
            None => info!("Cancelling agent {} {}: no turn is under way", self.id(), self.body.agent.name()),
        }
        self.body.control.cancel();
    }

    fn stop(&self) {
        info!("Stopping agent {} {}: it left the channel", self.id(), self.body.agent.name());
        self.body.control.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::AgentId;

    use caelumora_model::confidentiality::Confidentiality;
    use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
    use caelumora_model::message::{MessageId, MessageRole};
    use caelumora_model::tool::{Tool, ToolParameters, ToolSet, ToolVisibility};

    fn tool(name: &str, visibility: ToolVisibility) -> Tool {
        Tool {
            name: name.to_string(),
            description: format!("does {name}"),
            parameters: ToolParameters::object(Vec::<(String, ToolParameters)>::new(), Vec::<String>::new()),
            requires_approval: false,
            visibility,
            classification: None,
            presentation: Default::default(),
        }
    }

    fn call(name: &str) -> ToolCall {
        // Labelled as the model can never label a call: `label` must set it.
        ToolCall { id: None, function_name: name.to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::HIDDEN, presentation: Default::default() }
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
        agent_channel_state.begin_turn(Default::default(), Default::default(), channel.clone(), vec![ToolSet::new("tools", "some tools", vec![tool("shown", ToolVisibility::SHOWN), tool("call_only", ToolVisibility::CALL_ONLY)])], Default::default());
        let mut turn = Turn::under_way(agent, &mut agent_channel_state);

        let mut partial = answer(&[]);
        stamp(&mut partial, &turn, Visibility::Channel, false);
        assert_eq!(partial.turn_id, Some(turn.turn_id()), "a partial message too, which the turn never keeps");
        let mut message = answer(&["shown", "call_only", "unknown"]);
        stamp(&mut message, &turn, Visibility::Channel, true);

        assert_eq!((message.sender_id, message.sender_name.as_str()), (ActorId::Agent(agent), "agent"));
        assert_eq!(message.turn_id, Some(turn.turn_id()));
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
        let both = || vec![tool("shown", ToolVisibility::SHOWN), tool("call_only", ToolVisibility::CALL_ONLY), tool("without_result", ToolVisibility::WITHOUT_RESULT)];
        let reasoning = |tools: Vec<Tool>, personality: Visibility, private_knowledge: bool, calls: &[&str], complete: bool| {
            let mut agent_channel_state = AgentChannelState::default();
            agent_channel_state.begin_turn(Default::default(), Default::default(), Classification::CHANNEL_DEFAULT, vec![ToolSet::new("tools", "some tools", tools)], Default::default());
            let mut turn = Turn::under_way(agent, &mut agent_channel_state);
            if private_knowledge {
                turn.record_knowledge(KnowledgeRef { channel: turn.channel_id(), session: turn.session_id(), source: KnowledgeSource::PrivateNotes { agent, digest: None }, classification: None });
            }
            let mut message = answer(calls);
            label(&mut message, &turn, personality, complete);
            message.reasoning_visibility
        };

        assert_eq!(reasoning(both(), Visibility::Channel, false, &["shown"], true), Visibility::Channel, "nothing says otherwise");
        assert_eq!(reasoning(both(), Visibility::Owner, false, &["shown"], true), Visibility::Owner, "the personality says so");
        assert_eq!(reasoning(both(), Visibility::Channel, true, &["shown"], true), Visibility::Owner, "the turn saw private notes");
        assert_eq!(reasoning(both(), Visibility::Channel, false, &["call_only"], true), Visibility::Owner, "it leads to a call whose arguments are the owner's");
        assert_eq!(reasoning(both(), Visibility::Channel, false, &["without_result"], true), Visibility::Channel,
            "a result the owner's alone comes after the reasoning");
        assert_eq!(reasoning(both(), Visibility::Channel, false, &["shown"], false), Visibility::Owner,
            "while it streams, the agent could still call a tool whose arguments are the owner's");
        assert_eq!(reasoning(vec![tool("shown", ToolVisibility::SHOWN), tool("without_result", ToolVisibility::WITHOUT_RESULT)], Visibility::Channel, false, &[], false), Visibility::Channel,
            "but not if it has no such tool");
    }
}
