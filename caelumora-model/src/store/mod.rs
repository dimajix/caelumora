//! Where what outlasts a run is kept (issues 93, 323): the entities of every kind behind one
//! checked interface (`EntityStore`), opened from a backend (`Persistence`), and the state, the
//! histories and the transcripts of what runs (`StateStore`). A stored entity is deleted by a mark
//! (`Removal`, issue 164), which the store hands out with what was deleted (`Deleted`).

pub(crate) mod access;
pub mod documents;
pub mod history;
pub mod kept_chain;
pub mod memberships;
pub mod memory;
pub mod query;
#[cfg(any(test, feature = "test-support"))]
pub mod suite;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod transcript;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;

use crate::agent::{Agent, AgentChannelState, AgentId, AgentState, Personality};
use crate::channel::{Channel, ChannelId, ChannelState};
use crate::component::ComponentBundle;
use crate::confidentiality::Classification;
use crate::group::Group;
use crate::intern::intern;
use crate::project::Project;
use crate::removal::{Removal, RemovalId};
use crate::security::{Policies, Refused, Secured, SecurityContext, Verb};
use crate::session::Session;
use crate::store::documents::{CheckedStore, Document, DocumentOf, DocumentStorage};
use crate::store::history::HistoryStore;
use crate::store::memory::MemoryStore;
use crate::store::query::{Field, Page, Query};
use crate::store::transcript::TranscriptStore;
use crate::template::SessionTemplate;
use crate::type_map::TypeKey;
use crate::user::{User, UserChannelState, UserId, UserProfile};
use crate::value::name::ScopedName;
use crate::value::ValueError;
use crate::workspace::Workspace;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::fmt;
use std::ptr;
use std::sync::{Mutex, PoisonError};

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
    /// Written on a revision that is no longer the stored one (issue 492): something changed
    /// it meanwhile, or purged it. Whoever wrote reads it again and makes their change once
    /// more.
    #[error("the {entity} {id} was changed meanwhile")]
    Outdated { entity: &'static str, id: String },
}

impl StoreError {
    /// Whether it says that what was asked for is not there, or refused to the caller (`NotFound`,
    /// `PermissionDenied`), rather than that the store failed or the request did not fit.
    pub fn is_absent_or_refused(&self) -> bool {
        matches!(self, Self::NotFound { .. } | Self::PermissionDenied(_))
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

/// What a lookup answers when what it looks for may no longer exist (issue 192), and what a write
/// answers when it may have been refused as outdated (issue 492).
pub trait StoreResultExt<T> {
    /// What was found, `None` if it is not there (`StoreError::NotFound`);
    /// any other error stays one.
    fn optional(self) -> StoreResult<Option<T>>;

    /// What a write answered, `None` if it was refused as outdated (`StoreError::Outdated`):
    /// whoever wrote reads again and makes the change once more. Any other error stays one.
    fn written(self) -> StoreResult<Option<T>>;

    /// What a creation answered, `None` if the id was taken (`StoreError::Conflict`) - made
    /// meanwhile elsewhere, or there before, deleted or not. Any other error stays one.
    fn created(self) -> StoreResult<Option<T>>;
}

impl<T> StoreResultExt<T> for StoreResult<T> {
    fn optional(self) -> StoreResult<Option<T>> {
        match self {
            Ok(found) => Ok(Some(found)),
            Err(StoreError::NotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn written(self) -> StoreResult<Option<T>> {
        match self {
            Ok(written) => Ok(Some(written)),
            Err(StoreError::Outdated { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn created(self) -> StoreResult<Option<T>> {
        match self {
            Ok(created) => Ok(Some(created)),
            Err(StoreError::Conflict(_)) => Ok(None),
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

/// A kind of entity the stores keep: what the actions that concern every
/// kind alike are keyed by, and its name in errors. Open
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
        self.name()
            .split('_')
            .map(|word| {
                let mut letters = word.chars();
                letters
                    .next()
                    .map(|first| first.to_ascii_uppercase().to_string() + letters.as_str())
                    .unwrap_or_default()
            })
            .collect()
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

/// The revision of a stored entity (issue 492): counted up by every write of it, so that a write
/// made on an outdated one is refused (`StoreError::Outdated`) instead of overwriting what came
/// between - by every storage, in the step that writes, so that it holds against every writer of
/// the storage, also of another process. A new entity carries the first, which no stored one has:
/// it is created as the one after it (issue 500), so that a copy never stored is never taken for
/// one read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision(u64);

impl Revision {
    /// The revision after this one: what a write of an entity at this one is written as.
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }

    /// Its number: the first, that of an entity never stored, is 0.
    pub fn number(self) -> u64 {
        self.0
    }
}

/// An entity the `EntityStore` keeps: what the stores are generic over, besides `Secured`. Every
/// kind is marked when it is deleted, not removed (issue 164): what holds content, an identity, or
/// a definition that is managed. A store keeps it as it is serialized, and hands out copies, so
/// that a backend can keep any kind (issue 323). Every kind carries its revision (issue 492), by
/// which a write of it is judged.
pub trait Stored: Secured + Clone + Serialize + DeserializeOwned + Send + Sync + 'static {
    /// Its kind (issue 323), by which what is done with it is asked and
    /// named.
    const KIND: EntityKind;

    /// What it is kept and found by: a UUID under a type of its kind's
    /// (`define_id!`), so that every store keys every kind alike.
    type Id: Clone
        + Eq
        + Hash
        + Display
        + FromStr
        + Debug
        + Send
        + Sync
        + From<Uuid>
        + Into<Uuid>
        + 'static;

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

    /// Whether, and by whom and when, it was deleted (issue 164).
    fn deleted(&self) -> Option<&Removal>;

    /// Marks it deleted, or with `None` takes the mark away.
    fn set_deleted(&mut self, deleted: Option<Removal>);

    /// Its revision (issue 492): as it was read, what a write of it is judged against - the first
    /// if it was never stored -; once the store wrote it, the one it was written as.
    fn revision(&self) -> Revision;

    /// Sets its revision: the store's, as it writes it.
    fn set_revision(&mut self, revision: Revision);

    /// Whether its purge has begun (issue 492): what lies behind it is being removed, and no
    /// restoring takes it back any more. Begun today only for what was deleted, which then stays
    /// deleted until it is purged.
    fn purging(&self) -> bool;

    /// Marks its purge begun: the store's, as it begins it (`EntityStore::begin_purge`).
    fn set_purging(&mut self);

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

/// Which of a kind's entities an operation of a store is about (issue 459): those there, or those
/// deleted - kept marked so that it can be told later that they were there, who deleted them and
/// when (issue 164) -, whichever deletion or with one (issue 478). Reading the deleted is asked by
/// a verb of its own (`Presence::verb`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Present,
    Deleted,
    /// Those deleted with the deletion of this id: what went with it (issue 469).
    DeletedWith(RemovalId),
}

impl Presence {
    /// Whether `value` is among them, by its mark (`Stored::deleted`).
    pub(crate) fn holds<T: Stored>(self, value: &T) -> bool {
        self.holds_mark(value.deleted().map(|removal| removal.id))
    }

    /// Whether a document with the mark of the deletion `removal_id` - `None` if it is there - is
    /// among them: what a storage compares the marks of its documents with
    /// (`Document::removal_id`).
    pub(crate) fn holds_mark(self, removal_id: Option<RemovalId>) -> bool {
        removal_id.is_some() == self.marked()
            && self.removal_id().is_none_or(|id| removal_id == Some(id))
    }

    /// Whether they carry the mark of a deletion, whichever: what a storage that compares the mark
    /// in parts compares first, and `removal_id` next.
    pub fn marked(self) -> bool {
        self != Self::Present
    }

    /// The deletion whose mark they carry, if they are those of one.
    pub fn removal_id(self) -> Option<RemovalId> {
        match self {
            Self::DeletedWith(id) => Some(id),
            Self::Present | Self::Deleted => None,
        }
    }

    /// What reading one of them asks: `Get`, or `ReadDeleted`.
    pub(crate) fn verb(self) -> Verb {
        match self {
            Self::Present => Verb::Get,
            Self::Deleted | Self::DeletedWith(_) => Verb::ReadDeleted,
        }
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

/// Something deleted as the store hands it out (issues 164, 472): the entity as it is stored, and
/// the mark it is sure to carry - which deletion, who and when -, and so what went with it (issues
/// 469, 478).
/// The entity carries the mark as well, as it is stored; `removal` is the one to read, which
/// needs no check.
#[derive(Debug, Clone, PartialEq)]
pub struct Deleted<T> {
    pub entity: T,
    pub removal: Removal,
}

impl<T: Stored> Deleted<T> {
    /// `entity` as deleted, if it carries the mark.
    pub(crate) fn of(entity: T) -> Option<Self> {
        let removal = entity.deleted()?.clone();
        Some(Self { entity, removal })
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
/// Deleting marks (issue 164): `delete` sets the mark it is given - which deletion, who, when - and
/// keeps the entity. A deletion makes its mark once and gives it to everything that goes with what
/// it deletes (issue 469); the mark names the caller, another only if the caller is the system
/// (`Validation`). What is marked is not there for `get`, `list`, `update` and `delete`, also not
/// for the system, and its id is not free for `create`; nothing new or changed may carry the mark
/// (`Validation`). What was deleted is reached by operations of its own: `get_deleted` and
/// `list_deleted` read it (`ReadDeleted`), each with its mark (`Deleted`), and `list_deleted_with`
/// what was deleted with one deletion (issue 478); `restore` takes the mark away (`Restore`) and
/// returns what it restored, and `purge` removes what was deleted for good (`Purge`) - only what
/// is marked, so that a purge never takes what was restored. Where something lies behind an
/// entity - a workspace's place, an artifact's content -, its purge is begun first
/// (`begin_purge`): from then on no restoring takes it back, while what lies behind it is removed
/// before `purge` (issue 492). Such a purge goes through `purge_behind`, which keeps that order.
///
/// Every write is judged on the revision that was read (issue 492), and writes the one after it:
/// `create` a new entity, which carries the first, as the one after it (issue 500); `update`
/// writes `value` in place of the revision it carries, and `Outdated` says that it is no longer
/// the stored one - the caller reads again and changes again. A copy never stored is so never
/// written in place of one created meanwhile. `delete`, `restore` and `purge` read what they
/// write themselves, and do so again if it was changed meanwhile; no caller sees that.
#[async_trait]
pub trait EntityStore<T: Stored>: Debug + Send + Sync {
    /// Stores the new entity `value` as the revision after the one it carries; `value` then
    /// carries the one it was written as.
    async fn create(&self, ctx: &SecurityContext, value: &mut T) -> StoreResult<()>;
    async fn get(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T>;
    /// Changes the entity into `value`, which carries the revision it was read at; `value` then
    /// carries the one it was written as. `Outdated` if it is no longer the stored one.
    async fn update(&self, ctx: &SecurityContext, value: &mut T) -> StoreResult<()>;
    async fn delete(
        &self,
        ctx: &SecurityContext,
        id: T::Id,
        removal: &Removal,
    ) -> StoreResult<bool>;
    async fn list(&self, ctx: &SecurityContext, query: &Query<T::Field>) -> StoreResult<Page<T>>;
    async fn restore(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T>;
    /// Begins the purge of the deleted entity `id`, asked as `purge` asks: it says so from then
    /// on (`Stored::purging`), and `restore` refuses it, so that what lies behind it can be
    /// removed before `purge` removes it - and again, if that failed, by a purge begun once more.
    /// Answers the entity with its purge begun; `None` if there is nothing deleted the caller could
    /// see.
    async fn begin_purge(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<Option<T>>;
    async fn purge(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<bool>;
    async fn get_deleted(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<Deleted<T>>;
    async fn list_deleted(
        &self,
        ctx: &SecurityContext,
        query: &Query<T::Field>,
    ) -> StoreResult<Page<Deleted<T>>>;
    /// Those meeting `query` that were deleted with the deletion `removal_id` - that carry its
    /// mark -, read as `list_deleted` reads.
    async fn list_deleted_with(
        &self,
        ctx: &SecurityContext,
        removal_id: RemovalId,
        query: &Query<T::Field>,
    ) -> StoreResult<Page<T>>;
    /// The one of `name`, in exactly that scope, among what the caller may
    /// read (issue 178).
    async fn find_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<T>;
}

/// Purges the deleted entity `id` of `store` on behalf of `ctx`, what lies behind it first: its
/// purge begun (`EntityStore::begin_purge`), so that no restoring takes it back while that goes
/// (issue 492), then `remove_behind` given the entity, then the entity. The one order of every
/// purge of what has something behind it (issue 497); each caller answers in its own errors.
/// `false` if there was nothing deleted the caller could see.
///
/// # Errors
/// What `remove_behind` reports, or whatever the store reports; if it fails once the purge is
/// begun, the entity stays deleted, its purge begun, so that the next purge tries again.
pub async fn purge_behind<T: Stored, E: From<StoreError>>(
    store: &dyn EntityStore<T>,
    ctx: &SecurityContext,
    id: T::Id,
    remove_behind: impl AsyncFnOnce(&T) -> Result<(), E>,
) -> Result<bool, E> {
    let Some(entity) = store.begin_purge(ctx, id.clone()).await? else { return Ok(false) };
    remove_behind(&entity).await?;
    Ok(store.purge(ctx, id).await?)
}

/// The stores of the entities (issue 323), one per kind of the core,
/// which the runtime is given and hands to the managers that keep each
/// kind. A plugin's kinds are not among them: it asks the builder for their
/// stores itself (`RuntimeBuilder::entity_store`).
#[derive(Debug, Clone)]
pub struct EntityStores {
    pub workspaces: Arc<dyn EntityStore<Workspace>>,
    pub groups: Arc<dyn EntityStore<Group>>,
    pub projects: Arc<dyn EntityStore<Project>>,
    pub session_templates: Arc<dyn EntityStore<SessionTemplate>>,
    pub user_profiles: Arc<dyn EntityStore<UserProfile>>,
    pub personalities: Arc<dyn EntityStore<Personality>>,
    pub users: Arc<dyn EntityStore<User>>,
    pub agents: Arc<dyn EntityStore<Agent>>,
    pub channels: Arc<dyn EntityStore<Channel>>,
    pub sessions: Arc<dyn EntityStore<Session>>,
    pub component_bundles: Arc<dyn EntityStore<ComponentBundle>>,
}

impl EntityStores {
    /// The stores of the core's kinds in `persistence`, every access checked by `policies`.
    ///
    /// # Errors
    /// If one cannot be opened.
    pub fn open(persistence: &Persistence, policies: &Policies) -> StoreResult<Self> {
        Ok(Self {
            workspaces: persistence.entity_store(policies)?,
            groups: persistence.entity_store(policies)?,
            projects: persistence.entity_store(policies)?,
            session_templates: persistence.entity_store(policies)?,
            user_profiles: persistence.entity_store(policies)?,
            personalities: persistence.entity_store(policies)?,
            users: persistence.entity_store(policies)?,
            agents: persistence.entity_store(policies)?,
            channels: persistence.entity_store(policies)?,
            sessions: persistence.entity_store(policies)?,
            component_bundles: persistence.entity_store(policies)?,
        })
    }
}

/// A backend of the stores (issues 446, 448) - the memory, the JSON files -:
/// where they keep what they keep, without asking anything - the documents
/// of every kind (`DocumentStorage`), the states and the histories and
/// transcripts they hold. It offers each store by a method of its own and is
/// none of them, so that a backend that keeps only the one part refuses the
/// other. The checks are the `Persistence`'s, which is made of a backend of
/// the entities and a backend of the states, and checks a store by the policies it is opened by.
#[async_trait]
pub trait PersistenceBackend: Debug + Send + Sync {
    /// The storage of the documents of `kind`, opened once - asked again,
    /// the same one, so that whoever writes a kind writes one storage.
    /// `document_of` tells a document of the kind as a storage that keeps
    /// bodies alone reads it back.
    ///
    /// # Errors
    /// If the storage cannot be opened, or the backend keeps no entities.
    fn documents(
        &self,
        kind: EntityKind,
        document_of: DocumentOf,
    ) -> StoreResult<Arc<dyn DocumentStorage>>;

    /// The store of the states - the backend itself, if it is one.
    ///
    /// # Errors
    /// If it cannot be opened, or the backend keeps no states.
    fn state_store(self: Arc<Self>) -> StoreResult<Arc<dyn StateStore>>;

    /// The read side of the histories the states hold (issue 379), which
    /// reads what the state store of the same backend writes.
    ///
    /// # Errors
    /// As `state_store`.
    fn history_store(self: Arc<Self>) -> StoreResult<Arc<dyn HistoryStore>>;

    /// The read side of the agents' transcripts the states hold (issue
    /// 380), which reads what the state store of the same backend writes.
    ///
    /// # Errors
    /// As `state_store`.
    fn transcript_store(self: Arc<Self>) -> StoreResult<Arc<dyn TranscriptStore>>;

    /// Makes the backend ready to be used (issue 447): once, at start,
    /// after every plugin registered - so after the kinds of the core and of
    /// the plugins were opened -, and before anything reads or writes. A
    /// database connects here, applies its migrations and makes what the
    /// kinds opened need, so that what is wrong with it fails the start; a
    /// kind opened later is the backend's to make ready when it is first
    /// used. Nothing, unless a backend says otherwise.
    ///
    /// # Errors
    /// If the backend cannot be made ready.
    async fn open(&self) -> StoreResult<()> {
        Ok(())
    }
}

/// The stores (issues 323, 446, 448, 449): of the entities of every kind and of the states, over
/// the backend of the entities and the backend of the states - one, or two. It holds no policies:
/// the store of a kind is checked by the policies whoever opens it gives, the same way for every
/// backend (`CheckedStore`) - the runtime's by its own. It refuses a kind whose name another kind
/// has already, since the name is what a kind is kept, asked about and announced by
/// (`EntityKind`). The three state stores are taken from the backend of the states when it is
/// made, together, since they read what one another writes.
#[derive(Debug, Clone)]
pub struct Persistence {
    /// What the entities are kept in.
    entities: Arc<dyn PersistenceBackend>,
    /// What the states are kept in - the same as the entities, or another.
    states: Arc<dyn PersistenceBackend>,
    state_store: Arc<dyn StateStore>,
    history_store: Arc<dyn HistoryStore>,
    transcript_store: Arc<dyn TranscriptStore>,
    /// The type of each kind opened, by its name.
    kinds: Arc<Mutex<HashMap<EntityKind, TypeKey>>>,
}

impl Persistence {
    /// The stores of the entities in `entities` and of the states in `states`.
    ///
    /// # Errors
    /// If `states` keeps no states, or its state stores cannot be opened.
    pub fn new(
        entities: Arc<dyn PersistenceBackend>,
        states: Arc<dyn PersistenceBackend>,
    ) -> StoreResult<Self> {
        Ok(Self {
            state_store: Arc::clone(&states).state_store()?,
            history_store: Arc::clone(&states).history_store()?,
            transcript_store: Arc::clone(&states).transcript_store()?,
            entities,
            states,
            kinds: Default::default(),
        })
    }

    /// The stores of the entities and of the states, both in `backend`.
    ///
    /// # Errors
    /// As `new`.
    pub fn over(backend: Arc<dyn PersistenceBackend>) -> StoreResult<Self> {
        Self::new(Arc::clone(&backend), backend)
    }

    /// Stores in memory, the entities and the states in one - for tests and throwaway setups.
    /// Unlike `over`, it cannot fail: the memory store is the three state stores itself.
    pub fn in_memory() -> Self {
        let memory = Arc::new(MemoryStore::new());
        Self {
            entities: Arc::<MemoryStore>::clone(&memory),
            states: Arc::<MemoryStore>::clone(&memory),
            state_store: Arc::<MemoryStore>::clone(&memory),
            history_store: Arc::<MemoryStore>::clone(&memory),
            transcript_store: memory,
            kinds: Default::default(),
        }
    }

    /// Makes its backends ready to be used (`PersistenceBackend::open`):
    /// once, at start, after every kind was opened - the backend of the
    /// states first, then that of the entities, if it is another instance.
    ///
    /// # Errors
    /// If a backend cannot be made ready.
    pub async fn open(&self) -> StoreResult<()> {
        self.states.open().await?;
        if !ptr::addr_eq(Arc::as_ptr(&self.entities), Arc::as_ptr(&self.states)) {
            self.entities.open().await?;
        }
        Ok(())
    }

    /// The store of the states.
    pub fn state_store(&self) -> Arc<dyn StateStore> {
        Arc::clone(&self.state_store)
    }

    /// The read side of the histories the states hold (issue 379), kept
    /// where the states are.
    pub fn history_store(&self) -> Arc<dyn HistoryStore> {
        Arc::clone(&self.history_store)
    }

    /// The read side of the agents' transcripts the states hold (issue
    /// 380), kept where the states are.
    pub fn transcript_store(&self) -> Arc<dyn TranscriptStore> {
        Arc::clone(&self.transcript_store)
    }

    /// The store of the kind `T`, every access checked by `policies`: over the one storage of the
    /// kind its backend keeps, whoever opens it, and by whichever policies.
    ///
    /// # Errors
    /// `StoreError::Conflict` if another kind has the name of `T`; whatever opening its storage
    /// fails with.
    pub fn entity_store<T: Stored>(
        &self,
        policies: &Policies,
    ) -> StoreResult<Arc<dyn EntityStore<T>>> {
        let kind = TypeKey::of::<T>();
        {
            let mut kinds = self.kinds.lock().unwrap_or_else(PoisonError::into_inner);
            let named = *kinds.entry(T::KIND).or_insert(kind);
            if named.id != kind.id {
                return Err(StoreError::Conflict(format!(
                    "the kinds {} and {} are both named '{}'",
                    named.name(),
                    kind.name(),
                    T::KIND
                )));
            }
        }
        let storage = self.entities.documents(T::KIND, Document::read::<T>)?;
        Ok(Arc::new(CheckedStore::<T>::new(storage, policies)))
    }
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
pub trait StateStore: Debug + Sync + Send {
    /// An agent in a channel, of either kind (issue 206).
    async fn get_agent_channel_state(
        &self,
        agent_id: AgentId,
        channel_id: ChannelId,
    ) -> StoreResult<AgentChannelState>;
    /// Stores the agent's state in the channel, and keeps the index of the
    /// work under way with it (issue 388): the agent is in it for the channel
    /// while the state stored has work under way
    /// (`AgentChannelState::has_work_under_way`).
    async fn put_agent_channel_state(
        &self,
        agent_id: AgentId,
        channel_id: ChannelId,
        state: &mut AgentChannelState,
    ) -> StoreResult<()>;

    /// The channels in which an agent's state stored has work under way
    /// (issue 388), each once, in no particular order: what a restart starts
    /// again. Read from the index, without loading a state.
    async fn channels_with_work_under_way(&self) -> StoreResult<Vec<ChannelId>>;

    async fn get_agent_state(&self, agent_id: AgentId) -> StoreResult<AgentState>;
    async fn put_agent_state(&self, agent_id: AgentId, state: &mut AgentState) -> StoreResult<()>;

    async fn get_channel_state(&self, channel_id: ChannelId) -> StoreResult<ChannelState>;
    async fn put_channel_state(
        &self,
        channel_id: ChannelId,
        state: &mut ChannelState,
    ) -> StoreResult<()>;

    /// A user in a channel (issue 404): up to where they read it. One never
    /// stored reads as the default, nothing read.
    async fn get_user_channel_state(
        &self,
        user_id: UserId,
        channel_id: ChannelId,
    ) -> StoreResult<UserChannelState>;
    async fn put_user_channel_state(
        &self,
        user_id: UserId,
        channel_id: ChannelId,
        state: &mut UserChannelState,
    ) -> StoreResult<()>;

    /// Deletes the state of a channel that no longer exists: its own and that
    /// of every agent and user in it. Nothing to delete is not an error
    /// (issue 09).
    async fn delete_channel_state(&self, channel_id: ChannelId) -> StoreResult<()>;

    /// Deletes the state of an agent purged for good (issue 164): its own
    /// and its state in every channel. Nothing to delete is not an error.
    async fn delete_agent_state(&self, agent_id: AgentId) -> StoreResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::ActorId;
    use crate::removal::Removal;
    use crate::security::{OwnershipPolicy, SecurityAttributes, SecurityContext};
    use crate::session::testing::bare_session;
    use crate::session::SessionId;
    use crate::store::memory::MemoryStore;
    use crate::store::query::NoField;
    use crate::value::name::Name;
    use std::ops::Not;
    use std::sync::Arc;

    #[test]
    fn what_is_not_found_is_none_and_every_other_error_stays_one() {
        let found: StoreResult<u32> = Ok(1);
        assert_eq!(found.optional().unwrap(), Some(1));
        let gone: StoreResult<u32> =
            Err(StoreError::NotFound { entity: "user", id: "1".to_string() });
        assert_eq!(gone.optional().unwrap(), None);
        let failed: StoreResult<u32> = Err(StoreError::Backend("disk".to_string()));
        assert!(matches!(failed.optional(), Err(StoreError::Backend(_))));
    }

    #[tokio::test]
    async fn list_sessions_works() {
        let mut session1 =
            Session { name: Name::new("session1").unwrap(), ..bare_session(SessionId::new()) };
        let mut session2 =
            Session { name: Name::new("session2").unwrap(), ..bare_session(SessionId::new()) };
        let store = EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all())
            .unwrap()
            .sessions;
        let ctx = SecurityContext::default();

        store.create(&ctx, &mut session1).await.unwrap();
        store.create(&ctx, &mut session2).await.unwrap();

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
        #[serde(default)]
        revision: Revision,
        #[serde(default, skip_serializing_if = "Not::not")]
        purging: bool,
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

        fn set_deleted(&mut self, deleted: Option<Removal>) {
            self.deleted = deleted;
        }

        fn revision(&self) -> Revision {
            self.revision
        }

        fn set_revision(&mut self, revision: Revision) {
            self.revision = revision;
        }

        fn purging(&self) -> bool {
            self.purging
        }

        fn set_purging(&mut self) {
            self.purging = true;
        }
    }

    /// A new note of the system's.
    fn new_note() -> Note {
        Note {
            id: NoteId::new(),
            security_attributes: SecurityAttributes::systems(),
            deleted: None,
            revision: Default::default(),
            purging: false,
        }
    }

    /// What a caller of `purge_behind` answers in: its own errors beside the store's.
    #[derive(Debug)]
    enum Behind {
        Unremovable,
        Store(
            #[expect(dead_code, reason = "shown by `Debug` alone, when a test fails")] StoreError,
        ),
    }

    impl From<StoreError> for Behind {
        fn from(error: StoreError) -> Self {
            Self::Store(error)
        }
    }

    /// Issue 497: a purge with something behind it removes that between beginning the purge and
    /// purging; if removing fails, the caller's error comes back as it was given, and the entity
    /// stays deleted, its purge begun, so that no restoring takes it back.
    #[tokio::test]
    async fn what_lies_behind_goes_between_beginning_the_purge_and_purging() {
        let notes = Persistence::in_memory()
            .entity_store::<Note>(&Policies::permitting_all())
            .expect("the store of notes opens");
        let system = SecurityContext::system();
        let mut note = new_note();
        notes.create(&system, &mut note).await.unwrap();
        let removed = Mutex::new(Vec::new());
        let remove = async |behind: &Note| {
            removed.lock().unwrap().push(behind.id);
            Ok::<_, Behind>(())
        };

        assert!(!purge_behind(notes.as_ref(), &system, note.id, remove).await.unwrap());
        assert!(removed.lock().unwrap().is_empty(), "nothing removed for what is not deleted");

        notes.delete(&system, note.id, &Removal::now(ActorId::System)).await.unwrap();
        let failed =
            purge_behind(notes.as_ref(), &system, note.id, async |_| Err(Behind::Unremovable))
                .await;
        assert!(matches!(failed, Err(Behind::Unremovable)), "the caller's error: {failed:?}");
        let left = notes.get_deleted(&system, note.id).await.expect("still deleted");
        assert!(left.entity.purging(), "its purge begun");
        assert!(notes.restore(&system, note.id).await.is_err(), "not to be restored");

        let remove = async |behind: &Note| {
            assert!(behind.purging(), "removed once its purge is begun");
            removed.lock().unwrap().push(behind.id);
            Ok::<_, Behind>(())
        };
        assert!(purge_behind(notes.as_ref(), &system, note.id, remove).await.unwrap());
        assert_eq!(*removed.lock().unwrap(), vec![note.id], "what lies behind it, removed");
        let gone = notes.get_deleted(&system, note.id).await;
        assert!(matches!(gone, Err(StoreError::NotFound { .. })), "purged: {gone:?}");
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

        fn deleted(&self) -> Option<&Removal> {
            self.0.deleted()
        }

        fn set_deleted(&mut self, deleted: Option<Removal>) {
            self.0.set_deleted(deleted);
        }

        fn revision(&self) -> Revision {
            self.0.revision()
        }

        fn set_revision(&mut self, revision: Revision) {
            self.0.set_revision(revision);
        }

        fn purging(&self) -> bool {
            self.0.purging()
        }

        fn set_purging(&mut self) {
            self.0.set_purging();
        }
    }

    /// A kind is kept in one storage of its backend however it is opened, by whichever policies
    /// (issue 449), and no two kinds share a name.
    #[tokio::test]
    async fn a_kind_is_kept_in_one_storage_however_it_is_opened_and_no_two_kinds_of_one_name() {
        let memory = Arc::new(MemoryStore::new());
        let backend = Persistence::over(Arc::<MemoryStore>::clone(&memory)).unwrap();
        let notes = backend.entity_store::<Note>(&Policies::permitting_all()).unwrap();
        let mut note = new_note();
        notes.create(&SecurityContext::system(), &mut note).await.unwrap();
        let owners = Policies::by_default_rules(Arc::new(OwnershipPolicy::new()));
        let again = backend.entity_store::<Note>(&owners).unwrap();
        assert!(
            again.get(&SecurityContext::system(), note.id).await.is_ok(),
            "the same storage, opened again by other policies"
        );
        EntityStores::open(&backend, &Policies::permitting_all()).unwrap();
        let refused = backend
            .entity_store::<Impostor>(&Policies::permitting_all())
            .expect_err("named as the groups are");
        assert!(
            matches!(&refused, StoreError::Conflict(why) if why.contains("'group'")),
            "{refused}"
        );
        let other = Persistence::over(memory).unwrap();
        assert!(
            other
                .entity_store::<Note>(&Policies::permitting_all())
                .unwrap()
                .get(&SecurityContext::system(), note.id)
                .await
                .is_ok(),
            "one storage of a kind in a backend"
        );
    }
}
