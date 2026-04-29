use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::agent::AgentState;
use crate::message::Message;

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
    pub fn new(template: &SessionTemplate) -> anyhow::Result<Self> {
        let now = Utc::now();
        let session_id = Uuid::new_v4();
        let mut participants = Vec::new();

        for participant in &template.participants {
            participants.push(Participant::new(participant));
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
pub struct Participant {
    pub id: Uuid,
    pub name: String,
    pub kind: ParticipantKind,
}

impl Participant {
    pub fn new(template: &ParticipantTemplate) -> Self {
        let participant_id = Uuid::new_v4();
        let kind = match template.kind {
            ParticipantKindTemplate::User => ParticipantKind::User {},
            ParticipantKindTemplate::AgentTemplate { template_id: agent_id } => ParticipantKind::Agent {
                agent_id,
                state: AgentState::default()
            }
        };
        Participant {
            id: participant_id,
            name: template.name.clone(),
            kind
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParticipantKind {
    User,
    Agent {
        agent_id: Uuid,
        state: AgentState
    },
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
    AgentTemplate {
        template_id: Uuid,
    },
}
