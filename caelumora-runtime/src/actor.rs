use crate::message::MessageEvent;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::message::{ActorId, Message};
use caelumora_model::session::{ChannelId, SessionId};
use chrono::{DateTime, Utc};
use std::any::Any;
use std::fmt::Debug;
use tokio::sync::mpsc::{Receiver, Sender};
use uuid::Uuid;

use caelumora_model::security::SecurityContext;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventContext {
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    pub channel_name: String,
    pub sender_id: ActorId,
    pub sender_name: String,
    pub event_id: Uuid,
    pub correlation_id: Uuid,
    pub span_id: Uuid,
    pub span_depth: i32,
}

impl EventContext {
    pub fn new(session_id: SessionId, channel_id: ChannelId, channel_name:String, sender_id:ActorId, sender_name:String) -> Self {
        Self {
            session_id,
            channel_id,
            channel_name,
            sender_id,
            sender_name,
            event_id: Uuid::new_v4(),
            correlation_id: Uuid::default(),
            span_id: Uuid::new_v4(),
            span_depth: 0,
        }
    }

    pub fn next(&self, sender_id:ActorId, sender_name:String) -> Self {
        Self {
            session_id: self.session_id,
            channel_id: self.channel_id,
            channel_name: self.channel_name.clone(),
            sender_id,
            sender_name,
            event_id: Uuid::new_v4(),
            correlation_id: self.event_id,
            span_id: self.span_id,
            span_depth: self.span_depth + 1,
        }
    }
}



#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActorEvent {
    ToolExecutionRequested,
    ToolExecutionGranted,
    ToolExecutionDenied,
    ResponseRequested,
    ResponseStarted,
    PartialMessageGenerated(Message),
    CompleteMessageGenerated(Message),
    FullResponseGenerated(Vec<Message>),
    WakeupRequested,
}

impl ActorEvent {
    pub fn from_message_event(event: MessageEvent) -> Self {
        match event {
            MessageEvent::Partial(msg) => {
                ActorEvent::PartialMessageGenerated(msg)
            }
            MessageEvent::Complete(msg) => {
                ActorEvent::CompleteMessageGenerated(msg)
            }
            MessageEvent::EndOfTurn(vec) => {
                ActorEvent::FullResponseGenerated(vec)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub timestamp: DateTime<Utc>,
    pub security: SecurityContext,
    pub context: EventContext,
    pub event: ActorEvent
}


impl Envelope {
    pub fn new(security: SecurityContext, context: EventContext, event: ActorEvent) -> Self {
        Self {
            timestamp: Utc::now(),
            security,
            context,
            event
        }
    }

    pub fn forward(&self) -> Self {
        Self {
            timestamp: Utc::now(),
            security: self.security.clone(),
            context: self.context.clone(),
            event: self.event.clone(),
        }
    }
    pub fn forward_with_response(&self, messages:Vec<Message>) -> Self {
        Self {
            timestamp: Utc::now(),
            security: self.security.clone(),
            context: self.context.clone(),
            event: ActorEvent::FullResponseGenerated(messages)
        }
    }
    pub fn forward_with_event(&self, event:ActorEvent) -> Self {
        Self {
            timestamp: Utc::now(),
            security: self.security.clone(),
            context: self.context.clone(),
            event
        }
    }

    pub fn respond(&self, sender_id:ActorId, sender_name:String, messages:Vec<Message>) -> Self {
        Self {
            timestamp: Utc::now(),
            security: self.security.next(sender_id),
            context: self.context.next(sender_id, sender_name),
            event: ActorEvent::FullResponseGenerated(messages)
        }
    }

    pub fn with_event(mut self, event: ActorEvent) -> Self {
        self.event = event;
        self
    }
    pub fn with_security(mut self, security_context: SecurityContext) -> Self {
        self.security = security_context;
        self
    }
}


#[async_trait]
pub trait Actor : Any + Debug + Send + Sync {
    fn id(&self) -> ActorId;
    fn kind(&self) -> String;
    fn name(&self) -> String;

    /// Returns an ActorSender and a Receiver, which handles filtering and queue management
    fn create_channel(&self) -> (Box<dyn ActorSender>, Receiver<Envelope>);

    async fn run(&self, rx: Receiver<Envelope>, tx: Sender<Envelope>) -> Result<()>;

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

