use anyhow::{Context, Result};
use async_trait::async_trait;
use caelumora_model::agent::{AgentChannelState, AgentId, AgentState};
use tracing::warn;
use std::any::Any;
use std::fmt::Debug;
use std::mem;
use std::sync::Arc;

use crate::transport::{EventReceiver, EventSender};
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_model::approval::{ApprovalId, ApprovalStatus};
use caelumora_model::channel::ChannelId;
use caelumora_model::message::{ContextRef, Message, MessageId};
use caelumora_model::agent::SeenContext;
use caelumora_model::plan::{PlanProgress, PlanState};
use caelumora_model::security::SecurityContext;
use caelumora_model::session::SessionId;
use caelumora_model::store::StateStore;
use caelumora_model::tool::ToolCallState;

#[async_trait]
pub trait Actor: Any + Debug + Send + Sync {
    fn id(&self) -> ActorId;
    fn kind(&self) -> String;
    fn name(&self) -> String;

    /// Returns the predicate that declares which events this actor wants to receive.
    /// The ChannelLoop wraps the actor's input sender in a `FilteringEventSender` using
    /// this predicate, so events that don't match are silently dropped at dispatch time.
    fn event_filter(&self) -> fn(&Envelope) -> bool;

    async fn run(
        &self,
        rx: Box<dyn EventReceiver>,
        tx: Box<dyn EventSender>,
    ) -> Result<()>;

    /// Returns the current processing state of the actor.
    fn state(&self) -> ActorState;

    /// Wakes up the actor from a Sleep state.
    async fn wakeup(&self) -> Result<()> {
        Ok(())
    }

    async fn cancel(&self) -> Result<()> {
        Ok(())
    }

    /// What this actor's model saw when it produced the message `message`:
    /// its context and system prompt (issue 06), on behalf of `caller`.
    /// Only agents have one; every other actor answers `NotRecorded`.
    ///
    /// # Errors
    /// `MessageContextError::Forbidden` if `caller` may not read it,
    /// `NotRecorded` if this actor did not produce the message,
    /// `Unavailable` if what was recorded cannot be resolved any more.
    async fn message_context(&self, _caller: &SecurityContext, message: MessageId) -> Result<SeenContext, MessageContextError> {
        Err(MessageContextError::NotRecorded { actor: self.id(), message })
    }
}

/// Why a message's context cannot be shown. The messages are meant for users
/// (see `crate::error::user_message`).
#[derive(Debug, thiserror::Error)]
pub enum MessageContextError {
    #[error("You are not permitted to see what {actor} saw")]
    Forbidden { actor: ActorId },
    #[error("No context is recorded for message {message} by {actor}")]
    NotRecorded { actor: ActorId, message: MessageId },
    #[error("The context of message {message} by {actor} is no longer available")]
    Unavailable { actor: ActorId, message: MessageId },
    /// Reading the actor's state failed; the cause is in the log.
    #[error("The context of message {message} by {actor} cannot be read")]
    Failed { actor: ActorId, message: MessageId },
}


#[derive(Debug)]
pub struct TurnContext<'a> {
    pub agent_id: AgentId,
    pub channel_id: ChannelId,
    pub session_id: SessionId,
    pub agent_name: String,
    agent_state: &'a mut AgentState,
    agent_channel_state: &'a mut AgentChannelState,
}

impl<'a> TurnContext<'a> {
    pub fn new(agent_id: AgentId, agent_name:String, agent_state: &'a mut AgentState, agent_channel_state: &'a mut AgentChannelState) -> Self {
        Self {
            agent_id,
            channel_id: agent_channel_state.turn_event_context.channel_id,
            session_id: agent_channel_state.turn_event_context.session_id,
            agent_name,
            agent_state,
            agent_channel_state,
        }
    }

    pub fn next_turn(&mut self, event_context: EventContext, security_context: SecurityContext) {
        self.agent_channel_state.turn_scratch.clear();
        self.agent_channel_state.turn_context.clear();
        self.agent_channel_state.turn_messages.clear();
        self.agent_channel_state.requested_state = None;
        self.agent_channel_state.plan = None;
        self.agent_channel_state.turn_system_prompt = None;
        self.agent_channel_state.turn_event_context = event_context;
        self.agent_channel_state.turn_security_context = security_context;
    }

    /// The event context of the current turn: carries the ids (`span_id`,
    /// `correlation_id`) that tie the turn to the envelope that started it.
    pub fn event_context(&self) -> &EventContext {
        &self.agent_channel_state.turn_event_context
    }

    pub fn finish_turn(&mut self) {
        self.agent_channel_state.turn_scratch.clear();
        self.agent_channel_state.turn_context.clear();
        self.agent_channel_state.turn_messages.clear();
    }

    pub fn context(&self) -> &Vec<Message> {
        &self.agent_channel_state.turn_context
    }
    pub fn messages(&self) -> &Vec<Message> {
        &self.agent_channel_state.turn_messages
    }

    pub fn last_message(&self) -> Option<&Message> {
        self.agent_channel_state.turn_messages.last()
    }

    pub fn full_context(&self) -> Vec<Message> {
        let mut context = self.agent_channel_state.turn_context.clone();
        context.extend_from_slice(self.agent_channel_state.turn_messages.as_slice());
        context
    }

    pub fn set_context(&mut self, context: Vec<Message>) {
        self.agent_channel_state.turn_context = context;
    }

    pub fn push_message(&mut self, message: Message) {
        self.agent_channel_state.turn_messages.push(message);
    }

    pub fn set_tool_calls(&mut self, tool_calls: Vec<ToolCallState>) {
        self.agent_channel_state.tool_calls = tool_calls;
    }
    pub fn set_tool_approval_status(&mut self, id:ApprovalId, status: ApprovalStatus) -> bool {
        if let Some(tool_call) = self.agent_channel_state.tool_calls.iter_mut().find(|tc| tc.id == id) {
            tool_call.status = status;
            true
        }
        else {
            false
        }
    }
    pub fn tool_calls(&self) -> &Vec<ToolCallState> {
        &self.agent_channel_state.tool_calls
    }
    pub fn take_tool_calls(&mut self) -> Vec<ToolCallState> {
        mem::take(&mut self.agent_channel_state.tool_calls)
    }
    pub fn has_tool_calls(&self) -> bool {
        !self.agent_channel_state.tool_calls.is_empty()
    }

    pub fn set_plan(&mut self, plan: PlanState) {
        self.agent_channel_state.plan = Some(plan);
    }
    pub fn plan(&self) -> Option<&PlanState> {
        self.agent_channel_state.plan.as_ref()
    }
    pub fn take_plan(&mut self) -> Option<PlanState> {
        self.agent_channel_state.plan.take()
    }
    /// Whether a plan is currently in play. A `PlanState` is only ever
    /// constructed after its `create_plan` tool call has already been
    /// through the generic tool-call approval gate, so its mere presence
    /// implies "approved and executing".
    pub fn has_plan(&self) -> bool {
        self.agent_channel_state.plan.is_some()
    }
    pub fn set_plan_progress(&mut self, progress: PlanProgress) {
        if let Some(plan) = self.agent_channel_state.plan.as_mut() {
            plan.progress = progress;
        }
    }

    pub fn security_context(&self) -> &SecurityContext {
        &self.agent_channel_state.turn_security_context
    }

    pub fn state_context(&self) -> Vec<Message> {
        self.agent_channel_state.context.messages()
    }
    pub fn extend_state_context(&mut self, messages: Vec<Message>) {
        self.agent_channel_state.context.extend(messages);
    }
    pub fn extend_state_history(&mut self, messages: Vec<Message>) {
        self.agent_channel_state.history.extend(messages);
    }

    /// Records `prompt` as this turn's system prompt: kept once per digest,
    /// so that what the agent saw can be told later (issue 06).
    pub fn set_system_prompt(&mut self, prompt: &str) {
        let digest = caelumora_model::digest::system_prompt(prompt);
        self.agent_channel_state.system_prompts.entry(digest).or_insert_with(|| prompt.to_string());
        self.agent_channel_state.turn_system_prompt = Some(digest);
    }

    /// Appends the messages this agent produced in the turn to its context,
    /// and those that are not internal to its history - each with what it was
    /// produced from: the context before it and the turn's system prompt
    /// (issue 06).
    pub fn record_turn(&mut self, messages: Vec<Message>) {
        let before = self.agent_channel_state.context.extend(messages.clone());
        let system_prompt = self.agent_channel_state.turn_system_prompt;
        for (message, context) in messages.into_iter().zip(before) {
            if !message.internal {
                self.agent_channel_state.history.push(message, Some(ContextRef { context, system_prompt }));
            }
        }
    }

    /// Records what state the turn should land on once `Reflecting` finishes,
    /// overriding the default `Idle`. This does not touch the live `ActorState`
    /// atomic directly — only the run loop does that; this is purely a same-turn
    /// handoff (survives across separate `TurnContext` borrows because it lives
    /// in `AgentChannelState`, not in `TurnContext` itself).
    pub fn request_state(&mut self, state: ActorState) {
        self.agent_channel_state.requested_state = Some(state);
    }

    /// Peek at a previously requested end-of-turn state without consuming it.
    pub fn requested_state(&self) -> Option<ActorState> {
        self.agent_channel_state.requested_state
    }

    /// Consume the requested end-of-turn state. Used once, by `Reflecting`.
    pub fn take_requested_state(&mut self) -> Option<ActorState> {
        self.agent_channel_state.requested_state.take()
    }

    pub async fn store_agent_state(&self, state_store: &Arc<dyn StateStore>) {
        if let Err(e) = state_store
            .put_agent_state(self.agent_id, self.agent_state)
            .await
        {
            warn!(
                "Cannot store agent state of agent {}: {}", self.agent_id, e
            )
        }
    }
    pub async fn store_channel_state(&self, state_store: &Arc<dyn StateStore>) {
        if let Err(e) = state_store
            .put_agent_channel_state(self.agent_id, self.channel_id, self.agent_channel_state)
            .await
        {
            warn!(
                "Cannot store agent channel state of agent {}: {}", self.agent_id, e
            )
        }
    }

    pub async fn send_event(&self, av: ActorEvent, output: &dyn EventSender) -> Result<()> {
        let ev = Envelope::new(
            self.agent_channel_state.turn_security_context.clone(),
            self.agent_channel_state.turn_event_context.clone(),
            av,
        );
        output
            .send(ev)
            .await
            .context(format!("Actor {} failed to send event", self.agent_id))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let mut agent_state = AgentState::default();
        let mut agent_channel_state = AgentChannelState::default();
        let mut ctx = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);

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
        let mut agent_state = AgentState::default();
        let mut agent_channel_state = AgentChannelState::default();
        let mut ctx = TurnContext::new(AgentId::default(), String::new(), &mut agent_state, &mut agent_channel_state);

        ctx.set_plan(make_plan_state());
        assert!(ctx.plan().is_some());

        ctx.next_turn(EventContext::default(), SecurityContext::default());
        assert!(ctx.plan().is_none());
    }

    #[test]
    fn what_the_agent_saw_can_be_told_for_each_message_it_produced() {
        use caelumora_model::agent::{AgentChannelState, AgentId, AgentState};
        use caelumora_model::message::{MessageId, MessageRole};

        let text = |role: MessageRole, content: &str| Message { id: MessageId::new(), role, content: Some(content.to_string()), ..Message::default() };
        let mut agent_state = AgentState::default();
        let mut state = AgentChannelState::default();
        let question = text(MessageRole::User, "What is 2+2?");
        state.context.push(question.clone());

        let (first, second) = (text(MessageRole::Assistant, "Let me think."), text(MessageRole::Assistant, "4"));
        {
            let mut turn = TurnContext::new(AgentId::new(), "agent".to_string(), &mut agent_state, &mut state);
            turn.set_system_prompt("You are good at sums.");
            turn.record_turn(vec![first.clone(), second.clone()]);
        }

        let seen = state.seen_context_of(second.id).expect("the agent produced it");
        assert_eq!(seen.system_prompt.as_deref(), Some("You are good at sums."));
        assert_eq!(seen.messages.iter().map(|m| m.id).collect::<Vec<_>>(), vec![question.id, first.id], "everything before it, its own earlier message included");
        assert!(state.seen_context_of(question.id).is_none(), "not produced by the agent");

        // The same prompt in another turn is kept once.
        {
            let mut turn = TurnContext::new(AgentId::new(), "agent".to_string(), &mut agent_state, &mut state);
            turn.set_system_prompt("You are good at sums.");
        }
        assert_eq!(state.system_prompts.len(), 1);
    }
}
