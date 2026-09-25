use crate::agent::{AgentId, AgentUpdateEvent};
use crate::channel::{ChannelId, ChannelUpdateEvent};
use crate::session::{SessionId, SessionUpdateEvent};
use crate::user::{UserId, UserUpdateEvent};
use crate::security::SecurityAttributes;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GlobalEvent {
    SessionCreated(SessionId),
    SessionUpdated(SessionId, SessionUpdateEvent),
    SessionDeleted(SessionId),
    SessionStarted(SessionId),
    SessionStopped(SessionId),
    ChannelCreated(SessionId, ChannelId),
    ChannelUpdated(SessionId, ChannelId, ChannelUpdateEvent),
    ChannelDeleted(SessionId, ChannelId),
    ChannelStarted(SessionId, ChannelId),
    ChannelStopped(SessionId, ChannelId),
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
    /// Publishes `event` about an entity with these `attributes` - the
    /// session's, channel's, agent's or user's the event is about (issue 39).
    /// They decide who may receive it; they are not part of the event.
    fn publish(&self, event: GlobalEvent, attributes: SecurityAttributes);
}

impl EventPublisher for () {
    fn publish(&self, _event: GlobalEvent, _attributes: SecurityAttributes) {}
}
