use crate::meta::Scope::{GlobalScope, SessionScope, UserScope};
use crate::meta::{Metadata, Scope};
use crate::channel::{Channel, Participant};
use crate::session::{Session, SessionContext};
use chrono::Utc;
use derive_more::Display;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::agent::PersonalityId;

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
    pub participants: Vec<ParticipantTemplate>,
}

fn default_moderator() -> String {
    "round_robin".to_string()
}

impl ChannelTemplate {
    pub async fn instantiate(&self, context: &SessionContext) -> anyhow::Result<Channel> {
        let mut participants = Vec::new();

        for participant in &self.participants {
            participants.push(participant.instantiate(context).await?);
        }

        Ok(Channel::new(
            self.name.clone(),
            self.moderator.clone(),
            participants,
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
    pub channels: Vec<ChannelTemplate>,
}

impl SessionTemplate {
    pub async fn instantiate(&self, name:String, context:&SessionContext) -> anyhow::Result<Session> {
        let now = Utc::now();

        let mut channels = Vec::new();
        for channel in &self.channels {
            channels.push(channel.instantiate(context).await?)
        }

        let session = Session {
            id: context.id,
            metadata: Metadata {
                name,
                scope: self.scope.instantiate(context),
                description: self.metadata.description.clone(),
                ..Metadata::default()
            },
            channels,
            created_at: now,
            updated_at: now,
        };
        context.state_store.create_session(&context.security, &session).await.expect("failed to create session");
        Ok(session)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParticipantTemplate {
    User,
    Agent {
        name: String,
        scope: ScopeTemplate,
        personality_id: PersonalityId,
    },
}

impl ParticipantTemplate {
    pub async fn instantiate(&self, context:&SessionContext) -> anyhow::Result<Participant> {
        let participant = match self {
            ParticipantTemplate::User => {
                Participant::User {}
            },
            ParticipantTemplate::Agent { ref name, ref scope, personality_id } => {
                let sc = scope.instantiate(&context);
                let agent = if let Some(agent) = context.state_store.find_agent_by_name(&context.security, name, &sc).await.expect("failed to find agent") {
                    agent
                } else {
                    let personality = context.template_store.get_personality(&context.security, personality_id.clone()).await.expect("failed to get personality");
                    let agent = crate::agent::Agent::new(name.clone(), sc, &personality);
                    context.state_store.create_agent(&context.security, &agent).await.expect("failed to create agent");
                    agent
                };
                Participant::Agent(agent)
            },
        };
        Ok(participant)
    }
}
