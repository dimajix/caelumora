pub(crate) mod access;
pub mod history;
pub mod json;
pub mod memberships;
pub mod memory;
pub mod query;
#[cfg(test)]
pub(crate) mod suite;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::str::FromStr;
use std::sync::Arc;

use crate::agent::{Agent, AgentChannelState, AgentId, AgentState, Personality};
use crate::channel::{Channel, ChannelId, ChannelState};
use crate::component::StoredComponents;
use crate::removal::{Deletable, Removal};
use crate::confidentiality::{Classification, ConfidentialityPolicy};
use crate::security::{Refused, Secured, SecurityContext, AccessPolicy};
use crate::session::Session;
use crate::template::SessionTemplate;
use crate::user::{User, UserProfile};
use crate::value::name::ScopedName;
use crate::value::ValueError;
use chrono::{DateTime, Utc};
use crate::workspace::Workspace;
use crate::group::Group;
use crate::intern::intern;
use crate::store::history::HistoryStore;
use crate::store::json::JsonFileStore;
use crate::store::memory::MemoryStore;
use crate::store::query::{Field, NoField, Page, Query};
use crate::type_map::TypeKey;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::fmt;


#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("not found: {entity} {id}")]
    NotFound { entity: &'static str, id: String },
    /// An id that is taken - of an entity, or the name of a kind (issue
    /// 323).
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
    Invalid(#[from] ValueError),
    #[error("backend: {0}")]
    Backend(String),
    /// Refused (issue 323): what was refused is its source.
    #[error("permission denied")]
    PermissionDenied(#[source] Refused),
}

pub type StoreResult<T> = Result<T, StoreError>;

/// The `AccessPolicy` and the `ConfidentialityPolicy` every store of a
/// backend asks.
pub(crate) type Policies = (Arc<dyn AccessPolicy>, Arc<dyn ConfidentialityPolicy>);

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

impl Display for EntityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// An entity the `EntityStore` keeps: what the stores are generic over,
/// besides `Secured`. Whether it was deleted is asked of every kind alike,
/// so that every kind is read the same way; only a `Deletable` kind can be.
/// A store keeps it as it is serialized, and hands out copies, so that a
/// backend can keep any kind (issue 323).
pub trait Stored: Secured + Clone + Serialize + DeserializeOwned + Send + Sync + 'static {
    /// Its kind (issue 323), by which what is done with it is asked and
    /// named.
    const KIND: EntityKind;

    /// What it is kept and found by.
    type Id: Clone + Eq + Hash + Display + FromStr + Debug + Send + Sync + 'static;

    /// The kind of entity it lies in - a channel in a `Session` -, or
    /// `NoEntity` for a kind that lies in none (issue 335). Its events name
    /// that entity beside its own id.
    type Within: Enclosing;

    /// The fields a query of its kind names (issue 293).
    type Field: Field;

    /// Its id.
    fn id(&self) -> Self::Id;

    /// Its name and the scope it lies in, if its kind is `Scoped` - a name
    /// unique of its kind in that scope, which the store keeps so (issue
    /// 250) -; `None` for a kind without one.
    fn unique_name(&self) -> Option<ScopedName> {
        None
    }

    /// Whether, and by whom and when, it was deleted; never for a kind that
    /// is not `Deletable`.
    fn deleted(&self) -> Option<&Removal> {
        None
    }

    /// Nothing if it may be stored as it is; why not otherwise, which the
    /// store answers as `StoreError::Validation` when it is created or
    /// changed.
    fn validate(&self) -> Result<(), String> {
        Ok(())
    }

    /// How it is classified - a channel, a workspace, a skill, an artifact
    /// -, `None` for a kind that is not. Whoever reaches it through the store
    /// must be cleared for it (issue 322).
    fn classification(&self) -> Option<&Classification> {
        None
    }
}

/// A kind whose changes are told by an update event (issues 248, 358): the
/// event says what was asked, `apply` makes it into the kind's values - where
/// it was asked and wherever it is announced, so that every live copy changes
/// alike.
pub trait Updatable: Stored {
    /// What a change of it says, as it is asked and announced
    /// (`GlobalEvent::updated`).
    type Update: Serialize + DeserializeOwned + Send + Sync;

    /// Applies `update`: what it says is made into its values here.
    ///
    /// # Errors
    /// `ValueError` if what it says does not fit; it is then unchanged.
    fn apply(&mut self, update: &Self::Update) -> Result<(), ValueError>;

    /// Notes that it was changed `at`, for a kind that keeps when it was
    /// changed last; nothing for one that does not.
    fn touch(&mut self, _at: DateTime<Utc>) {}
}

/// What a kind can lie in (issue 335): the kind of entity that encloses
/// it, or `NoEntity`. It says how an entity with an id `I` is named where
/// it lies - its `Address` -, and how its events write that.
pub trait Enclosing {
    /// How an entity with the id `I` that lies in it is named: by the id of
    /// the entity it lies in and its own, or by its own alone where it lies
    /// in none.
    type Address<I>;

    /// `address` as an event writes it: the id of the entity it lies in, if
    /// any, and its own.
    fn write<I: Display>(address: &Self::Address<I>) -> (Option<String>, String);

    /// The address an event wrote as `within` and `id`; `None` if one of
    /// them is no id of its kind, or names an entity it cannot lie in.
    fn read<I: FromStr>(within: Option<&str>, id: &str) -> Option<Self::Address<I>>;
}

/// Lying in an entity of a stored kind: named by its id and one's own.
impl<T: Stored> Enclosing for T {
    type Address<I> = (T::Id, I);

    fn write<I: Display>((within, id): &(T::Id, I)) -> (Option<String>, String) {
        (Some(within.to_string()), id.to_string())
    }

    fn read<I: FromStr>(within: Option<&str>, id: &str) -> Option<(T::Id, I)> {
        Some((within?.parse().ok()?, id.parse().ok()?))
    }
}

/// What a kind that lies in no entity names as its `Stored::Within`: a
/// session, an agent, a user. There is none of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoEntity {}

/// Lying in nothing: named by one's own id alone.
impl Enclosing for NoEntity {
    type Address<I> = I;

    fn write<I: Display>(id: &I) -> (Option<String>, String) {
        (None, id.to_string())
    }

    fn read<I: FromStr>(within: Option<&str>, id: &str) -> Option<I> {
        match within {
            None => id.parse().ok(),
            Some(_) => None,
        }
    }
}

/// How an entity of the kind `T` is named where it lies (issue 335): its
/// id, or the id of the entity it lies in and its own - a channel's
/// `(SessionId, ChannelId)`.
pub type Address<T> = <<T as Stored>::Within as Enclosing>::Address<<T as Stored>::Id>;

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
pub trait EntityStore<T: Deletable>: Debug + Send + Sync {
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

/// What opens the store of any kind (issue 323): a backend, which opens
/// each kind once - asked again, it gives the same store, so that the core
/// and every plugin asking for a kind share its store.
pub trait EntityStoreFactory {
    /// What opening a store can fail with.
    type Error;

    /// The store of the entities of `T`.
    ///
    /// # Errors
    /// If the store cannot be opened.
    fn entity_store<T: Deletable>(&self) -> Result<Arc<dyn EntityStore<T>>, Self::Error>;
}

/// The stores of the entities (issue 323), one per kind of the core,
/// which the runtime is given and hands to the managers that keep each
/// kind. A plugin's kinds are not among them: it asks for their stores
/// itself (`EntityStoreFactory`).
#[derive(Debug, Clone)]
pub struct EntityStores {
    pub workspaces: Arc<dyn EntityStore<Workspace>>,
    pub groups: Arc<dyn EntityStore<Group>>,
    pub session_templates: Arc<dyn EntityStore<SessionTemplate>>,
    pub user_profiles: Arc<dyn EntityStore<UserProfile>>,
    pub personalities: Arc<dyn EntityStore<Personality>>,
    pub users: Arc<dyn EntityStore<User>>,
    pub agents: Arc<dyn EntityStore<Agent>>,
    pub channels: Arc<dyn EntityStore<Channel>>,
    pub sessions: Arc<dyn EntityStore<Session>>,
}

impl EntityStores {
    /// The stores of the core's kinds, as `factory` opens them.
    ///
    /// # Errors
    /// If one cannot be opened.
    pub fn open<F: EntityStoreFactory>(factory: &F) -> Result<Self, F::Error> {
        Ok(Self {
            workspaces: factory.entity_store()?,
            groups: factory.entity_store()?,
            session_templates: factory.entity_store()?,
            user_profiles: factory.entity_store()?,
            personalities: factory.entity_store()?,
            users: factory.entity_store()?,
            agents: factory.entity_store()?,
            channels: factory.entity_store()?,
            sessions: factory.entity_store()?,
        })
    }
}

/// The store of the configured backend (issue 323): of the entities of
/// every kind, of the preconfigured components and of the states. It opens
/// the store of a kind for whoever asks - the core, a plugin - and refuses a
/// kind whose name another kind has already, since the name is what a kind
/// is kept, asked about and announced by (`EntityKind`).
#[derive(Debug, Clone)]
pub struct StoreBackend {
    store: Backend,
    /// The kinds opened, by name.
    kinds: Arc<Mutex<HashMap<EntityKind, TypeKey>>>,
}

#[derive(Debug, Clone)]
enum Backend {
    Memory(Arc<MemoryStore>),
    Json(Arc<JsonFileStore>),
}

impl StoreBackend {
    fn of(store: Backend) -> Self {
        Self { store, kinds: Default::default() }
    }

    /// The stores of the core's kinds.
    ///
    /// # Errors
    /// If one cannot be opened.
    pub fn entity_stores(&self) -> StoreResult<EntityStores> {
        EntityStores::open(self)
    }

    /// The store of the preconfigured components, kept where the entities
    /// are.
    pub fn component_store(&self) -> Arc<dyn ComponentStore> {
        match &self.store {
            Backend::Memory(store) => Arc::<MemoryStore>::clone(store),
            Backend::Json(store) => Arc::<JsonFileStore>::clone(store),
        }
    }

    /// The store of the states.
    pub fn state_store(&self) -> Arc<dyn StateStore> {
        match &self.store {
            Backend::Memory(store) => Arc::<MemoryStore>::clone(store),
            Backend::Json(store) => Arc::<JsonFileStore>::clone(store),
        }
    }

    /// The read side of the histories the states hold (issue 379), kept
    /// where the states are.
    pub fn history_store(&self) -> Arc<dyn HistoryStore> {
        match &self.store {
            Backend::Memory(store) => Arc::<MemoryStore>::clone(store),
            Backend::Json(store) => Arc::<JsonFileStore>::clone(store),
        }
    }
}

impl From<Arc<MemoryStore>> for StoreBackend {
    fn from(store: Arc<MemoryStore>) -> Self {
        Self::of(Backend::Memory(store))
    }
}

impl From<Arc<JsonFileStore>> for StoreBackend {
    fn from(store: Arc<JsonFileStore>) -> Self {
        Self::of(Backend::Json(store))
    }
}

impl EntityStoreFactory for StoreBackend {
    type Error = StoreError;

    /// # Errors
    /// `StoreError::Conflict` if another kind has the name of `T`; whatever
    /// opening it fails with.
    fn entity_store<T: Deletable>(&self) -> StoreResult<Arc<dyn EntityStore<T>>> {
        let kind = TypeKey::of::<T>();
        {
            let mut kinds = self.kinds.lock().unwrap_or_else(PoisonError::into_inner);
            let named = *kinds.entry(T::KIND).or_insert(kind);
            if named.id != kind.id {
                return Err(StoreError::Conflict(format!("the kinds {} and {} are both named '{}'", named.name(), kind.name(), T::KIND)));
            }
        }
        match &self.store {
            Backend::Memory(store) => store.entity_store().map_err(|never| match never {}),
            Backend::Json(store) => store.entity_store(),
        }
    }
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
    use crate::confidentiality::ConfidentialityRules;
    use crate::removal::Removal;
    use crate::security::AllowAllPolicy;
    use crate::store::memory::MemoryStore;
    use std::sync::Arc;
    use uuid::Uuid;
    use crate::value::name::Name;


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
            name: Name::new("session1").unwrap(),
            participants: Vec::new(),
            channels: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
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
            name: Name::new("session2").unwrap(),
            participants: Vec::new(),
            channels: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
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

    define_id!(NoteId);

    /// A kind of a plugin's (issue 323).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct Note {
        id: NoteId,
        security_attributes: SecurityAttributes,
        deleted: Option<Removal>,
    }

    impl Secured for Note {
        fn security_attributes(&self) -> &SecurityAttributes {
            &self.security_attributes
        }
    }

    impl Stored for Note {
        const KIND: EntityKind = EntityKind::new("note");
        type Id = NoteId;
        type Within = NoEntity;
        type Field = NoField;

        fn id(&self) -> NoteId {
            self.id
        }

        fn deleted(&self) -> Option<&Removal> {
            self.deleted.as_ref()
        }
    }

    impl Deletable for Note {
        fn set_deleted(&mut self, deleted: Option<Removal>) {
            self.deleted = deleted;
        }
    }

    /// A kind named as the core's groups are.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct Impostor(Note);

    impl Secured for Impostor {
        fn security_attributes(&self) -> &SecurityAttributes {
            &self.0.security_attributes
        }
    }

    impl Stored for Impostor {
        const KIND: EntityKind = Group::KIND;
        type Id = NoteId;
        type Within = NoEntity;
        type Field = NoField;

        fn id(&self) -> NoteId {
            self.0.id
        }
    }

    impl Deletable for Impostor {
        fn set_deleted(&mut self, _deleted: Option<Removal>) {}
    }

    #[tokio::test]
    async fn a_backend_opens_a_kind_once_for_everybody_and_no_two_kinds_of_one_name() {
        let memory = Arc::new(MemoryStore::new());
        let backend = StoreBackend::from(Arc::clone(&memory));
        let notes = backend.entity_store::<Note>().unwrap();
        let note = Note { id: NoteId::new(), security_attributes: SecurityAttributes::default(), deleted: None };
        notes.create(&SecurityContext::system(), &note).await.unwrap();
        let again = backend.entity_store::<Note>().unwrap();
        assert!(again.get(&SecurityContext::system(), note.id).await.is_ok(), "the same store, asked again");
        let core = backend.entity_stores().unwrap();
        assert!(Arc::ptr_eq(&core.groups, &memory.entity_stores().groups), "the core's kinds, too");
        let refused = backend.entity_store::<Impostor>().expect_err("named as the groups are");
        assert!(matches!(&refused, StoreError::Conflict(why) if why.contains("'group'")), "{refused}");

        let dir = tempfile::tempdir().unwrap();
        let json = StoreBackend::from(Arc::new(JsonFileStore::new(dir.path(), Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default())).unwrap()));
        json.entity_store::<Note>().unwrap().create(&SecurityContext::system(), &note).await.unwrap();
        assert!(dir.path().join("entities/note").join(format!("{}.json", note.id)).is_file(), "kept under its name");
    }
}