use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::message::{MessageContext, MessageHistory};
use crate::template::AgentTemplate;


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: Uuid,
    pub template_id: Uuid,
    pub name: String,
}

impl Agent {
    pub fn new(template:AgentTemplate, name:String) -> Self {
        let agent_id = Uuid::new_v4();
        Agent {
            id: agent_id,
            template_id: template.id,
            name,
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
