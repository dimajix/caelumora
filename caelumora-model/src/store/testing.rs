//! A stand-in for tests of what writes again when its write was refused as outdated (issue 492):
//! stores in memory whose documents of one kind refuse to be written in place of as many times as
//! a test says.
#![expect(clippy::expect_used, reason = "test code: what fails here fails the test that runs it")]

use crate::confidentiality::ConfidentialityRules;
use crate::security::{Admission, AllowAllPolicy};
use crate::store::documents::{Document, DocumentOf, DocumentStorage};
use crate::store::history::HistoryStore;
use crate::store::memory::MemoryStore;
use crate::store::query::{FieldPath, Filter};
use crate::store::transcript::TranscriptStore;
use crate::store::{
    EntityKind, Persistence, PersistenceBackend, Presence, Revision, StateStore, StoreError,
    StoreResult,
};
use crate::value::name::ScopedName;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use uuid::Uuid;

/// A backend in memory whose documents of one kind refuse a write in place of one - a change, a
/// mark, a restoring, a removal - as outdated (`StoreError::Outdated`) as many times as the count
/// of refusals says, counted down with every refusal: a storage another writer got to between a
/// read and the write. Otherwise a `MemoryStore`.
#[derive(Debug)]
pub struct ContendedStore {
    inner: Arc<MemoryStore>,
    kind: EntityKind,
    refusals: Arc<AtomicUsize>,
}

impl ContendedStore {
    /// The stores over such a backend of `kind`, permitting everything, and the count of the
    /// refusals to come - none until a test sets it; a test reads it back to see that they were
    /// given.
    pub fn stores(kind: EntityKind) -> (Persistence, Arc<AtomicUsize>) {
        let refusals = Arc::new(AtomicUsize::new(0));
        let backend =
            Self { inner: Arc::new(MemoryStore::new()), kind, refusals: Arc::clone(&refusals) };
        let stores = Persistence::over(
            Arc::new(backend),
            Arc::new(AllowAllPolicy::new()),
            Arc::new(ConfidentialityRules::default()),
        )
        .expect("stores in memory");
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

/// The documents of the contended kind: `replace` and `remove` refused while refusals remain.
#[derive(Debug)]
struct Contended {
    inner: Arc<dyn DocumentStorage>,
    kind: EntityKind,
    refusals: Arc<AtomicUsize>,
}

impl Contended {
    /// Refuses the write of the document `id` if a refusal remains, and counts it down.
    fn contend(&self, id: Uuid) -> StoreResult<()> {
        match self.refusals.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)) {
            Ok(_) => Err(StoreError::Outdated { entity: self.kind.name(), id: id.to_string() }),
            Err(_) => Ok(()),
        }
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
        self.inner.insert(document).await
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
