use derive_more::Display;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::message::{MessageContext, MessageHistory};
use crate::meta::{Metadata, Scope};
use crate::component::ComponentConfig;

#[derive(Debug, Clone, Copy, Default, Display, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PersonalityId(pub Uuid);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Personality {
    pub id: PersonalityId,
    pub metadata: Metadata,
    #[serde(default)]
    pub components: ComponentConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessingState {
    Idle,
    Active,
    Cancelling,
}

impl Default for ProcessingState {
    fn default() -> Self {
        Self::Idle
    }
}

impl From<u8> for ProcessingState {
    fn from(v: u8) -> Self {
        match v {
            0 => ProcessingState::Idle,
            1 => ProcessingState::Active,
            2 => ProcessingState::Cancelling,
            _ => ProcessingState::Idle,
        }
    }
}

impl From<ProcessingState> for u8 {
    fn from(s: ProcessingState) -> Self {
        match s {
            ProcessingState::Idle => 0,
            ProcessingState::Active => 1,
            ProcessingState::Cancelling => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Display, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentId(pub Uuid);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: AgentId,
    pub metadata: Metadata,
    pub personality_id: PersonalityId,
}

impl Agent {
    pub fn new(name:String, scope:Scope, personality: &Personality) -> Self {
        let agent_id = AgentId(Uuid::new_v4());
        Agent {
            id: agent_id,
            personality_id: personality.id,
            metadata: Metadata {
                name,
                scope,
                description: personality.metadata.description.clone(),
                ..Metadata::default()
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    /// Full message history
    pub history: MessageHistory,
    /// Possibly preprocessed history used as context
    pub context: MessageContext,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            history: MessageHistory::new(),
            context: MessageContext::new(),
        }
    }
}
