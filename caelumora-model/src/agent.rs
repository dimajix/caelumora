use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::message::Message;


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTemplate {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
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
    pub status: AgentStatus,
    /// Full message history
    pub history: Vec<Message>,
    /// Possibly preprocessed history used as context
    pub context: Vec<Message>,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            status: AgentStatus::Idle,
            history: Vec::new(),
            context: Vec::new()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentStatus {
    Idle,
    Busy,
    Error,
    Offline,
}

