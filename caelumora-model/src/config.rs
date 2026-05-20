use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;
use crate::agent::Personality;
use crate::template::{ParticipantTemplate, SessionTemplate};

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AgentTemplatesDocument {
    List(Vec<Personality>),
    Wrapped { agent_templates: Vec<Personality> },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SessionTemplatesDocument {
    List(Vec<SessionTemplate>),
    Wrapped { session_templates: Vec<SessionTemplate> },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ParticipantTemplatesDocument {
    List(Vec<ParticipantTemplate>),
    Wrapped {
        participant_templates: Vec<ParticipantTemplate>,
    },
}

pub fn parse_agent_templates_yaml(content: &str) -> Result<Vec<Personality>> {
    let document: AgentTemplatesDocument = serde_yaml::from_str(content)
        .context("Failed to parse agent templates YAML")?;

    Ok(match document {
        AgentTemplatesDocument::List(agent_templates) => agent_templates,
        AgentTemplatesDocument::Wrapped { agent_templates } => agent_templates,
    })
}

pub fn parse_session_templates_yaml(content: &str) -> Result<Vec<SessionTemplate>> {
    let document: SessionTemplatesDocument = serde_yaml::from_str(content)
        .context("Failed to parse session templates YAML")?;

    Ok(match document {
        SessionTemplatesDocument::List(session_templates) => session_templates,
        SessionTemplatesDocument::Wrapped { session_templates } => session_templates,
    })
}

pub fn parse_participant_templates_yaml(content: &str) -> Result<Vec<ParticipantTemplate>> {
    let document: ParticipantTemplatesDocument = serde_yaml::from_str(content)
        .context("Failed to parse participant templates YAML")?;

    Ok(match document {
        ParticipantTemplatesDocument::List(participant_templates) => participant_templates,
        ParticipantTemplatesDocument::Wrapped {
            participant_templates,
        } => participant_templates,
    })
}

pub fn load_agent_templates_yaml(path: impl AsRef<Path>) -> Result<Vec<Personality>> {
    let path_ref = path.as_ref();
    let content = fs::read_to_string(path_ref).with_context(|| {
        format!(
            "Failed to read agent templates YAML file: {}",
            path_ref.display()
        )
    })?;
    parse_agent_templates_yaml(&content)
}

pub fn load_session_templates_yaml(path: impl AsRef<Path>) -> Result<Vec<SessionTemplate>> {
    let path_ref = path.as_ref();
    let content = fs::read_to_string(path_ref).with_context(|| {
        format!(
            "Failed to read session templates YAML file: {}",
            path_ref.display()
        )
    })?;
    parse_session_templates_yaml(&content)
}

pub fn load_participant_templates_yaml(
    path: impl AsRef<Path>,
) -> Result<Vec<ParticipantTemplate>> {
    let path_ref = path.as_ref();
    let content = fs::read_to_string(path_ref).with_context(|| {
        format!(
            "Failed to read participant templates YAML file: {}",
            path_ref.display()
        )
    })?;
    parse_participant_templates_yaml(&content)
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[test]
    fn parses_wrapped_agent_templates() {
        let agent_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        let yaml = format!(
            "agent_templates:\n  - id: {agent_id}\n    metadata:\n      name: assistant\n      scope: GlobalScope\n      owner: {owner_id}\n      description: Helper\n"
        );

        let templates = parse_agent_templates_yaml(&yaml).unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].metadata.name, "assistant");
    }

    #[test]
    fn parses_wrapped_session_templates() {
        let session_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        let yaml = format!(
            "session_templates:\n  - id: {session_id}\n    metadata:\n      name: ops\n      scope: GlobalScope\n      owner: {owner_id}\n      description: Ops flow\n    scope: GlobalScope\n    channels:\n      - name: general\n        participants:\n          - User\n"
        );

        let sessions = parse_session_templates_yaml(&yaml).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].metadata.name, "ops");
        assert_eq!(sessions[0].channels.len(), 1);
    }

    #[test]
    fn parses_wrapped_participant_templates() {
        let yaml = "participant_templates:\n  - User\n";

        let participants = parse_participant_templates_yaml(yaml).unwrap();
        assert_eq!(participants.len(), 1);
    }

    #[test]
    fn fails_on_invalid_yaml_shape() {
        let yaml = "agent_templates:\n  name: invalid\n";

        let result = parse_agent_templates_yaml(yaml);
        assert!(result.is_err());
    }
}