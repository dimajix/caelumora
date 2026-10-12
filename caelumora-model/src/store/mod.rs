pub(crate) mod access;
pub mod json;
pub mod memberships;
pub mod memory;
pub mod query;
#[cfg(test)]
pub(crate) mod suite;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::Debug;
use std::sync::Arc;

use crate::agent::{Agent, AgentChannelState, AgentId, AgentState, Personality};
use crate::channel::{Channel, ChannelId, ChannelState};
use crate::component::StoredComponents;
use crate::removal::Deletable;
use crate::security::SecurityContext;
use crate::session::Session;
use crate::template::SessionTemplate;
use crate::user::{User, UserProfile};
use crate::value::name::ScopedName;
use crate::workspace::Workspace;
use crate::skill::Skill;
use crate::group::Group;
use crate::intern::intern;
use crate::artifact::Artifact;
use crate::store::query::{NoField, Page, Query};


#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("not found: {entity} {id}")]
    NotFound { entity: &'static str, id: String },
    /// An id that is taken.
    #[error("conflict: {0}")]
    Conflict(String),
    /// A name that another of the kind has in its name space (issue 250):
    /// creating, renaming or restoring one under it is refused.
    #[error("the name is taken by another {entity}")]
    NameTaken { entity: &'static str },
    #[error("validation: {0}")]
    Validation(String),
    /// A text that does not fit the field it was to be written into
    /// (issue 248): what the field is called and why.
    #[error("{0}")]
    Invalid(#[from] crate::value::ValueError),
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


/// A kind the stores keep - of the entities of the `EntityStore` and the
/// components of the `ComponentStore` (issue 164): what the actions that
/// concern every kind alike are keyed by, and its name in errors. Open
/// (issue 323): each stored type names its own (`Stored::KIND`), so that a
/// kind is added without a change here. Its name is unique among the kinds;
/// it is also what the JSON store keeps the kind's files under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntityKind(&'static str);

impl EntityKind {
    /// The kind of `name`, written in `snake_case`.
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    /// Its name, as errors give it.
    pub fn name(self) -> &'static str {
        self.0
    }

    /// Its name as a type's is written - `SessionTemplate` -, as
    /// `ActionType::describe` names what is done with it.
    pub fn title(self) -> String {
        self.name().split('_').map(|word| {
            let mut letters = word.chars();
            letters.next().map(|first| first.to_ascii_uppercase().to_string() + letters.as_str()).unwrap_or_default()
        }).collect()
    }
}

/// Written as its name; read back as the kind of that name (`intern`), so
/// that an event from another instance (issue 122) names the kind this one
/// defines - or one nothing here waits for.
impl Serialize for EntityKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for EntityKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|name| Self(intern(&name)))
    }
}

impl std::fmt::Display for EntityKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The entities of one kind (issues 93, 323): every kind has the same
/// operations, checked the same way (`store::access`), and is kept by a
/// store of its own. `create` refuses an entity that exists (`Conflict`),
/// `update` one that does not (`NotFound`); `delete` says whether there was
/// one to delete. What has a name unique of its kind in its scope
/// (`Stored::unique_name`, issue 250) is created, renamed and restored only
/// under a name no other one of its kind that is not deleted has there,
/// also one the caller may not read (`NameTaken`), and is found by it
/// (`find_by_name`) - `NotFound` for a kind without one.
///
/// Deleting marks (issue 164): `delete` sets the mark - who, when - and
/// keeps the entity. What is marked is not there for `get`, `list`,
/// `update` and `delete`, also not for the system, and its id is not free
/// for `create`; nothing new or changed may carry the mark (`Validation`).
/// What was deleted is reached by four operations of its own:
/// `get_deleted` and `list_deleted` read it (`ReadDeleted`), `restore`
/// takes the mark away (`Restore`) and returns what it restored, and
/// `purge` removes what was deleted for good (`Purge`) - only what is
/// marked, so that a purge never takes what was restored.
#[async_trait]
pub trait EntityStore<T: Deletable + Send + Sync + 'static>: Debug + Send + Sync {
    async fn create(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()>;
    async fn get(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T>;
    async fn update(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()>;
    async fn delete(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<bool>;
    async fn list(&self, ctx: &SecurityContext, query: &Query<T::Field>) -> StoreResult<Page<T>>;
    async fn restore(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T>;
    async fn purge(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<bool>;
    async fn get_deleted(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T>;
    async fn list_deleted(&self, ctx: &SecurityContext, query: &Query<T::Field>) -> StoreResult<Page<T>>;
    /// The one of `name`, in exactly that scope, among what the caller may
    /// read (issue 178).
    async fn find_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<T>;
}

/// The stores of the entities (issue 323), one per kind of the core,
/// which the runtime is given and hands to the managers that keep each
/// kind.
#[derive(Debug, Clone)]
pub struct EntityStores {
    pub skills: Arc<dyn EntityStore<Skill>>,
    pub workspaces: Arc<dyn EntityStore<Workspace>>,
    pub artifacts: Arc<dyn EntityStore<Artifact>>,
    pub groups: Arc<dyn EntityStore<Group>>,
    pub session_templates: Arc<dyn EntityStore<SessionTemplate>>,
    pub user_profiles: Arc<dyn EntityStore<UserProfile>>,
    pub personalities: Arc<dyn EntityStore<Personality>>,
    pub users: Arc<dyn EntityStore<User>>,
    pub agents: Arc<dyn EntityStore<Agent>>,
    pub channels: Arc<dyn EntityStore<Channel>>,
    pub sessions: Arc<dyn EntityStore<Session>>,
}

/// The preconfigured components: runtime configuration that personalities
/// and templates refer to by `kind: "ref"`, kept by id - not an entity of
/// their own standing, so not in the `EntityStore`. The same five
/// operations, checked the same way (`store::access`); deleting
/// removes for good, since a component is hidden with what refers to it
/// (issue 164).
#[async_trait]
pub trait ComponentStore : Debug + Sync + Send {
    async fn create_components(&self, ctx: &SecurityContext, value: &StoredComponents) -> StoreResult<()>;
    async fn get_components(&self, ctx: &SecurityContext, id: &str) -> StoreResult<StoredComponents>;
    async fn update_components(&self, ctx: &SecurityContext, value: &StoredComponents) -> StoreResult<()>;
    async fn delete_components(&self, ctx: &SecurityContext, id: &str) -> StoreResult<bool>;
    async fn list_components(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<StoredComponents>>;
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
    use crate::session::SessionId;
    use super::*;
    use crate::security::{SecurityAttributes, SecurityContext};
    use crate::store::memory::MemoryStore;
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
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            workspace: None,
            commands: Default::default(),
            privacy: Default::default(),
            id: SessionId(Uuid::new_v4()),
            name: crate::value::name::Name::new("session1").unwrap(),
            participants: Vec::new(),
            channels: Vec::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            security_attributes: SecurityAttributes::default(),
            deleted: None,
        };
        let session2 = Session {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            workspace: None,
            commands: Default::default(),
            privacy: Default::default(),
            id: SessionId(Uuid::new_v4()),
            name: crate::value::name::Name::new("session2").unwrap(),
            participants: Vec::new(),
            channels: Vec::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            security_attributes: SecurityAttributes::default(),
            deleted: None,
        };
        let store = Arc::new(MemoryStore::new()).entity_stores().sessions;
        let ctx = SecurityContext::default();

        store.create(&ctx, &session1).await.unwrap();
        store.create(&ctx, &session2).await.unwrap();

        let sessions = store.list(&ctx, &Query::all()).await.map(|page| page.items).unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().any(|s| s.id == session1.id));
        assert!(sessions.iter().any(|s| s.id == session2.id));
    }
}