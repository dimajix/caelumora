pub(crate) mod access;
pub mod json;
pub mod memberships;
pub mod memory;
pub mod query;
#[cfg(test)]
pub(crate) mod suite;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt::Debug;
use std::sync::Arc;

use crate::agent::{Agent, AgentChannelState, AgentId, AgentState, Personality, PersonalityId};
use crate::channel::{Channel, ChannelId, ChannelState};
use crate::component::StoredComponents;
use crate::removal::Deletable;
use crate::security::SecurityContext;
use crate::session::{Session, SessionId};
use crate::template::{SessionTemplate, SessionTemplateId};
use crate::user::{User, UserId, UserProfile};
use crate::value::name::ScopedName;
use crate::workspace::{Workspace, WorkspaceId};
use crate::skill::Skill;
use crate::group::{Group, GroupField, GroupId};
use crate::artifact::{Artifact, ArtifactField, ArtifactId};
use crate::channel::ChannelField;
use crate::scope::ScopeField;
use crate::session::SessionField;
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
/// kind. Until every kind has its own, `entities` offers the others at
/// once.
#[derive(Debug, Clone)]
pub struct EntityStores {
    pub entities: Arc<dyn Entities>,
    pub skills: Arc<dyn EntityStore<Skill>>,
}

/// Every kind's entities at once, as one store offered them before each
/// kind had its own (issue 323): implemented over the stores of the kinds,
/// and gone once every caller uses the store of its kind.
///
/// The durable entities (issue 93): every kind has the same five
/// operations - `create`, `get`, `update`, `delete`, `list` - by id, and
/// every one of them is checked the same way (`store::access`).
/// `create` refuses an entity that exists (`Conflict`), `update` one that
/// does not (`NotFound`); `delete` says whether there was one to delete.
/// Some kinds are also found by name, among what the caller may read -
/// `NotFound` if there is none: users by their username, an identity, and
/// what lies in a scope - personalities, session templates, agents - by
/// their `ScopedName`, the name in exactly that scope (issue 178). Finding
/// one from a place, in the scopes around it, is `scope::find_by_name`
/// over `list_x`.
///
/// What is `Scoped` - personalities, session templates, agents, channels,
/// users, workspaces, skills, artifacts, groups - has a name unique of its kind in its scope (issue
/// 250): `create`,
/// `update` and `restore` refuse one that another has there (`NameTaken`),
/// checked against every one of the kind that is not deleted, also those
/// the caller may not read, in the same step as the write. Sessions have
/// names, but no unique ones.
///
/// Deleting marks (issue 164): `delete_x` sets the mark - who, when - and
/// keeps the document. What is marked is not there for any of the five:
/// not read, not listed, not changed, not deleted again - also not for the
/// system - and its id is not free for `create`. Nothing new or changed can
/// carry the mark (`Validation`): only deleting sets it. What was deleted
/// is reached by four operations of its own, each kind alike:
/// `get_deleted_x` and `list_deleted_x` read it (`ReadDeleted`),
/// `restore_x` takes the mark away (`Restore`) and returns what it
/// restored, and `purge_x` removes what was deleted for good (`Purge`) -
/// only what is marked, so that what is deleted is deleted first, and a
/// purge never takes what was restored.
#[async_trait]
pub trait Entities : Debug + Sync + Send {
    // Personalities
    async fn create_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()>;
    async fn get_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<Personality>;
    async fn update_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()>;
    async fn delete_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<bool>;
    async fn list_personalities(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Personality>>;
    async fn restore_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<Personality>;
    async fn purge_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<bool>;
    async fn get_deleted_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<Personality>;
    async fn list_deleted_personalities(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Personality>>;
    async fn find_personality_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Personality>;

    // Session templates
    async fn create_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()>;
    async fn get_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<SessionTemplate>;
    async fn update_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()>;
    async fn delete_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<bool>;
    async fn list_session_templates(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<SessionTemplate>>;
    async fn restore_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<SessionTemplate>;
    async fn purge_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<bool>;
    async fn get_deleted_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<SessionTemplate>;
    async fn list_deleted_session_templates(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<SessionTemplate>>;
    async fn find_session_template_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<SessionTemplate>;

    // Agents
    async fn create_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()>;
    async fn get_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent>;
    async fn update_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()>;
    async fn delete_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<bool>;
    async fn list_agents(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Agent>>;
    async fn restore_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent>;
    async fn purge_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<bool>;
    async fn get_deleted_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent>;
    async fn list_deleted_agents(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Agent>>;
    async fn find_agent_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Agent>;

    // Sessions
    async fn create_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()>;
    async fn get_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session>;
    async fn update_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()>;
    async fn delete_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<bool>;
    async fn list_sessions(&self, ctx: &SecurityContext, query: &Query<SessionField>) -> StoreResult<Page<Session>>;
    async fn restore_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session>;
    async fn purge_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<bool>;
    async fn get_deleted_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session>;
    async fn list_deleted_sessions(&self, ctx: &SecurityContext, query: &Query<SessionField>) -> StoreResult<Page<Session>>;

    // Channels (issue 70): documents of their own, which the session lists
    // by id.
    async fn create_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()>;
    async fn get_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<Channel>;
    async fn update_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()>;
    async fn delete_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<bool>;
    async fn list_channels(&self, ctx: &SecurityContext, query: &Query<ChannelField>) -> StoreResult<Page<Channel>>;
    async fn restore_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<Channel>;
    async fn purge_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<bool>;
    async fn get_deleted_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<Channel>;
    async fn list_deleted_channels(&self, ctx: &SecurityContext, query: &Query<ChannelField>) -> StoreResult<Page<Channel>>;

    // Users - they belong to the system: everybody reads them, only the
    // system changes them (issue 94).
    async fn create_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()>;
    async fn get_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<User>;
    async fn update_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()>;
    async fn delete_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool>;
    async fn list_users(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<User>>;
    async fn restore_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<User>;
    async fn purge_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool>;
    async fn get_deleted_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<User>;
    async fn list_deleted_users(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<User>>;
    async fn find_user_by_name(&self, ctx: &SecurityContext, name: &str) -> StoreResult<User>;

    // User profiles - what a user says about themselves (issue 176), one
    // per user, under the user's id. Unlike the user, a profile belongs to
    // its user.
    async fn create_user_profile(&self, ctx: &SecurityContext, value: &UserProfile) -> StoreResult<()>;
    async fn get_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<UserProfile>;
    async fn update_user_profile(&self, ctx: &SecurityContext, value: &UserProfile) -> StoreResult<()>;
    async fn delete_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool>;
    async fn list_user_profiles(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<UserProfile>>;
    async fn restore_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<UserProfile>;
    async fn purge_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool>;
    async fn get_deleted_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<UserProfile>;
    async fn list_deleted_user_profiles(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<UserProfile>>;

    // Workspaces (issue 253) - their files are no part of the document, and
    // whoever purges one removes them.
    async fn create_workspace(&self, ctx: &SecurityContext, value: &Workspace) -> StoreResult<()>;
    async fn get_workspace(&self, ctx: &SecurityContext, id: WorkspaceId) -> StoreResult<Workspace>;
    async fn update_workspace(&self, ctx: &SecurityContext, value: &Workspace) -> StoreResult<()>;
    async fn delete_workspace(&self, ctx: &SecurityContext, id: WorkspaceId) -> StoreResult<bool>;
    async fn list_workspaces(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Workspace>>;
    async fn restore_workspace(&self, ctx: &SecurityContext, id: WorkspaceId) -> StoreResult<Workspace>;
    async fn purge_workspace(&self, ctx: &SecurityContext, id: WorkspaceId) -> StoreResult<bool>;
    async fn get_deleted_workspace(&self, ctx: &SecurityContext, id: WorkspaceId) -> StoreResult<Workspace>;
    async fn list_deleted_workspaces(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Workspace>>;
    async fn find_workspace_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Workspace>;

    // Artifacts (issue 271) - their content is no part of the document, and
    // whoever purges one removes it.
    async fn create_artifact(&self, ctx: &SecurityContext, value: &Artifact) -> StoreResult<()>;
    async fn get_artifact(&self, ctx: &SecurityContext, id: ArtifactId) -> StoreResult<Artifact>;
    async fn update_artifact(&self, ctx: &SecurityContext, value: &Artifact) -> StoreResult<()>;
    async fn delete_artifact(&self, ctx: &SecurityContext, id: ArtifactId) -> StoreResult<bool>;
    async fn list_artifacts(&self, ctx: &SecurityContext, query: &Query<ArtifactField>) -> StoreResult<Page<Artifact>>;
    async fn restore_artifact(&self, ctx: &SecurityContext, id: ArtifactId) -> StoreResult<Artifact>;
    async fn purge_artifact(&self, ctx: &SecurityContext, id: ArtifactId) -> StoreResult<bool>;
    async fn get_deleted_artifact(&self, ctx: &SecurityContext, id: ArtifactId) -> StoreResult<Artifact>;
    async fn list_deleted_artifacts(&self, ctx: &SecurityContext, query: &Query<ArtifactField>) -> StoreResult<Page<Artifact>>;
    async fn find_artifact_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Artifact>;

    // Groups (issue 286) - teams, and the participants of sessions and
    // channels, kept by those.
    async fn create_group(&self, ctx: &SecurityContext, value: &Group) -> StoreResult<()>;
    async fn get_group(&self, ctx: &SecurityContext, id: GroupId) -> StoreResult<Group>;
    async fn update_group(&self, ctx: &SecurityContext, value: &Group) -> StoreResult<()>;
    async fn delete_group(&self, ctx: &SecurityContext, id: GroupId) -> StoreResult<bool>;
    async fn list_groups(&self, ctx: &SecurityContext, query: &Query<GroupField>) -> StoreResult<Page<Group>>;
    async fn restore_group(&self, ctx: &SecurityContext, id: GroupId) -> StoreResult<Group>;
    async fn purge_group(&self, ctx: &SecurityContext, id: GroupId) -> StoreResult<bool>;
    async fn get_deleted_group(&self, ctx: &SecurityContext, id: GroupId) -> StoreResult<Group>;
    async fn list_deleted_groups(&self, ctx: &SecurityContext, query: &Query<GroupField>) -> StoreResult<Page<Group>>;
    async fn find_group_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Group>;
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
        let store = Arc::new(MemoryStore::new());
        let ctx = SecurityContext::default();

        store.create_session(&ctx, &session1).await.unwrap();
        store.create_session(&ctx, &session2).await.unwrap();

        let sessions = store.list_sessions(&ctx, &Query::all()).await.map(|page| page.items).unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().any(|s| s.id == session1.id));
        assert!(sessions.iter().any(|s| s.id == session2.id));
    }
}