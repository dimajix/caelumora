//! What tests open the stores by, stand-ins for them, and checks of what they keep.
//!
//! `Policies::permitting_all` opens the stores for what a test writes before it runs what it tests,
//! `Policies::by_default_rules` for a test of an access policy (issue 449). `ContendedStore` stands
//! in for a storage that another writer got to first (issue 492): stores in memory whose documents
//! of one kind refuse to be written as many times as a test says. `stored` and `replayed` check a
//! journaled state (`Journaled`): as a store holds it, and what its changes replay onto.
#![expect(clippy::expect_used, reason = "test code: what fails here fails the test that runs it")]

use crate::confidentiality::ConfidentialityRules;
use crate::security::{AccessPolicy, Admission, AllowAllPolicy, Policies};
use crate::store::documents::{Document, DocumentOf, DocumentStorage};
use crate::store::history::HistoryStore;
use crate::store::memory::MemoryStore;
use crate::store::query::{FieldPath, Filter};
use crate::store::transcript::TranscriptStore;
#[cfg(test)]
use crate::store::Journaled;
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

impl Policies {
    /// The policies that permit everything (`AllowAllPolicy`) by the default rules - for what a
    /// test writes before it runs what it tests, and for what a test runs that checks nothing.
    pub fn permitting_all() -> Self {
        Self::by_default_rules(Arc::new(AllowAllPolicy::new()))
    }

    /// The policies of `access_policy` by the default rules (`ConfidentialityRules::default`) - for
    /// a test of what `access_policy` decides.
    pub fn by_default_rules(access_policy: Arc<dyn AccessPolicy>) -> Self {
        Self::new(access_policy, Arc::new(ConfidentialityRules::default()))
    }
}

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

/// `state` as a store holds it, and the live state marked stored.
#[cfg(test)]
pub(crate) fn stored<T: Journaled>(state: &mut T) -> T {
    let copy = serde_json::from_value(serde_json::to_value(&*state).unwrap()).unwrap();
    state.mark_stored();
    copy
}

/// Replays the changes of `state` onto `copy` - twice, since replaying
/// is idempotent - and asserts that both are the same afterwards.
#[cfg(test)]
pub(crate) fn replayed<T: Journaled>(state: &T, copy: &mut T) {
    for _ in 0..2 {
        for change in state.changes() {
            copy.apply(change).unwrap();
        }
    }
    assert_eq!(serde_json::to_value(&*copy).unwrap(), serde_json::to_value(state).unwrap());
    // The small parts are told whole every time; the growing ones must
    // have recorded nothing: as a copy loaded afresh.
    let reloaded: T = serde_json::from_value(serde_json::to_value(&*copy).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(copy.changes()).unwrap(),
        serde_json::to_value(reloaded.changes()).unwrap(),
        "replaying records nothing"
    );
}
