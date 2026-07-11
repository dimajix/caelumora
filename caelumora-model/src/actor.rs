use crate::agent::AgentId;
use crate::approval::{ApprovalRequest, ApprovalResponse};
use crate::channel::ChannelId;
use crate::message::{Message, MessageEvent};
use crate::security::SecurityContext;
use crate::session::SessionId;
use crate::user::UserId;
use chrono::{DateTime, Utc};
use derive_more::Display;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, Display, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActorId {
    #[default]
    None,
    User(UserId),
    Agent(AgentId),
    Approver(Uuid),
}
impl ActorId {
    pub fn id(&self) -> Uuid {
        match &self {
            ActorId::None => {Uuid::nil()}
            ActorId::User(id) => {id.0}
            ActorId::Agent(id) => {id.0}
            ActorId::Approver(id) => {*id}
        }
    }

    pub fn as_agent_id(&self) -> Option<AgentId> {
        match self {
            ActorId::Agent(id) => Some(*id),
            _ => None,
        }
    }

    pub fn as_user_id(&self) -> Option<UserId> {
        match self {
            ActorId::User(id) => Some(*id),
            _ => None,
        }
    }
}



#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActorEvent {
    ToolExecutionRequested(ApprovalRequest),
    ToolExecutionGranted(ApprovalResponse),
    ToolExecutionDenied(ApprovalResponse),
    ToolExecutionRequestTimedOut(ApprovalResponse),
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventContext {
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    pub sender_id: ActorId,
    pub sender_name: String,
    pub event_id: Uuid,
    pub correlation_id: Uuid,
    pub span_id: Uuid,
    pub span_depth: i32,
}

impl EventContext {
    pub fn new(session_id: SessionId, channel_id: ChannelId, sender_id:ActorId, sender_name:String) -> Self {
        Self {
            session_id,
            channel_id,
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
