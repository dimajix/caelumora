use crate::agent::Agent;
use crate::message::Message;
use crate::template::{ParticipantKindTemplate, ParticipantTemplate, SessionTemplate};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::store::EntityResolver;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: Uuid,
    pub name: String,
    pub participants: Vec<Participant>,
    pub history: Vec<Message>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Session {
    pub fn new(template: &SessionTemplate, resolver: &dyn EntityResolver) -> anyhow::Result<Self> {
        let now = Utc::now();
        let session_id = Uuid::new_v4();
        let mut participants = Vec::new();

        for participant in &template.participants {
            participants.push(Participant::new(participant, resolver)?);
        }

        Ok(Self {
            id: session_id,
            name: template.name.clone(),
            participants,
            history: Vec::new(),
            created_at: now,
            updated_at: now,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Full message history
    pub history: Vec<Message>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Participant {
    pub id: Uuid,
    pub name: String,
    pub kind: ParticipantKind,
}

impl Participant {
    pub fn new(template: &ParticipantTemplate, resolver: &dyn EntityResolver) -> anyhow::Result<Self> {
        let participant_id = Uuid::new_v4();
        let kind = match template.kind {
            ParticipantKindTemplate::User => ParticipantKind::User {},
            ParticipantKindTemplate::Agent { agent_id } => {
                let agent = resolver.get_agent(agent_id)?;
                ParticipantKind::Agent(
                    agent
                )
            },
            ParticipantKindTemplate::AgentTemplate { template_id, ref agent_name } => {
                let template = resolver.get_agent_template(template_id)?;
                let agent = Agent::new(template, agent_name.clone());
                ParticipantKind::Agent(
                    agent
                )
            }
        };
        Ok(Participant {
            id: participant_id,
            name: template.name.clone(),
            kind
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParticipantKind {
    User,
    Agent(Agent)
}

