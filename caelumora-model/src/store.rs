use async_trait::async_trait;
use std::fmt::Debug;

use crate::agent::{Agent, AgentChannelState, AgentId, AgentState, Personality, PersonalityId};
use crate::channel::{Channel, ChannelId, ChannelState};
use crate::component::ComponentDescriptor;
use crate::security::SecurityContext;
use crate::session::{Session, SessionId};
use crate::template::{SessionTemplate, SessionTemplateId};
use crate::user::{User, UserId, UserProfile};


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


/// The durable entities (issue 93): every kind has the same five
/// operations - `create`, `get`, `update`, `delete`, `list` - by id, and
/// every one of them is checked the same way (`store_impl::access`).
/// `create` refuses an entity that exists (`Conflict`), `update` one that
/// does not (`NotFound`); `delete` says whether there was one to delete.
/// Only users are also found by name: a username is an identity. Whoever
/// looks for another kind by name filters `list_x`, with `Scope::contains`
/// where the scope matters.
#[async_trait]
pub trait EntityStore : Debug + Sync + Send {
    // Personalities
    async fn create_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()>;
    async fn get_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<Personality>;
    async fn update_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()>;
    async fn delete_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<bool>;
    async fn list_personalities(&self, ctx: &SecurityContext) -> StoreResult<Vec<Personality>>;

    // Session templates
    async fn create_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()>;
    async fn get_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<SessionTemplate>;
    async fn update_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()>;
    async fn delete_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<bool>;
    async fn list_session_templates(&self, ctx: &SecurityContext) -> StoreResult<Vec<SessionTemplate>>;

    // Agents
    async fn create_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()>;
    async fn get_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent>;
    async fn update_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()>;
    async fn delete_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<bool>;
    async fn list_agents(&self, ctx: &SecurityContext) -> StoreResult<Vec<Agent>>;

    // Sessions
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
    async fn list_channels(&self, ctx: &SecurityContext) -> StoreResult<Vec<Channel>>;

    // Users - they belong to the system: everybody reads them, only the
    // system changes them (issue 94).
    async fn create_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()>;
    async fn get_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<User>;
    async fn update_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()>;
    async fn delete_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool>;
    async fn list_users(&self, ctx: &SecurityContext) -> StoreResult<Vec<User>>;
    async fn find_user_by_name(&self, ctx: &SecurityContext, name: &str) -> StoreResult<User>;

    // User profiles - what a user says about themselves (issue 176), one
    // per user, under the user's id. Unlike the user, a profile belongs to
    // its user.
    async fn create_user_profile(&self, ctx: &SecurityContext, value: &UserProfile) -> StoreResult<()>;
    async fn get_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<UserProfile>;
    async fn update_user_profile(&self, ctx: &SecurityContext, value: &UserProfile) -> StoreResult<()>;
    async fn delete_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool>;
    async fn list_user_profiles(&self, ctx: &SecurityContext) -> StoreResult<Vec<UserProfile>>;

    // Preconfigured components (referenceable elsewhere via `kind: "ref"`)
    async fn create_component(&self, ctx: &SecurityContext, value: &ComponentDescriptor) -> StoreResult<()>;
    async fn get_component(&self, ctx: &SecurityContext, id: &str) -> StoreResult<ComponentDescriptor>;
    async fn update_component(&self, ctx: &SecurityContext, value: &ComponentDescriptor) -> StoreResult<()>;
    async fn delete_component(&self, ctx: &SecurityContext, id: &str) -> StoreResult<bool>;
    async fn list_components(&self, ctx: &SecurityContext) -> StoreResult<Vec<ComponentDescriptor>>;
}

#[async_trait]
pub trait StateStore : Debug + Sync + Send {
    async fn get_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId) -> StoreResult<AgentChannelState>;
    async fn put_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId, state: &AgentChannelState) -> StoreResult<()>;

    async fn get_agent_state(&self, agent_id: AgentId) -> StoreResult<AgentState>;
    async fn put_agent_state(&self, agent_id: AgentId, state: &AgentState) -> StoreResult<()>;

    async fn get_channel_state(&self, channel_id: ChannelId) -> StoreResult<ChannelState>;
    async fn put_channel_state(&self, channel_id: ChannelId, state: &ChannelState) -> StoreResult<()>;

    /// Deletes the state of a channel that no longer exists: its own and that
    /// of every agent in it. Nothing to delete is not an error (issue 09).
    async fn delete_channel_state(&self, channel_id: ChannelId) -> StoreResult<()>;
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::{SecurityAttributes, SecurityContext};
    use crate::store_impl::memory::MemoryStore;
    use std::sync::Arc;
    use uuid::Uuid;


    #[tokio::test]
    async fn list_sessions_works() {
        let session1 = Session {
            commands: Default::default(),
            privacy: Default::default(),
            id: SessionId(Uuid::new_v4()),
            name: "session1".to_string(),
            participants: Vec::new(),
            channels: Vec::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            security_attributes: SecurityAttributes::default(),
        };
        let session2 = Session {
            commands: Default::default(),
            privacy: Default::default(),
            id: SessionId(Uuid::new_v4()),
            name: "session2".to_string(),
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