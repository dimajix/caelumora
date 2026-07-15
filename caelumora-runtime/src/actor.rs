use anyhow::{Context, Result};
use async_trait::async_trait;
use caelumora_model::agent::{AgentChannelState, AgentId, AgentState};
use log::warn;
use std::any::Any;
use std::fmt::Debug;
use std::mem;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, Sender};

use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_model::approval::{ApprovalId, ApprovalStatus};
use caelumora_model::channel::ChannelId;
use caelumora_model::message::Message;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::SessionId;
use caelumora_model::store::StateStore;
use caelumora_model::tool::ToolCallState;

#[async_trait]
pub trait Actor: Any + Debug + Send + Sync {
    fn id(&self) -> ActorId;
    fn kind(&self) -> String;
    fn name(&self) -> String;

    /// Returns an ActorSender and a Receiver, which handles filtering and queue management
    fn new_sender(&self, tx: Sender<Envelope>) -> Box<dyn ActorSender>;

    async fn run(&self, rx: Receiver<Envelope>, tx: Sender<Envelope>) -> Result<()>;

    /// Returns the current processing state of the actor
    fn state(&self) -> ActorState;

    /// Wakes up the actor from a Stopped state
    async fn wakeup(&self) -> Result<()> {
        Ok(())
    }

    async fn cancel(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
pub enum SendError {
    Full,
    Closed,
    Filtered,
    Other(String),
}

impl From<tokio::sync::mpsc::error::TrySendError<Envelope>> for SendError {
    fn from(e: tokio::sync::mpsc::error::TrySendError<Envelope>) -> Self {
        match e {
            tokio::sync::mpsc::error::TrySendError::Full(_) => SendError::Full,
            tokio::sync::mpsc::error::TrySendError::Closed(_) => SendError::Closed,
        }
    }
}

pub trait ActorSender: Send + Sync {
    /// Tries to send an event to the actor.
    /// This method implements filtering and potentially queue compression/eviction.
    fn try_send(&self, event: Envelope) -> Result<(), SendError>;
}

pub struct StandardActorSender {
    pub tx: Sender<Envelope>,
    pub filter: Box<dyn Fn(&Envelope) -> bool + Send + Sync>,
}

impl ActorSender for StandardActorSender {
    fn try_send(&self, event: Envelope) -> Result<(), SendError> {
        // 1. Filtering logic
        if !(self.filter)(&event) {
            return Err(SendError::Filtered);
        }

        // 2. Queue management logic
        self.tx.try_send(event).map_err(SendError::from)
    }
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
        self.agent_channel_state.turn_event_context = event_context;
        self.agent_channel_state.turn_security_context = security_context;
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

    pub fn push_pending_event(&mut self, env: Envelope) {
        self.agent_channel_state.pending_events.push(env);
    }
    pub fn take_pending_events(&mut self) -> Vec<Envelope> {
        mem::take(&mut self.agent_channel_state.pending_events)
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

    pub fn set_processing_state(&self, state: ActorState) {
        self.agent_channel_state.processing_state.store(state);
    }

    pub async fn store_agent_state(&self, state_store: &Arc<dyn StateStore>) {
        if let Err(_e) = state_store
            .put_agent_state(self.agent_id, self.agent_state)
            .await
        {
            warn!(
                "Cannot store agent state of agent {}", self.agent_id
            )
        }
    }
    pub async fn store_channel_state(&self, state_store: &Arc<dyn StateStore>) {
        if let Err(_e) = state_store
            .put_agent_channel_state(self.agent_id, self.channel_id, self.agent_channel_state)
            .await
        {
            warn!(
                "Cannot store agent channel state of agent {}", self.agent_id
            )
        }
    }

    pub async fn send_event(&self, av:ActorEvent, output: &Sender<Envelope>) -> Result<()> {
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
