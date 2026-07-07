use anyhow::{Context, Result};
use async_trait::async_trait;
use caelumora_model::agent::{AgentChannelState, AgentId, AgentState, ProcessingState};
use std::any::Any;
use std::fmt::Debug;
use std::mem;
use tokio::sync::mpsc::{Receiver, Sender};

use caelumora_model::actor::{ActorEvent, ActorId, Envelope};
use caelumora_model::approval::ApprovableToolCall;
use caelumora_model::channel::ChannelId;
use caelumora_model::message::Message;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::SessionId;

#[async_trait]
pub trait Actor: Any + Debug + Send + Sync {
    fn id(&self) -> ActorId;
    fn kind(&self) -> String;
    fn name(&self) -> String;

    /// Returns an ActorSender and a Receiver, which handles filtering and queue management
    fn new_sender(&self, tx: Sender<Envelope>) -> Box<dyn ActorSender>;

    async fn run(&self, rx: Receiver<Envelope>, tx: Sender<Envelope>) -> Result<()>;

    /// Returns the current processing state of the actor
    fn state(&self) -> ProcessingState;

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
    pub agent_name: String,
    pub agent_state: &'a mut AgentState,
    pub agent_channel_state: &'a mut AgentChannelState,
}

impl<'a> TurnContext<'a> {
    pub fn new(agent_id: AgentId, agent_name:String, agent_state: &'a mut AgentState, agent_channel_state: &'a mut AgentChannelState) -> Self {
        Self {
            agent_id,
            agent_name,
            agent_state,
            agent_channel_state,
        }
    }

    pub fn session_id(&self) -> SessionId {
        self.agent_channel_state.turn_event_context.session_id
    }
    pub fn channel_id(&self) -> ChannelId {
        self.agent_channel_state.turn_event_context.channel_id
    }

    pub fn context(&self) -> Vec<Message> {
        self.agent_channel_state.turn_context.clone()
    }
    pub fn messages(&self) -> Vec<Message> {
        self.agent_channel_state.turn_messages.clone()
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

    pub fn take_pending_events(&mut self) -> Vec<Envelope> {
        mem::take(&mut self.agent_channel_state.pending_events)
    }

    pub fn take_tool_calls(&mut self) -> Vec<ApprovableToolCall> {
        mem::take(&mut self.agent_channel_state.pending_tool_calls)
    }

    pub fn security_context(&self) -> &SecurityContext {
        &self.agent_channel_state.turn_security_context
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
