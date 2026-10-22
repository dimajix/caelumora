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
use crate::store::query::{Filter, NoField, Page, Query};
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
    /// deleted may have it.
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

    /// The ids of every document, in no particular order.
    async fn ids(&self) -> StoreResult<Vec<String>>;

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

    /// The entity of `id`, if it is there and meets `filter` - judged on its
    /// document, before it is read as its type. `None` as well if it cannot
    /// be read, which is logged: one broken document does not break a list.
    async fn meeting(&self, id: String, filter: Option<&Filter<T::Field>>) -> StoreResult<Option<T>> {
        let body = match self.storage.get(&id).await {
            Ok(Some(body)) => body,
            Ok(None) => return Ok(None),
            Err(err) => {
                warn!("The {} {} cannot be read: {}", T::KIND, id, err);
                return Ok(None);
            }
        };
        if filter.is_some_and(|filter| !filter.matches(&body)) {
            return Ok(None);
        }
        match read_as(body) {
            Ok(value) => Ok(Some(value)),
            Err(err) => {
                warn!("The {} {} cannot be read: {}", T::KIND, id, err);
                Ok(None)
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

    /// The page `query` asks for, among what is deleted if `deleted`.
    async fn page(&self, ctx: &SecurityContext, query: &Query<T::Field>, deleted: bool) -> StoreResult<Page<T>> {
        let access = self.access(ctx);
        access.allow(ActionType::List(T::KIND)).await?;
        let keys = self.storage.ids().await?.into_iter().map(|id| (id.clone(), id)).collect();
        let filter = query.filter.as_ref();
        access.page(keys, query, deleted, |id| self.meeting(id, filter)).await
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

    async fn find_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<T> {
        let all = self.page(ctx, &Query::all(), false).await?.items;
        all.into_iter().find(|value| value.unique_name().as_ref() == Some(name))
            .ok_or_else(|| StoreError::NotFound { entity: T::KIND.name(), id: name.to_string() })
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
