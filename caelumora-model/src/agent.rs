use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::message::{MessageContext, MessageHistory};
use crate::meta::{Metadata, Scope};


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Personality {
    pub id: Uuid,
    pub metadata: Metadata,
    // ContextFilters
    // Tools
    // Memory
    // Knowledge
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: Uuid,
    pub metadata: Metadata,
    pub personality_id: Uuid,
}

impl Agent {
    pub fn new(name:String, scope:Scope, personality: &Personality) -> Self {
        let agent_id = Uuid::new_v4();
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
