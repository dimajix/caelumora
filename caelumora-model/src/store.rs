use async_trait::async_trait;
use std::fmt::Debug;

use crate::agent::{Agent, AgentChannelState, AgentId, AgentState, Personality, PersonalityId};
use crate::channel::{Channel, ChannelId, ChannelState};
use crate::component::ComponentDescriptor;
use crate::meta::Scope;
use crate::security::SecurityContext;
use crate::session::{Session, SessionId, SessionState};
use crate::template::{SessionTemplate, SessionTemplateId};
use crate::user::{User, UserId};


#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("not found: {entity} {id}")]
    NotFound { entity: &'static str, id: String },
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("validation: {0}")]
    Validation(String),
    #[error("backend: {0}")]
    Backend(String),
    #[error("permission denied")]
    PermissionDenied,
}

pub type StoreResult<T> = Result<T, StoreError>;


#[async_trait]
pub trait EntityStore : Debug + Sync + Send {
    // Personalities (formerly in TemplateStore)
    async fn get_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<Personality>;
    async fn find_personality_by_name(&self, ctx: &SecurityContext, name: &str, scope: &Scope) -> StoreResult<Personality>;
    async fn list_personalities(&self, ctx: &SecurityContext) -> StoreResult<Vec<Personality>>;
    async fn put_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()>;
    async fn delete_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<bool>;

    // Session templates (formerly in TemplateStore)
    async fn find_session_template_by_name(&self, ctx: &SecurityContext, name: &str, scope: &Scope) -> StoreResult<SessionTemplate>;
    async fn list_session_templates(&self, ctx: &SecurityContext) -> StoreResult<Vec<SessionTemplate>>;
    async fn put_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()>;
    async fn delete_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<bool>;

    // Agents (durable definition)
    async fn create_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()>;
    async fn get_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent>;
    async fn find_agent_by_name(&self, ctx: &SecurityContext, name: &str, scope: &Scope) -> StoreResult<Agent>;
    async fn update_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()>;

    // Sessions (durable definition)
    async fn create_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()>;
    async fn get_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session>;
    async fn update_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()>;
    async fn delete_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<bool>;
    async fn list_sessions(&self, ctx: &SecurityContext) -> StoreResult<Vec<Session>>;

    // Channels (issue 70): documents of their own, which the session lists
    // by id.
    async fn create_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()>;
    async fn get_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<Channel>;
    async fn update_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()>;
    async fn delete_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<bool>;

    // Users
    async fn create_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()>;
    async fn get_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<User>;
    async fn find_user_by_name(&self, ctx: &SecurityContext, name: &str) -> StoreResult<User>;
    async fn update_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()>;

    // Preconfigured components (referenceable elsewhere via `kind: "ref"`)
    async fn get_component(&self, ctx: &SecurityContext, id: &str) -> StoreResult<ComponentDescriptor>;
    async fn list_components(&self, ctx: &SecurityContext) -> StoreResult<Vec<ComponentDescriptor>>;
    async fn put_component(&self, ctx: &SecurityContext, value: &ComponentDescriptor) -> StoreResult<()>;
    async fn delete_component(&self, ctx: &SecurityContext, id: &str) -> StoreResult<bool>;
}

#[async_trait]
pub trait StateStore : Debug + Sync + Send {
    async fn get_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId) -> StoreResult<AgentChannelState>;
    async fn put_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId, state: &AgentChannelState) -> StoreResult<()>;

    async fn get_agent_state(&self, agent_id: AgentId) -> StoreResult<AgentState>;
    async fn put_agent_state(&self, agent_id: AgentId, state: &AgentState) -> StoreResult<()>;

    async fn get_session_state(&self, session_id: SessionId) -> StoreResult<SessionState>;
    async fn put_session_state(&self, session_id: SessionId, state: &SessionState) -> StoreResult<()>;

    async fn get_channel_state(&self, channel_id: ChannelId) -> StoreResult<ChannelState>;
    async fn put_channel_state(&self, channel_id: ChannelId, state: &ChannelState) -> StoreResult<()>;

    /// Deletes the state of a channel that no longer exists: its own and that
    /// of every agent in it. Nothing to delete is not an error (issue 09).
    async fn delete_channel_state(&self, channel_id: ChannelId) -> StoreResult<()>;
    /// Deletes the state of a session that no longer exists; its channels'
    /// states are deleted with `delete_channel_state`.
    async fn delete_session_state(&self, session_id: SessionId) -> StoreResult<()>;
}


#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::meta::Metadata;
    use crate::security::{SecurityAttributes, SecurityContext};
    use crate::session::Participant;
    use crate::store_impl::memory::MemoryStore;
    use crate::user::UserId;
    use std::collections::HashMap;
    use std::sync::Arc;
    use uuid::Uuid;


    fn personality_template() -> Personality {
        Personality {
            reasoning_confidentiality: crate::confidentiality::Confidentiality::Public,
            security_attributes: Default::default(),
            id: PersonalityId(Uuid::new_v4()),
            metadata: Metadata {
                name: "helper".to_string(),
                scope: Scope::GlobalScope,
                description: "Helpful assistant".to_string(),
            },
            components: crate::component::ComponentConfig::default(),
        }
    }

    #[tokio::test]
    async fn get_or_create_agent_returns_existing_agent() {
        let personality = personality_template();
        let existing = Agent::new("assistant".to_string(), Scope::GlobalScope, &personality, crate::actor::ActorId::None);

        let store = Arc::new(MemoryStore::new());
        let ctx = SecurityContext::default();
        store.put_personality(&ctx, &personality).await.unwrap();
        store.create_agent(&ctx, &existing).await.unwrap();

        let context = crate::session::SessionContext {
            id: SessionId(Uuid::new_v4()),
            owner: UserId::default(),
            security: SecurityContext::default(),
            entity_store: store.clone(),
        };

        let pt = crate::template::ParticipantTemplate::Agent {
            name: "assistant".to_string(),
            scope: crate::template::ScopeTemplate::GlobalScope,
            personality_id: personality.id,
        };

        let resolved = pt.instantiate(&context, &HashMap::new()).await.unwrap();

        if let Participant::Agent(agent) = resolved {
            assert_eq!(agent.id, existing.id);
        } else {
            panic!("Expected agent participant");
        }
    }

    #[tokio::test]
    async fn get_or_create_agent_creates_agent_when_missing() {
        let personality = personality_template();
        let store = Arc::new(MemoryStore::new());
        let ctx = SecurityContext::default();
        store.put_personality(&ctx, &personality).await.unwrap();

        let context = crate::session::SessionContext {
            id: SessionId(Uuid::new_v4()),
            owner: UserId::default(),
            security: SecurityContext::default(),
            entity_store: store.clone(),
        };

        let pt = crate::template::ParticipantTemplate::Agent {
            name: "assistant".to_string(),
            scope: crate::template::ScopeTemplate::GlobalScope,
            personality_id: personality.id,
        };

        let created_participant = pt.instantiate(&context, &HashMap::new()).await.unwrap();
        let created = if let Participant::Agent(agent) = created_participant {
            agent
        } else {
            panic!("Expected agent participant");
        };

        let fetched_participant = pt.instantiate(&context, &HashMap::new()).await.unwrap();
        let fetched = if let Participant::Agent(agent) = fetched_participant {
            agent
        } else {
            panic!("Expected agent participant");
        };

        assert_eq!(created.id, fetched.id);
        assert_eq!(created.personality_id, personality.id);
    }

    #[tokio::test]
    async fn list_sessions_works() {
        let session1 = Session {
            commands: Default::default(),
            id: SessionId(Uuid::new_v4()),
            metadata: Metadata {
                name: "session1".to_string(),
                scope: Scope::GlobalScope,
                ..Metadata::default()
            },
            participants: Vec::new(),
            channels: Vec::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            security_attributes: SecurityAttributes::default(),
        };
        let session2 = Session {
            commands: Default::default(),
            id: SessionId(Uuid::new_v4()),
            metadata: Metadata {
                name: "session2".to_string(),
                scope: Scope::GlobalScope,
                ..Metadata::default()
            },
            participants: Vec::new(),
            channels: Vec::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            security_attributes: SecurityAttributes::default(),
        };
        let store = Arc::new(MemoryStore::new());
        let ctx = SecurityContext::default();

        store.create_session(&ctx, &session1).await.unwrap();
        store.create_session(&ctx, &session2).await.unwrap();

        let sessions = store.list_sessions(&ctx).await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().any(|s| s.id == session1.id));
        assert!(sessions.iter().any(|s| s.id == session2.id));
    }
}