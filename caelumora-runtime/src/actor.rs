use crate::message::MessageEvent;
use crate::security::SecurityContext;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::message::{ActorId, Message};
use caelumora_model::session::{ChannelId, SessionId};
use chrono::{DateTime, Utc};
use std::any::Any;
use std::fmt::Debug;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio_stream::StreamExt;
use uuid::Uuid;

#[derive(Debug, Clone)]
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


#[derive(Debug, Clone)]
pub enum ActorEvent {
    ToolExecutionRequested,
    ToolExecutionGranted,
    ToolExecutionDenied,
    ResponseRequested,
    PartialMessageGenerated(Message),
    CompleteMessageGenerated(Message),
    FullResponseGenerated(Vec<Message>),
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

#[derive(Debug, Clone)]
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

    async fn run(&self, rx: Receiver<Envelope>, tx: Sender<Envelope>) -> Result<()>;
}

