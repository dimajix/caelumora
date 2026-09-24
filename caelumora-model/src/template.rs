use crate::agent::{AgentId, PersonalityId};
use crate::approval::{ApprovalStrategy, Approver, ApproverId};
use crate::channel::{participant_attributes, Channel, Participant};
use crate::component::ComponentConfig;
use crate::meta::Scope::{GlobalScope, SessionScope, UserScope};
use crate::meta::{Metadata, Scope};
use crate::session::{Session, SessionContext};
use chrono::Utc;
use derive_more::Display;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::actor::ActorId;
use crate::security::{Secured, SecurityAttributes};
use crate::store::StoreError;
use crate::user::UserId;
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("participant '{0}' not found in session")]
    ParticipantNotFound(String),
    #[error("mapping for '{0}' not found")]
    MappingNotFound(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub type TemplateResult<T> = Result<T, TemplateError>;

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
    /// Builds the channel. `session_participants` pairs each participant of
    /// the session with its name in the session template, which is what
    /// `self.participants` refers to - not the user's or agent's own name: the
    /// template's "user" is whoever the session was created for.
    pub async fn instantiate(&self, context: &SessionContext, session_participants: &[(String, Participant)]) -> TemplateResult<Channel> {
        let mut participants = Vec::new();

        for name in &self.participants {
            let participant = if name == "approval" {
                Participant::Approver(Approver {
                    id: ApproverId(Uuid::new_v4()),
                    strategy: ApprovalStrategy::Any,
                })
            } else {
                session_participants.iter()
                    .find(|(template_name, _)| template_name == name)
                    .map(|(_, participant)| participant.clone())
                    .ok_or_else(|| TemplateError::ParticipantNotFound(name.clone()))?
            };
            participants.push(participant);
        }

        // Ensure Approver actor is present
        if !participants.iter().any(|p| matches!(p, Participant::Approver(_))) {
            participants.push(Participant::Approver(Approver {
                id: ApproverId(Uuid::new_v4()),
                strategy: ApprovalStrategy::Any,
            }));
        }

        let security_attributes = participant_attributes(ActorId::User(context.owner), &participants);

        Ok(Channel::new(
            context.id,
            self.name.clone(),
            self.moderator.clone(),
            participants,
            security_attributes
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
    #[serde(default)]
    pub security_attributes: SecurityAttributes,
    /// User commands of sessions made from this template, in addition to the
    /// global ones from `AppConfig` (issue 28) - for commands that belong to
    /// what the session is for, such as an integration it uses.
    #[serde(default)]
    pub commands: ComponentConfig,
}

impl Secured for SessionTemplate {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl SessionTemplate {
    pub async fn instantiate(&self, name:String, context:&SessionContext, mapping: HashMap<String, ParticipantId>) -> TemplateResult<Session> {
        let now = Utc::now();

        let mut named_participants = Vec::new();
        let mut participants = Vec::new();
        for template in &self.participants {
            let participant = template.instantiate(context, &mapping).await?;
            if let Some(name) = template.name() {
                named_participants.push((name.to_string(), participant.clone()));
            }
            participants.push(participant);
        }

        let mut channels = Vec::new();
        for channel in &self.channels {
            channels.push(channel.instantiate(context, &named_participants).await?)
        }

        let security_attributes = participant_attributes(ActorId::User(context.owner), &participants);

        let session = Session {
            id: context.id,
            metadata: Metadata {
                name,
                scope: self.scope.instantiate(context),
                description: self.metadata.description.clone(),
                ..Metadata::default()
            },
            participants,
            channels,
            commands: self.commands.clone(),
            created_at: now,
            updated_at: now,
            security_attributes,
        };
        // NOTE: persistence is intentionally the caller's responsibility (SessionManager), so
        // that session lifecycle events can be emitted at the point of creation. This just
        // builds the in-memory session.
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
    Approver,
}

impl ParticipantTemplate {
    /// The name channels use to refer to this participant, if it has one.
    pub fn name(&self) -> Option<&str> {
        match self {
            ParticipantTemplate::User { name } | ParticipantTemplate::Agent { name, .. } => Some(name),
            ParticipantTemplate::Approver => None,
        }
    }

    pub async fn instantiate(&self, context:&SessionContext, mapping: &HashMap<String, ParticipantId>) -> TemplateResult<Participant> {
        let participant = match self {
            ParticipantTemplate::User { name } => {
                let id = mapping.get(name).and_then(|id| {
                    if let ParticipantId::UserId(uid) = id {
                        Some(*uid)
                    } else {
                        None
                    }
                }).ok_or_else(|| TemplateError::MappingNotFound(name.clone()))?;
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
                    let personality = context.entity_store.get_personality(&context.security, personality_id.clone()).await?;
                    let agent = crate::agent::Agent::new(name.clone(), sc, &personality, context.security.clone());
                    context.entity_store.create_agent(&context.security, &agent).await?;
                    agent
                };
                Participant::Agent(agent)
            },
            ParticipantTemplate::Approver => {
                Participant::Approver(Approver {
                    id: ApproverId(Uuid::new_v4()),
                    strategy: ApprovalStrategy::Any,
                })
            }
        };
        Ok(participant)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecurityContext;
    use crate::store::EntityStore;
    use crate::store_impl::memory::MemoryStore;
    use crate::user::{User, UserId};
    use std::sync::Arc;

    #[tokio::test]
    async fn channels_refer_to_participants_by_their_template_name() {
        let store: Arc<dyn EntityStore> = Arc::new(MemoryStore::new());
        let alice = User {
            id: UserId::new(),
            username: "alice".to_string(),
            email: String::new(),
            roles: Vec::new(),
            security: SecurityContext::default(),
        };
        store.create_user(&SecurityContext::default(), &alice).await.unwrap();

        let template = SessionTemplate {
            security_attributes: Default::default(),
            id: SessionTemplateId(Uuid::new_v4()),
            metadata: Metadata::default(),
            scope: ScopeTemplate::SessionScope,
            participants: vec![ParticipantTemplate::User { name: "user".to_string() }],
            channels: vec![ChannelTemplate {
                name: "main".to_string(),
                moderator: "broadcast".to_string(),
                participants: vec!["user".to_string()],
            }],
            commands: crate::command::default_commands(),
        };
        let context = SessionContext {
            id: Default::default(),
            owner: alice.id,
            security: SecurityContext::default(),
            entity_store: store,
        };
        let mapping = HashMap::from([("user".to_string(), ParticipantId::UserId(alice.id))]);

        let session = template.instantiate("s".to_string(), &context, mapping).await
            .expect("the template's \"user\" is alice, whatever her username");

        let users: Vec<_> = session.channels[0].participants.iter()
            .filter_map(|p| match p { Participant::User(u) => Some(u.id), _ => None })
            .collect();
        assert_eq!(users, vec![alice.id]);
        assert_eq!(session.commands.component_nodes.len(), 2, "the session keeps its template's commands");

        // Sessions stored before sessions had commands still load.
        let mut stored = serde_json::to_value(&session).unwrap();
        stored.as_object_mut().unwrap().remove("commands");
        let loaded: Session = serde_json::from_value(stored).expect("a session without commands loads");
        assert!(loaded.commands.component_nodes.is_empty());
    }
}
