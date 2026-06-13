use crate::agent::Agent;
use crate::message::{MessageHistory};
use crate::meta::Metadata;
use chrono::{DateTime, Utc};
use derive_more::Display;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::store::EntityResolver;
use crate::user::UserId;

#[derive(Debug, Clone, Copy, Default, Display, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub Uuid);

pub struct SessionContext {
    pub id: SessionId,
    pub owner: UserId,
    pub resolver: Box<dyn EntityResolver>
}

#[derive(Debug, Clone, Copy, Default, Display, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChannelId(pub Uuid);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    pub name: String,
    pub moderator: String,
    pub participants: Vec<Participant>,
}

impl Channel {
    pub fn new(name: String, moderator: String, participants: Vec<Participant>) -> Self {
        let id = ChannelId(Uuid::new_v4());
        Self {
            id,
            name,
            moderator,
            participants
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub metadata: Metadata,
    pub channels: Vec<Channel>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelState {
    pub channel_id: ChannelId,
    pub history: MessageHistory,
}

impl ChannelState {
    pub fn new(channel_id: ChannelId) -> Self {
        Self {
            channel_id,
            history: MessageHistory::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionState {
    pub channels: std::collections::HashMap<ChannelId, ChannelState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Participant {
    User,
    Agent(Agent)
}

