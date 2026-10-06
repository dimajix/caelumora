use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt::Debug;

use crate::agent::{Agent, AgentChannelState, AgentId, AgentState, Personality, PersonalityId};
use crate::channel::{Channel, ChannelId, ChannelState};
use crate::component::StoredComponent;
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

/// What a lookup answers when what it looks for may no longer exist
/// (issue 192).
pub trait StoreResultExt<T> {
    /// What was found, `None` if it is not there (`StoreError::NotFound`);
    /// any other error stays one.
    fn optional(self) -> StoreResult<Option<T>>;
}

impl<T> StoreResultExt<T> for StoreResult<T> {
    fn optional(self) -> StoreResult<Option<T>> {
        match self {
            Ok(found) => Ok(Some(found)),
            Err(StoreError::NotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }
}


/// A state the `StateStore` keeps (issue 193): it tells what changed since
/// it was last stored, as a sequence of changes, so that a store writes only
/// that, and replays such changes onto what it stored before. A state is
/// small parts and growing ones - chains, values kept by digest - and its
/// `Change` has a variant per part: a growing part's records what was
/// added, a small part's its whole value, which `changes` puts at the end of
/// every sequence. Order holds within a part; across parts it is the fixed
/// order of `changes`, which loses nothing, since every reference between
/// parts is a digest or an id, resolved when read and never while replaying.
pub trait Journaled: Serialize + DeserializeOwned + Send {
    type Change: Serialize + DeserializeOwned + Send;
    /// What changed since `mark_stored`, in order.
    fn changes(&self) -> Vec<Self::Change>;
    /// Forgets the changes: they are stored.
    fn mark_stored(&mut self);
    /// Replays `change`, as a store does when loading what it appended -
    /// idempotent, and no change to store again (`Chain::apply`).
    ///
    /// # Errors
    /// If the change does not fit the state.
    fn apply(&mut self, change: Self::Change) -> Result<(), String>;
}


/// The kinds the stores keep - the entities of the `EntityStore` and the
/// components of the `ComponentStore` (issue 164): what the actions that
/// concern every kind alike are keyed by, and its name in errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityKind {
    Personality,
    SessionTemplate,
    Agent,
    Session,
    Channel,
    User,
    UserProfile,
    Component,
}

impl EntityKind {
    /// Its name, as errors give it.
    pub fn name(self) -> &'static str {
        match self {
            EntityKind::Personality => "personality",
            EntityKind::SessionTemplate => "session_template",
            EntityKind::Agent => "agent",
            EntityKind::Session => "session",
            EntityKind::Channel => "channel",
            EntityKind::User => "user",
            EntityKind::UserProfile => "user_profile",
            EntityKind::Component => "component",
        }
    }
}

impl std::fmt::Display for EntityKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The durable entities (issue 93): every kind has the same five
/// operations - `create`, `get`, `update`, `delete`, `list` - by id, and
/// every one of them is checked the same way (`store_impl::access`).
/// `create` refuses an entity that exists (`Conflict`), `update` one that
/// does not (`NotFound`); `delete` says whether there was one to delete.
/// Only users are also found by name: a username is an identity. Whoever
/// looks for another kind by name filters `list_x`, with `Scope::contains`
/// where the scope matters.
///
/// Deleting marks (issue 164): `delete_x` sets the mark - who, when - and
/// keeps the document. What is marked is not there for any of the five:
/// not read, not listed, not changed, not deleted again - also not for the
/// system - and its id is not free for `create`. Nothing new or changed can
/// carry the mark (`Validation`): only deleting sets it. What was deleted
/// is reached by four operations of its own, each kind alike:
/// `get_deleted_x` and `list_deleted_x` read it (`ReadDeleted`),
/// `restore_x` takes the mark away (`Restore`), and `purge_x` removes what
/// was deleted for good (`Purge`) - only what is marked, so that what is
/// deleted is deleted first, and a purge never takes what was restored.
#[async_trait]
pub trait EntityStore : Debug + Sync + Send {
    // Personalities
    async fn create_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()>;
    async fn get_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<Personality>;
    async fn update_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()>;
    async fn delete_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<bool>;
    async fn list_personalities(&self, ctx: &SecurityContext) -> StoreResult<Vec<Personality>>;
    async fn restore_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<()>;
    async fn purge_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<bool>;
    async fn get_deleted_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<Personality>;
    async fn list_deleted_personalities(&self, ctx: &SecurityContext) -> StoreResult<Vec<Personality>>;

    // Session templates
    async fn create_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()>;
    async fn get_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<SessionTemplate>;
    async fn update_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()>;
    async fn delete_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<bool>;
    async fn list_session_templates(&self, ctx: &SecurityContext) -> StoreResult<Vec<SessionTemplate>>;
    async fn restore_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<()>;
    async fn purge_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<bool>;
    async fn get_deleted_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<SessionTemplate>;
    async fn list_deleted_session_templates(&self, ctx: &SecurityContext) -> StoreResult<Vec<SessionTemplate>>;

    // Agents
    async fn create_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()>;
    async fn get_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent>;
    async fn update_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()>;
    async fn delete_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<bool>;
    async fn list_agents(&self, ctx: &SecurityContext) -> StoreResult<Vec<Agent>>;
    async fn restore_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<()>;
    async fn purge_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<bool>;
    async fn get_deleted_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent>;
    async fn list_deleted_agents(&self, ctx: &SecurityContext) -> StoreResult<Vec<Agent>>;

    // Sessions
    async fn create_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()>;
    async fn get_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session>;
    async fn update_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()>;
    async fn delete_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<bool>;
    async fn list_sessions(&self, ctx: &SecurityContext) -> StoreResult<Vec<Session>>;
    async fn restore_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<()>;
    async fn purge_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<bool>;
    async fn get_deleted_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session>;
    async fn list_deleted_sessions(&self, ctx: &SecurityContext) -> StoreResult<Vec<Session>>;

    // Channels (issue 70): documents of their own, which the session lists
    // by id.
    async fn create_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()>;
    async fn get_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<Channel>;
    async fn update_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()>;
    async fn delete_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<bool>;
    async fn list_channels(&self, ctx: &SecurityContext) -> StoreResult<Vec<Channel>>;
    async fn restore_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<()>;
    async fn purge_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<bool>;
    async fn get_deleted_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<Channel>;
    async fn list_deleted_channels(&self, ctx: &SecurityContext) -> StoreResult<Vec<Channel>>;

    // Users - they belong to the system: everybody reads them, only the
    // system changes them (issue 94).
    async fn create_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()>;
    async fn get_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<User>;
    async fn update_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()>;
    async fn delete_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool>;
    async fn list_users(&self, ctx: &SecurityContext) -> StoreResult<Vec<User>>;
    async fn restore_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<()>;
    async fn purge_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool>;
    async fn get_deleted_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<User>;
    async fn list_deleted_users(&self, ctx: &SecurityContext) -> StoreResult<Vec<User>>;
    async fn find_user_by_name(&self, ctx: &SecurityContext, name: &str) -> StoreResult<User>;

    // User profiles - what a user says about themselves (issue 176), one
    // per user, under the user's id. Unlike the user, a profile belongs to
    // its user.
    async fn create_user_profile(&self, ctx: &SecurityContext, value: &UserProfile) -> StoreResult<()>;
    async fn get_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<UserProfile>;
    async fn update_user_profile(&self, ctx: &SecurityContext, value: &UserProfile) -> StoreResult<()>;
    async fn delete_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool>;
    async fn list_user_profiles(&self, ctx: &SecurityContext) -> StoreResult<Vec<UserProfile>>;
    async fn restore_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<()>;
    async fn purge_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool>;
    async fn get_deleted_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<UserProfile>;
    async fn list_deleted_user_profiles(&self, ctx: &SecurityContext) -> StoreResult<Vec<UserProfile>>;
}

/// The preconfigured components: runtime configuration that personalities
/// and templates refer to by `kind: "ref"`, kept by id - not an entity of
/// their own standing, so not in the `EntityStore`. The same five
/// operations, checked the same way (`store_impl::access`); deleting
/// removes for good, since a component is hidden with what refers to it
/// (issue 164).
#[async_trait]
pub trait ComponentStore : Debug + Sync + Send {
    async fn create_component(&self, ctx: &SecurityContext, value: &StoredComponent) -> StoreResult<()>;
    async fn get_component(&self, ctx: &SecurityContext, id: &str) -> StoreResult<StoredComponent>;
    async fn update_component(&self, ctx: &SecurityContext, value: &StoredComponent) -> StoreResult<()>;
    async fn delete_component(&self, ctx: &SecurityContext, id: &str) -> StoreResult<bool>;
    async fn list_components(&self, ctx: &SecurityContext) -> StoreResult<Vec<StoredComponent>>;
}

/// The runtime state (issue 193): every kind `Journaled`, read whole and
/// put as it stands. A `put_…` takes the state mutably: the store stores
/// it - whole, or only what changed since it was last stored, as the store
/// chooses - and on `Ok` has marked it stored (`Journaled::mark_stored`).
/// On `Err` nothing is marked: the changes stay pending, and the next put
/// includes them. So what is pending is never what a store has.
///
/// Every state has exactly one writer, and its puts never overlap (issue
/// 215): a store may keep only what changed since the state was last stored,
/// which is known of the one instance that stored it, not of another. Two
/// writers of one state, even one after the other, would store the changes
/// of the one onto what the other stored.
#[async_trait]
pub trait StateStore : Debug + Sync + Send {
    /// An agent in a channel, of either kind (issue 206).
    async fn get_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId) -> StoreResult<AgentChannelState>;
    async fn put_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId, state: &mut AgentChannelState) -> StoreResult<()>;

    async fn get_agent_state(&self, agent_id: AgentId) -> StoreResult<AgentState>;
    async fn put_agent_state(&self, agent_id: AgentId, state: &mut AgentState) -> StoreResult<()>;

    async fn get_channel_state(&self, channel_id: ChannelId) -> StoreResult<ChannelState>;
    async fn put_channel_state(&self, channel_id: ChannelId, state: &mut ChannelState) -> StoreResult<()>;

    /// Deletes the state of a channel that no longer exists: its own and that
    /// of every agent in it. Nothing to delete is not an error (issue 09).
    async fn delete_channel_state(&self, channel_id: ChannelId) -> StoreResult<()>;

    /// Deletes the state of an agent purged for good (issue 164): its own
    /// and its state in every channel. Nothing to delete is not an error.
    async fn delete_agent_state(&self, agent_id: AgentId) -> StoreResult<()>;
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::{SecurityAttributes, SecurityContext};
    use crate::store_impl::memory::MemoryStore;
    use std::sync::Arc;
    use uuid::Uuid;


    #[test]
    fn what_is_not_found_is_none_and_every_other_error_stays_one() {
        let found: StoreResult<u32> = Ok(1);
        assert_eq!(found.optional().unwrap(), Some(1));
        let gone: StoreResult<u32> = Err(StoreError::NotFound { entity: "user", id: "1".to_string() });
        assert_eq!(gone.optional().unwrap(), None);
        let failed: StoreResult<u32> = Err(StoreError::Backend("disk".to_string()));
        assert!(matches!(failed.optional(), Err(StoreError::Backend(_))));
    }

    #[tokio::test]
    async fn list_sessions_works() {
        let session1 = Session {
            approval_rules: Vec::new(),
            commands: Default::default(),
            privacy: Default::default(),
            id: SessionId(Uuid::new_v4()),
            name: "session1".to_string(),
            participants: Vec::new(),
            channels: Vec::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            security_attributes: SecurityAttributes::default(),
            deleted: None,
        };
        let session2 = Session {
            approval_rules: Vec::new(),
            commands: Default::default(),
            privacy: Default::default(),
            id: SessionId(Uuid::new_v4()),
            name: "session2".to_string(),
            participants: Vec::new(),
            channels: Vec::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            security_attributes: SecurityAttributes::default(),
            deleted: None,
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