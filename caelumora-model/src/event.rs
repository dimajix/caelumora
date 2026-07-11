use crate::agent::{AgentId, AgentUpdateEvent};
use crate::channel::{ChannelId, ChannelUpdateEvent};
use crate::session::{SessionId, SessionUpdateEvent};
use crate::user::{UserId, UserUpdateEvent};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GlobalEvent {
    SessionCreated(SessionId),
    SessionUpdated(SessionId, SessionUpdateEvent),
    SessionDeleted(SessionId),
    ChannelCreated(SessionId, ChannelId),
    ChannelUpdated(SessionId, ChannelId, ChannelUpdateEvent),
    ChannelDeleted(SessionId, ChannelId),
    AgentCreated(SessionId, AgentId),
    AgentUpdated(SessionId, AgentId, AgentUpdateEvent),
    AgentDeleted(SessionId, AgentId),
    AgentChannelJoined(SessionId, ChannelId, AgentId),
    AgentChannelLeft(SessionId, ChannelId, AgentId),
    UserChannelJoined(SessionId, ChannelId, UserId),
    UserChannelLeft(SessionId, ChannelId, UserId),
    UserCreated(UserId),
    UserUpdated(UserId, UserUpdateEvent),
    UserDeleted(UserId),
}

pub trait EventPublisher: Send + Sync + std::fmt::Debug {
    fn publish(&self, event: GlobalEvent);
}

impl EventPublisher for () {
    fn publish(&self, _event: GlobalEvent) {}
}
