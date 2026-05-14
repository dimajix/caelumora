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
pub struct Session {
    pub id: Uuid,
    pub metadata: Metadata,
    pub participants: Vec<Participant>,
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

