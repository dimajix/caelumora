//! The documents of a kind as a backend keeps them, and the one
//! `EntityStore` over them (issue 446).
//!
//! A backend keeps documents: serialized, by their id, without asking
//! anything (`DocumentStorage`) - as an `ArtifactStorage` keeps content. What
//! is asked, in which order and what is answered is the model's, once for
//! every kind and every backend (`CheckedStore`, over `store::access`). A
//! storage is not generic, so that a backend is a trait object and opens the
//! storage of a kind it was never told the type of - a plugin's too.

use crate::component::StoredComponents;
use crate::removal::{Deletable, Removal};
use crate::security::{ActionType, SecurityContext};
use crate::store::access::Access;
use crate::store::query::{Cursor, FieldPath, Filter, NoField, Page, Query};
use crate::store::{ComponentStore, EntityStore, Policies, StoreError, StoreResult, Stored};
use crate::value::name::ScopedName;
use async_trait::async_trait;
use serde_json::Value;
use std::fmt::{self, Debug};
use std::marker::PhantomData;
use std::sync::Arc;
use tracing::warn;

/// A document as it is handed to a storage: its body, and what the storage
/// needs to know of it without reading it as its type - its id, its name
/// unique in its scope, whether it is marked deleted.
#[derive(Debug, Clone)]
pub struct Document {
    pub id: String,
    /// Its name, if its kind keeps names unique in their scope
    /// (`Stored::unique_name`): no other document of the kind that is not
    /// deleted may have it. A storage compares names by `==`, or - where it
    /// needs one value, as a column of a unique index - by their keys
    /// (`ScopedName::key`, issue 447), which are equal exactly where the
    /// names are.
    pub name: Option<ScopedName>,
    pub deleted: bool,
    pub body: Value,
}

impl Document {
    /// `value` as the document of it.
    ///
    /// # Errors
    /// `StoreError::Backend` if `value` cannot be serialized.
    pub(crate) fn of<T: Stored>(value: &T) -> StoreResult<Self> {
        let body = serde_json::to_value(value).map_err(|e| StoreError::Backend(format!("a {} cannot be serialized: {e}", T::KIND)))?;
        Ok(Self { id: value.id().to_string(), name: value.unique_name(), deleted: value.deleted().is_some(), body })
    }

    /// The document of `body`, read as a `T`: what a storage that keeps
    /// bodies alone is given to tell the documents it reads
    /// (`DocumentOf`).
    ///
    /// # Errors
    /// `StoreError::Backend` if `body` is no `T`.
    pub(crate) fn read<T: Stored>(body: Value) -> StoreResult<Self> {
        let value: T = read_as(body.clone())?;
        Ok(Self { id: value.id().to_string(), name: value.unique_name(), deleted: value.deleted().is_some(), body })
    }
}

/// How a storage that keeps bodies alone - the JSON files - tells what it
/// reads: a document of the kind it was opened for (`Document::read`).
pub type DocumentOf = fn(Value) -> StoreResult<Document>;

/// The documents of one kind, as a backend keeps them (issue 446): by id,
/// serialized, without asking anything. What is kept unique - an id, a name
/// in its scope - a storage keeps so in the step that writes, so that it
/// holds against every writer of the storage, also of another process.
#[async_trait]
pub trait DocumentStorage: Debug + Send + Sync {
    /// The body of the document `id`, deleted or not; `None` if there is
    /// none.
    async fn get(&self, id: &str) -> StoreResult<Option<Value>>;

    /// The documents after the id `after`, in the order of their ids - as
    /// `str` orders them -, at most `limit`, each with its id (issue 447):
    /// every one that is marked deleted if `deleted`, not marked otherwise,
    /// and meets `filter` - and others besides, if the storage cannot tell:
    /// a scan may hand out more than it is asked for, never less, since the
    /// store judges every document again. It hands out fewer than `limit`
    /// only where none follow. A document a storage cannot read as it is
    /// kept is left out and logged; any other failure fails the scan.
    async fn scan(&self, filter: Option<&Filter<FieldPath>>, deleted: bool, after: Option<&str>, limit: usize) -> StoreResult<Vec<(String, Value)>>;

    /// The body of the document of `name` that is not marked deleted, if
    /// there is one - there is at most one (issue 250).
    async fn named(&self, name: &ScopedName) -> StoreResult<Option<Value>>;

    /// Writes `document`, which is new: `StoreError::Conflict` if one of
    /// its id is there, deleted or not; `StoreError::NameTaken` if another
    /// that is not deleted has its name.
    async fn insert(&self, document: Document) -> StoreResult<()>;

    /// Writes `document` in place of the one of its id:
    /// `StoreError::NameTaken` if another that is not deleted has its name.
    async fn replace(&self, document: Document) -> StoreResult<()>;

    /// Removes the document `id` for good; whether there was one.
    async fn remove(&self, id: &str) -> StoreResult<bool>;

    /// Removes the document `id` for good if it is marked deleted, in the
    /// same step; whether it was removed. What was restored meanwhile
    /// stays.
    async fn remove_deleted(&self, id: &str) -> StoreResult<bool>;
}

/// How many documents a list asks a storage for at a time (issue 447):
/// enough that a page is rarely more than one scan, few enough that what
/// follows a page's limit is hardly read.
const SCAN_BATCH: usize = 100;

/// `body` read as a `T`.
fn read_as<T: Stored>(body: Value) -> StoreResult<T> {
    serde_json::from_value(body).map_err(|e| StoreError::Backend(format!("a stored {} cannot be read: {e}", T::KIND)))
}

/// The entities of one kind over the storage of a backend (issue 446):
/// every operation of the `EntityStore`, checked as every kind is
/// (`store::access`) - for every backend alike, so that no backend asks a
/// policy itself.
pub(crate) struct CheckedStore<T> {
    storage: Arc<dyn DocumentStorage>,
    policies: Policies,
    kind: PhantomData<fn() -> T>,
}

impl<T: Stored> Debug for CheckedStore<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckedStore").field("kind", &T::KIND).field("storage", &self.storage).finish_non_exhaustive()
    }
}

impl<T: Stored> CheckedStore<T> {
    /// The store of the documents of `storage`, asking the `AccessPolicy`
    /// and the `ConfidentialityPolicy` of `policies`.
    pub(crate) fn new(storage: Arc<dyn DocumentStorage>, policies: &Policies) -> Self {
        Self { storage, policies: (Arc::clone(&policies.0), Arc::clone(&policies.1)), kind: PhantomData }
    }

    fn access<'a>(&'a self, ctx: &'a SecurityContext) -> Access<'a> {
        Access::new(self.policies.0.as_ref(), self.policies.1.as_ref(), ctx)
    }

    /// The entity of `id` as it is stored, deleted or not.
    async fn stored(&self, id: &T::Id) -> StoreResult<Option<T>> {
        self.storage.get(&id.to_string()).await?.map(read_as).transpose()
    }

    /// The entity of `body`, the document `id`, if it meets `filter` -
    /// judged on the document, before it is read as its type. `None` as well
    /// if it cannot be read as its type, which is logged: one broken document
    /// does not break a list.
    fn meeting(id: &str, body: Value, filter: Option<&Filter<T::Field>>) -> Option<T> {
        if filter.is_some_and(|filter| !filter.matches(&body)) {
            return None;
        }
        match read_as(body) {
            Ok(value) => Some(value),
            Err(err) => {
                warn!("The {} {} cannot be read: {}", T::KIND, id, err);
                None
            }
        }
    }

    async fn create(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()> {
        self.access(ctx).create(value).await?;
        self.storage.insert(Document::of(value)?).await
    }

    async fn get(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        let found = self.stored(&id).await?;
        self.access(ctx).read(found, id.to_string()).await
    }

    async fn update(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()> {
        let id = value.id();
        let stored = self.stored(&id).await?;
        self.access(ctx).change(stored.as_ref(), value, id.to_string()).await?;
        self.storage.replace(Document::of(value)?).await
    }

    /// Removes the entity for good: deleting a kind that is not marked.
    async fn remove(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<bool> {
        let stored = self.stored(&id).await?;
        if !self.access(ctx).delete(stored.as_ref()).await? {
            return Ok(false);
        }
        self.storage.remove(&id.to_string()).await
    }

    /// The page `query` asks for, among what is deleted if `deleted`: read
    /// from the storage in batches (issue 447), each document judged against
    /// the whole filter and asked about, until the limit counts what the
    /// caller may read (issue 293). A page that stops at its limit hands out
    /// the cursor after its last entity unless the storage showed that
    /// nothing follows.
    async fn page(&self, ctx: &SecurityContext, query: &Query<T::Field>, deleted: bool) -> StoreResult<Page<T>> {
        let access = self.access(ctx);
        access.allow(ActionType::List(T::KIND)).await?;
        let filter = query.filter.as_ref();
        let by_path = filter.map(Filter::by_path);
        let mut after = query.after.as_ref().map(|cursor| cursor.key().to_string());
        let mut items = Vec::new();
        loop {
            let batch = self.storage.scan(by_path.as_ref(), deleted, after.as_deref(), SCAN_BATCH).await?;
            let more = batch.len() == SCAN_BATCH;
            let mut rest = batch.into_iter().peekable();
            while let Some((id, body)) = rest.next() {
                if let Some(candidate) = Self::meeting(&id, body, filter) {
                    let readable = match deleted {
                        true => access.readable_deleted(&candidate).await,
                        false => access.readable(&candidate).await,
                    };
                    if readable {
                        items.push(candidate);
                        if query.limit.is_some_and(|limit| items.len() >= limit) {
                            let next = (more || rest.peek().is_some()).then(|| Cursor::after(id));
                            return Ok(Page { items, next });
                        }
                    }
                }
                after = Some(id);
            }
            if !more {
                return Ok(Page { items, next: None });
            }
        }
    }
}

#[async_trait]
impl<T: Deletable> EntityStore<T> for CheckedStore<T> {
    async fn create(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()> {
        CheckedStore::create(self, ctx, value).await
    }

    async fn get(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        CheckedStore::get(self, ctx, id).await
    }

    async fn update(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()> {
        CheckedStore::update(self, ctx, value).await
    }

    /// Marks the entity deleted, by the caller and now (issue 164).
    async fn delete(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<bool> {
        let stored = self.stored(&id).await?;
        if !self.access(ctx).delete(stored.as_ref()).await? {
            return Ok(false);
        }
        let Some(mut marked) = stored else { return Ok(false) };
        marked.set_deleted(Some(Removal::now(ctx.caller_id)));
        self.storage.replace(Document::of(&marked)?).await?;
        Ok(true)
    }

    async fn list(&self, ctx: &SecurityContext, query: &Query<T::Field>) -> StoreResult<Page<T>> {
        self.page(ctx, query, false).await
    }

    async fn restore(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        let stored = self.stored(&id).await?;
        let mut restored = self.access(ctx).restore(stored, id.to_string()).await?;
        restored.set_deleted(None);
        self.storage.replace(Document::of(&restored)?).await?;
        Ok(restored)
    }

    async fn purge(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<bool> {
        let stored = self.stored(&id).await?;
        if !self.access(ctx).purge(stored.as_ref()).await? {
            return Ok(false);
        }
        // Only if it is still marked: restored while the purge was asked
        // about, it stays.
        self.storage.remove_deleted(&id.to_string()).await
    }

    async fn get_deleted(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        let found = self.stored(&id).await?;
        self.access(ctx).read_deleted(found, id.to_string()).await
    }

    async fn list_deleted(&self, ctx: &SecurityContext, query: &Query<T::Field>) -> StoreResult<Page<T>> {
        self.page(ctx, query, true).await
    }

    /// Asked as a list is (`List`), and the one found as an entity of a list
    /// is: not found if the caller may not read it.
    async fn find_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<T> {
        let access = self.access(ctx);
        access.allow(ActionType::List(T::KIND)).await?;
        let not_found = || StoreError::NotFound { entity: T::KIND.name(), id: name.to_string() };
        let Some(body) = self.storage.named(name).await? else { return Err(not_found()) };
        match Self::meeting(&name.to_string(), body, None) {
            Some(value) if access.readable(&value).await => Ok(value),
            _ => Err(not_found()),
        }
    }
}

/// Preconfigured components, kept like the entities and removed for good
/// (issue 164).
#[async_trait]
impl ComponentStore for CheckedStore<StoredComponents> {
    async fn create_components(&self, ctx: &SecurityContext, value: &StoredComponents) -> StoreResult<()> {
        self.create(ctx, value).await
    }
    async fn get_components(&self, ctx: &SecurityContext, id: &str) -> StoreResult<StoredComponents> {
        self.get(ctx, id.to_string()).await
    }
    async fn update_components(&self, ctx: &SecurityContext, value: &StoredComponents) -> StoreResult<()> {
        self.update(ctx, value).await
    }
    async fn delete_components(&self, ctx: &SecurityContext, id: &str) -> StoreResult<bool> {
        self.remove(ctx, id.to_string()).await
    }
    async fn list_components(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<StoredComponents>> {
        self.page(ctx, query, false).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::ActorId;
    use crate::confidentiality::ConfidentialityRules;
    use crate::group::{Group, GroupField};
    use crate::scope::Scope;
    use crate::store::query::ordered_after;
    use crate::security::AllowAllPolicy;
    use crate::user::UserId;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Keeps documents in a list and hands out every one a scan reaches,
    /// whatever it is asked: what a storage that cannot tell may do.
    #[derive(Debug, Default)]
    struct Careless {
        documents: Mutex<Vec<(String, Value)>>,
        scans: AtomicUsize,
    }

    #[async_trait]
    impl DocumentStorage for Careless {
        async fn get(&self, id: &str) -> StoreResult<Option<Value>> {
            Ok(self.documents.lock().unwrap().iter().find(|(found, _)| found == id).map(|(_, body)| body.clone()))
        }

        async fn scan(&self, _filter: Option<&Filter<FieldPath>>, _deleted: bool, after: Option<&str>, limit: usize) -> StoreResult<Vec<(String, Value)>> {
            self.scans.fetch_add(1, Ordering::SeqCst);
            let documents = self.documents.lock().unwrap().clone();
            Ok(ordered_after(documents, after).take(limit).collect())
        }

        async fn named(&self, _name: &ScopedName) -> StoreResult<Option<Value>> {
            Ok(None)
        }

        async fn insert(&self, document: Document) -> StoreResult<()> {
            self.documents.lock().unwrap().push((document.id, document.body));
            Ok(())
        }

        async fn replace(&self, _document: Document) -> StoreResult<()> {
            Ok(())
        }

        async fn remove(&self, _id: &str) -> StoreResult<bool> {
            Ok(false)
        }

        async fn remove_deleted(&self, _id: &str) -> StoreResult<bool> {
            Ok(false)
        }
    }

    /// A list judges what a scan hands out against its whole filter and the
    /// mark of a deletion (issue 447), reads in batches until its page is
    /// full, and hands out a cursor only where more may follow.
    #[tokio::test]
    async fn a_list_judges_what_a_storage_hands_out_and_reads_on_in_batches() {
        let storage = Arc::new(Careless::default());
        let policies: Policies = (Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default()));
        let groups = CheckedStore::<Group>::new(Arc::<Careless>::clone(&storage), &policies);
        let system = SecurityContext::system();
        let (alice, bob) = (UserId::new(), UserId::new());
        let team = |index: usize, owner: UserId| Group::team(ScopedName::new(format!("team {index}"), Scope::UserScope { user_id: owner }).unwrap(), owner);
        let mut hers = Vec::new();
        for index in 0..2 * SCAN_BATCH {
            let owner = if index % 4 == 0 { alice } else { bob };
            let group = team(index, owner);
            groups.create(&system, &group).await.unwrap();
            if owner == alice {
                hers.push(group.id);
            }
        }
        let deleted = team(2 * SCAN_BATCH, alice);
        let mut marked = deleted.clone();
        marked.set_deleted(Some(Removal::now(ActorId::None)));
        storage.insert(Document::of(&marked).unwrap()).await.unwrap();
        hers.sort_by_key(|id| id.to_string());

        let theirs = Query::filtered(Filter::eq(GroupField::Owner, ActorId::User(alice)));
        let page = groups.list(&system, &theirs).await.unwrap();
        assert_eq!(page.items.iter().map(|group| group.id).collect::<Vec<_>>(), hers, "hers alone, not the deleted one, in the order of their ids");
        assert_eq!((page.next, storage.scans.swap(0, Ordering::SeqCst)), (None, 3), "every batch read, the last one short");

        let first = groups.list(&system, &theirs.clone().limited(1)).await.unwrap();
        assert_eq!(first.items.iter().map(|group| group.id).collect::<Vec<_>>(), hers[..1]);
        assert!(first.next.is_some(), "more may follow");
        assert_eq!(storage.scans.load(Ordering::SeqCst), 1, "nothing read past what the page needed");
    }
}
