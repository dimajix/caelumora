//! What tests store their world by, and stand-ins for the stores.
//!
//! `stored_agent` and `record_users` store an agent with its personality and users, by the system,
//! for a test that reads them. `ContendedStore` stands in for a storage that another writer got to
//! first (issue 492): stores in memory whose documents of one kind refuse to be written as many
//! times as a test says.
#![expect(clippy::expect_used, reason = "test code: what fails here fails the test that runs it")]

use crate::documents::{Document, DocumentOf, DocumentStorage};
use crate::history::HistoryStore;
use crate::memory::MemoryStore;
use crate::transcript::TranscriptStore;
use crate::{EntityStores, Persistence, PersistenceBackend, Presence, StateStore};
use async_trait::async_trait;
use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::{Agent, Personality};
use caelumora_domain::channel::Participant;
use caelumora_domain::scope::Scope;
use caelumora_domain::store::query::{FieldPath, Filter};
use caelumora_domain::store::{EntityKind, Revision, StoreError, StoreResult};
use caelumora_domain::user::User;
use caelumora_domain::value::name::ScopedName;
use caelumora_security::{Admission, SecurityContext};
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use uuid::Uuid;

/// A backend in memory whose documents of one kind refuse a write as many times as the count of
/// refusals says, counted down with every refusal: a storage another writer got to between a read
/// and the write. A write in place of one - a change, a mark, a restoring, a removal - is refused
/// as outdated (`StoreError::Outdated`), a creation as taken (`StoreError::Conflict`), the document
/// made as the other writer made it (issue 500). Otherwise a `MemoryStore`.
#[derive(Debug)]
pub struct ContendedStore {
    inner: Arc<MemoryStore>,
    kind: EntityKind,
    refusals: Arc<AtomicUsize>,
}

impl ContendedStore {
    /// The stores over such a backend of `kind`, and the count of the refusals to come - none until
    /// a test sets it; a test reads it back to see that they were given.
    pub fn stores(kind: EntityKind) -> (Persistence, Arc<AtomicUsize>) {
        let refusals = Arc::new(AtomicUsize::new(0));
        let backend =
            Self { inner: Arc::new(MemoryStore::new()), kind, refusals: Arc::clone(&refusals) };
        let stores = Persistence::over(Arc::new(backend)).expect("stores in memory");
        (stores, refusals)
    }
}

impl PersistenceBackend for ContendedStore {
    fn documents(
        &self,
        kind: EntityKind,
        document_of: DocumentOf,
    ) -> StoreResult<Arc<dyn DocumentStorage>> {
        let documents = self.inner.documents(kind, document_of)?;
        if kind != self.kind {
            return Ok(documents);
        }
        Ok(Arc::new(Contended { inner: documents, kind, refusals: Arc::clone(&self.refusals) }))
    }

    fn state_store(self: Arc<Self>) -> StoreResult<Arc<dyn StateStore>> {
        Arc::clone(&self.inner).state_store()
    }

    fn history_store(self: Arc<Self>) -> StoreResult<Arc<dyn HistoryStore>> {
        Arc::clone(&self.inner).history_store()
    }

    fn transcript_store(self: Arc<Self>) -> StoreResult<Arc<dyn TranscriptStore>> {
        Arc::clone(&self.inner).transcript_store()
    }
}

/// The documents of the contended kind: `insert`, `replace` and `remove` refused while refusals
/// remain.
#[derive(Debug)]
struct Contended {
    inner: Arc<dyn DocumentStorage>,
    kind: EntityKind,
    refusals: Arc<AtomicUsize>,
}

impl Contended {
    /// Whether a refusal remains, counting it down.
    fn refused(&self) -> bool {
        self.refusals.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok()
    }

    /// Refuses the write of the document `id` in place of one if a refusal remains.
    fn contend(&self, id: Uuid) -> StoreResult<()> {
        if self.refused() {
            return Err(StoreError::Outdated { entity: self.kind.name(), id: id.to_string() });
        }
        Ok(())
    }
}

#[async_trait]
impl DocumentStorage for Contended {
    async fn get(&self, id: Uuid) -> StoreResult<Option<Value>> {
        self.inner.get(id).await
    }

    async fn scan(
        &self,
        filter: Option<&Filter<FieldPath>>,
        admission: Option<&Admission>,
        presence: Presence,
        after: Option<Uuid>,
        limit: usize,
    ) -> StoreResult<Vec<(Uuid, Value)>> {
        self.inner.scan(filter, admission, presence, after, limit).await
    }

    async fn named(&self, name: &ScopedName) -> StoreResult<Option<Value>> {
        self.inner.named(name).await
    }

    async fn insert(&self, document: Document) -> StoreResult<()> {
        if !self.refused() {
            return self.inner.insert(document).await;
        }
        let id = document.id;
        self.inner.insert(document).await?;
        Err(StoreError::Conflict(format!("{} {id} already exists", self.kind)))
    }

    async fn replace(&self, document: Document, read: Revision) -> StoreResult<()> {
        self.contend(document.id)?;
        self.inner.replace(document, read).await
    }

    async fn remove(&self, id: Uuid, read: Revision) -> StoreResult<bool> {
        self.contend(id)?;
        self.inner.remove(id, read).await
    }
}

/// An agent `name` of the global scope, of `owner`, and its `personality`, both stored in `stores`
/// by the system.
pub async fn stored_agent(
    stores: &EntityStores,
    name: &str,
    mut personality: Personality,
    owner: ActorId,
) -> Agent {
    let system = SecurityContext::system();
    stores.personalities.create(&system, &mut personality).await.expect("a personality stored");
    let mut agent = Agent::new(
        ScopedName::new(name, Scope::GlobalScope).expect("a test's name of an agent is one"),
        personality.id,
        owner,
    );
    stores.agents.create(&system, &mut agent).await.expect("an agent stored");
    agent
}

/// Stores copies of `users` in `stores` by the system, and answers them as participants of a
/// channel. The users given stay copies never stored, at the first revision.
pub async fn record_users(stores: &EntityStores, users: &[&User]) -> Vec<Participant> {
    let system = SecurityContext::system();
    let mut participants = Vec::new();
    for user in users {
        stores.users.create(&system, &mut (*user).clone()).await.expect("a user stored");
        participants.push(Participant::User(user.id));
    }
    participants
}
