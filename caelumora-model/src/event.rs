use crate::agent::{AgentId, AgentUpdateEvent};
use crate::channel::{ChannelId, ChannelUpdateEvent};
use crate::session::{SessionId, SessionUpdateEvent};
use crate::user::{UserId, UserProfile, UserUpdateEvent};
use crate::security::SecurityAttributes;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GlobalEvent {
    SessionCreated(SessionId),
    SessionUpdated(SessionId, SessionUpdateEvent),
    SessionDeleted(SessionId),
    /// A deleted session was restored (issue 164): it is there again, as it
    /// was, and is started when it is opened.
    SessionRestored(SessionId),
    SessionStarted(SessionId),
    SessionStopped(SessionId),
    ChannelCreated(SessionId, ChannelId),
    ChannelUpdated(SessionId, ChannelId, ChannelUpdateEvent),
    ChannelDeleted(SessionId, ChannelId),
    /// A deleted channel was restored into its session (issue 164).
    ChannelRestored(SessionId, ChannelId),
    ChannelStarted(SessionId, ChannelId),
    ChannelStopped(SessionId, ChannelId),
    /// About an agent itself, which belongs to no session: one of user or
    /// global scope takes part in several (issue 121).
    AgentCreated(AgentId),
    AgentUpdated(AgentId, AgentUpdateEvent),
    AgentDeleted(AgentId),
    /// A deleted agent was restored (issue 164); it takes part in no
    /// channel until it is added again.
    AgentRestored(AgentId),
    AgentChannelJoined(SessionId, ChannelId, AgentId),
    AgentChannelLeft(SessionId, ChannelId, AgentId),
    /// The agent's own state changed - its memory (issue 62). Published with
    /// the agent's attributes once the change is stored; without content,
    /// since the state is private: whoever may see it fetches it.
    AgentStateChanged(AgentId),
    /// The state of an agent in a channel changed - its notes or private
    /// notes (issue 65). Published with the channel's attributes once it is
    /// stored, before the agent announces the end of its turn; without
    /// content, since part of it is private.
    AgentChannelStateChanged(SessionId, ChannelId, AgentId),
    UserChannelJoined(SessionId, ChannelId, UserId),
    UserChannelLeft(SessionId, ChannelId, UserId),
    UserCreated(UserId),
    UserUpdated(UserId, UserUpdateEvent),
    UserDeleted(UserId),
    /// The profile of a user changed (issue 13), as it now stands.
    /// Published with the profile's attributes: only its user, and the
    /// runtime, receive it.
    UserProfileUpdated(UserId, UserProfile),
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
