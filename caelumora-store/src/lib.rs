//! The stores (issues 93, 323, 517): the entities of every kind behind one checked interface
//! (`EntityStore`), opened from a backend (`Persistence`), and the state, the histories and the
//! transcripts of what runs (`StateStore`). A stored entity is deleted by a mark (`Removal`, issue
//! 164), which the store hands out with what was deleted (`Deleted`). What an entity is to be
//! stored and how a store fails (`caelumora_domain::store`), and what a query of a list asks
//! (`caelumora_domain::query`), are the domain's.

pub(crate) mod access;
pub mod documents;
pub mod history;
pub mod kept_chain;
pub mod memberships;
pub mod memory;
#[cfg(any(test, feature = "test-support"))]
pub mod suite;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod transcript;

use crate::documents::{CheckedStore, Document, DocumentOf, DocumentStorage};
use crate::history::HistoryStore;
use crate::memory::MemoryStore;
use crate::transcript::TranscriptStore;
use async_trait::async_trait;
use caelumora_domain::agent::{Agent, AgentChannelState, AgentId, Personality};
use caelumora_domain::channel::{Channel, ChannelId, ChannelState};
use caelumora_domain::component::ComponentBundle;
use caelumora_domain::group::Group;
use caelumora_domain::project::Project;
use caelumora_domain::query::{Page, Query};
use caelumora_domain::removal::{Removal, RemovalId};
use caelumora_domain::session::Session;
use caelumora_domain::store::{EntityKind, StoreError, StoreResult, Stored};
use caelumora_domain::template::SessionTemplate;
use caelumora_domain::type_map::TypeKey;
use caelumora_domain::user::{User, UserChannelState, UserId, UserProfile};
use caelumora_domain::value::name::ScopedName;
use caelumora_domain::workspace::Workspace;
use caelumora_security::{Policies, SecurityContext, Verb};
use std::collections::HashMap;
use std::fmt::Debug;
use std::ptr;
use std::sync::Arc;
use std::sync::{Mutex, PoisonError};

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
/// operations, checked the same way (`access`), and is kept by a
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

/// The stores of the entities (issue 323), one per kind of the kernel, which the kernel is given
/// and hands to the managers that keep each kind. A plugin's kinds are not among them: it asks the
/// builder for their stores itself (`KernelBuilder::entity_store`).
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
    /// The stores of the kernel's kinds in `persistence`, every access checked by `policies`.
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

    /// Makes the backend ready to be used (issue 447): once, at start, after every plugin
    /// registered - so after the kinds of the kernel and of the plugins were opened -, and before
    /// anything reads or writes. A database connects here, applies its migrations and makes what
    /// the kinds opened need, so that what is wrong with it fails the start; a kind opened later is
    /// the backend's to make ready when it is first used. Nothing, unless a backend says otherwise.
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
/// backend (`CheckedStore`) - the kernel's by its own. It refuses a kind whose name another kind
/// has already, since the name is what a kind is kept, asked about and announced by (`EntityKind`).
/// The three state stores are taken from the backend of the states when it is made, together, since
/// they read what one another writes.
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

    /// Deletes the states of an agent purged for good (issue 164): its state
    /// in every channel, with its transcripts and what was given to it there.
    /// Nothing to delete is not an error.
    async fn delete_agent_channel_states(&self, agent_id: AgentId) -> StoreResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryStore;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::define_id;
    use caelumora_domain::query::NoField;
    use caelumora_domain::removal::Removal;
    use caelumora_domain::security::{Secured, SecurityAttributes};
    use caelumora_domain::session::testing::bare_session;
    use caelumora_domain::session::SessionId;
    use caelumora_domain::store::{NoEntity, Revision};
    use caelumora_domain::value::name::Name;
    use caelumora_security::{OwnershipPolicy, SecurityContext};
    use serde::{Deserialize, Serialize};
    use std::ops::Not;
    use std::sync::Arc;

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

    /// A kind named as the kernel's groups are.
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
