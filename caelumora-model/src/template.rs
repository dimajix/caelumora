use crate::agent::{AgentId, PersonalityId};
use crate::channel::{Channel, Participant};
use crate::meta::Scope::{GlobalScope, SessionScope, UserScope};
use crate::meta::{Metadata, Scope};
use crate::session::{Session, SessionContext};
use chrono::Utc;
use derive_more::Display;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::user::UserId;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParticipantId {
    UserId(UserId),
    AgentId(AgentId),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScopeTemplate {
    SessionScope,
    UserScope,
    GroupScope,
    GlobalScope
}

impl ScopeTemplate {
    pub fn instantiate(&self, context:&SessionContext) -> Scope {
        match self {
            ScopeTemplate::SessionScope => {
                SessionScope { session_id: context.id }
            }
            ScopeTemplate::UserScope => {
                UserScope { user_id: context.owner }
            }
            ScopeTemplate::GroupScope => {
                todo!()
            }
            ScopeTemplate::GlobalScope => {
                GlobalScope
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelTemplate {
    pub name: String,
    #[serde(default = "default_moderator")]
    pub moderator: String,
    pub participants: Vec<String>,
}

fn default_moderator() -> String {
    "round_robin".to_string()
}

impl ChannelTemplate {
    pub async fn instantiate(&self, context: &SessionContext, session_participants: &[Participant]) -> anyhow::Result<Channel> {
        let mut participants = Vec::new();

        for name in &self.participants {
            let participant = session_participants.iter().find(|p| {
                match p {
                    Participant::User(u) => u.username == *name,
                    Participant::Agent(a) => a.metadata.name == *name,
                }
            }).cloned().ok_or_else(|| anyhow::anyhow!("Participant {} not found in session", name))?;
            participants.push(participant);
        }

        Ok(Channel::new(
            context.id,
            self.name.clone(),
            self.moderator.clone(),
            participants,
            context.security.clone(),
        ))
    }
}


#[derive(Debug, Clone, Copy, Default, Display, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionTemplateId(pub Uuid);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTemplate {
    pub id: SessionTemplateId,
    pub metadata: Metadata,
    pub scope: ScopeTemplate,
    pub participants: Vec<ParticipantTemplate>,
    pub channels: Vec<ChannelTemplate>,
}

impl SessionTemplate {
    pub async fn instantiate(&self, name:String, context:&SessionContext, mapping: HashMap<String, ParticipantId>) -> anyhow::Result<Session> {
        let now = Utc::now();

        let mut participants = Vec::new();
        for participant in &self.participants {
            participants.push(participant.instantiate(context, &mapping).await?);
        }

        let mut channels = Vec::new();
        for channel in &self.channels {
            channels.push(channel.instantiate(context, &participants).await?)
        }

        let session = Session {
            id: context.id,
            metadata: Metadata {
                name,
                owner: context.owner,
                scope: self.scope.instantiate(context),
                description: self.metadata.description.clone(),
                ..Metadata::default()
            },
            participants,
            channels,
            created_at: now,
            updated_at: now,
            security: context.security.clone(),
        };
        context.entity_store.create_session(&context.security, &session).await?;
        Ok(session)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParticipantTemplate {
    User { name: String },
    Agent {
        name: String,
        scope: ScopeTemplate,
        personality_id: PersonalityId,
    },
}

impl ParticipantTemplate {
    pub async fn instantiate(&self, context:&SessionContext, mapping: &HashMap<String, ParticipantId>) -> anyhow::Result<Participant> {
        let participant = match self {
            ParticipantTemplate::User { name } => {
                let id = mapping.get(name).and_then(|id| {
                    if let ParticipantId::UserId(uid) = id {
                        Some(*uid)
                    } else {
                        None
                    }
                }).ok_or_else(|| anyhow::anyhow!("UserId for {} not found in mapping", name))?;
                let user = context.entity_store.get_user(&context.security, id).await?;
                Participant::User(user)
            },
            ParticipantTemplate::Agent { ref name, ref scope, personality_id } => {
                let agent_id = mapping.get(name).and_then(|id| {
                    if let ParticipantId::AgentId(aid) = id {
                        Some(*aid)
                    } else {
                        None
                    }
                });

                let sc = scope.instantiate(&context);
                let agent = if let Some(aid) = agent_id {
                    context.entity_store.get_agent(&context.security, aid).await?
                } else if let Ok(agent) = context.entity_store.find_agent_by_name(&context.security, name, &sc).await {
                    agent
                } else {
                    let personality = context.entity_store.get_personality(&context.security, personality_id.clone()).await.expect("failed to get personality");
                    let agent = crate::agent::Agent::new(name.clone(), sc, &personality, context.security.clone());
                    context.entity_store.create_agent(&context.security, &agent).await.expect("failed to create agent");
                    agent
                };
                Participant::Agent(agent)
            },
        };
        Ok(participant)
    }
}
