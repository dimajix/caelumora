use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::message::Message;
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
    pub history: Vec<Message>,
    /// Possibly preprocessed history used as context
    pub context: Vec<Message>,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            history: Vec::new(),
            context: Vec::new()
        }
    }
}
