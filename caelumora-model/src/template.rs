use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTemplate {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTemplate {
    pub name: String,
    pub participants: Vec<ParticipantTemplate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticipantTemplate {
    pub name: String,
    pub kind: ParticipantKindTemplate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParticipantKindTemplate {
    User,
    Agent {
        agent_id: Uuid
    },
    AgentTemplate {
        template_id: Uuid,
    },
}

#[derive(Debug, Default)]
pub struct TemplateManager {
    session_templates: HashMap<String, SessionTemplate>,
    agent_templates: HashMap<String, AgentTemplate>,
}

impl TemplateManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_session_template(&mut self, template: SessionTemplate) {
        self.session_templates.insert(template.name.clone(), template);
    }

    pub fn find_session_template(&self, name: &str) -> Option<&SessionTemplate> {
        self.session_templates.get(name)
    }

    pub fn add_agent_template(&mut self, template: AgentTemplate) {
        self.agent_templates.insert(template.name.clone(), template);
    }

    pub fn find_agent_template(&self, name: &str) -> Option<&AgentTemplate> {
        self.agent_templates.get(name)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manages_session_templates() {
        let mut manager = TemplateManager::new();
        let template = SessionTemplate {
            name: "pairing".to_string(),
            participants: vec![ParticipantTemplate {
                name: "user".to_string(),
                kind: ParticipantKindTemplate::User,
            }],
        };

        manager.add_session_template(template);

        let found = manager.find_session_template("pairing").unwrap();
        assert_eq!(found.name, "pairing");
        assert_eq!(found.participants.len(), 1);
    }

    #[test]
    fn manages_agent_templates() {
        let mut manager = TemplateManager::new();
        let id = Uuid::new_v4();
        let template = AgentTemplate {
            id: id.clone(),
            name: "assistant".to_string(),
            description: "Helpful assistant".to_string(),
            capabilities: vec!["summarize".to_string()],
        };

        manager.add_agent_template(template);

        let found = manager.find_agent_template("assistant").unwrap();
        assert_eq!(found.name, "assistant");
    }
}
