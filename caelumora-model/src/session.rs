use crate::agent::Agent;
use crate::message::Message;
use crate::meta::Metadata;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::store::EntityResolver;


pub struct SessionContext {
    pub id: Uuid,
    pub owner: Uuid,
    pub resolver: Box<dyn EntityResolver>
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: Uuid,
    pub name: String,
    pub participants: Vec<Participant>,
}

impl Channel {
    pub fn new(name: String, participants: Vec<Participant>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            participants
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: Uuid,
    pub metadata: Metadata,
    pub channels: Vec<Channel>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Full message history
    pub history: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Participant {
    User,
    Agent(Agent)
}

