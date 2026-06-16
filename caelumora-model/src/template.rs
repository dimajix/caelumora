use crate::meta::Scope::{GlobalScope, SessionScope, UserScope};
use crate::meta::{Metadata, Scope};
use crate::session::{Channel, Participant, Session, SessionContext};
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
    pub fn instantiate(&self, context: &SessionContext) -> anyhow::Result<Channel> {
        let mut participants = Vec::new();

        for participant in &self.participants {
            participants.push(participant.instantiate(context)?);
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
    pub fn instantiate(&self, name:String, context:&SessionContext) -> anyhow::Result<Session> {
        let now = Utc::now();

        let mut channels = Vec::new();
        for channel in &self.channels {
            channels.push(channel.instantiate(context)?)
        }

        Ok(Session {
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
        })
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
    pub fn instantiate(&self, context:&SessionContext) -> anyhow::Result<Participant> {
        let resolver = &context.resolver;
        let participant = match self {
            ParticipantTemplate::User => {
                Participant::User {}
            },
            ParticipantTemplate::Agent { ref name, ref scope, personality_id } => {
                let sc = scope.instantiate(&context);
                let agent = resolver.get_or_create_agent(name, &sc, personality_id.clone())?;
                Participant::Agent(agent)
            },
        };
        Ok(participant)
    }
}
