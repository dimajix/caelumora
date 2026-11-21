//! `Lifecycle`: the documents of one kind made, marked deleted, their mark taken away and purged -
//! the lifecycle of a kind as its manager carries it out, each change but a purge announced as it
//! is stored (issue 593).

use caelumora_domain::event::{Announcing, EventPublisher, GlobalEvent};
use caelumora_domain::removal::Removal;
use caelumora_domain::store::{StoreError, StoreResult, StoreResultExt, Stored};
use caelumora_security::SecurityContext;
use caelumora_store::{purge_behind, EntityStore};
use std::fmt;
use std::ops::AsyncFnOnce;
use std::sync::Arc;

/// The lifecycle of the documents of one kind, as its manager carries it out - a manager of the
/// kernel's or of a plugin's: each made, marked deleted, its mark taken away or purged in the
/// store, which asks whether the caller may, and each but a purge announced as it is stored, so
/// that whoever makes a change announces it, an undo as much as the change it undoes (issue 593).
pub struct Lifecycle<T: Stored> {
    store: Arc<dyn EntityStore<T>>,
    events: Arc<dyn EventPublisher>,
}

impl<T: Stored> fmt::Debug for Lifecycle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lifecycle").field("kind", &T::KIND).finish_non_exhaustive()
    }
}

impl<T: Stored> Lifecycle<T> {
    /// The lifecycle of the documents in `store`, announced through `events`.
    pub fn new(store: &Arc<dyn EntityStore<T>>, events: Arc<dyn EventPublisher>) -> Self {
        Self { store: Arc::clone(store), events }
    }

    /// The store of the kind, for what is read and changed of it apart from its lifecycle - and to
    /// purge what has nothing behind it.
    pub fn store(&self) -> &dyn EntityStore<T> {
        self.store.as_ref()
    }

    /// Stores `document` as new on behalf of `ctx` (`Create…`, asked by the store), then announces
    /// it created.
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::PermissionDenied` and `NameTaken` among it.
    pub async fn create(&self, ctx: &SecurityContext, document: &mut T) -> StoreResult<()> {
        self.store.create(ctx, document).await?;
        self.events.announce(GlobalEvent::created::<T>(&document.address()), document);
        Ok(())
    }

    /// Marks `document` deleted with `removal` on behalf of `ctx` (`Delete…`, asked by the store),
    /// then announces it deleted, as it was; `false` if the store finds nothing to mark.
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::PermissionDenied` among it.
    pub async fn delete(
        &self,
        ctx: &SecurityContext,
        document: &T,
        removal: &Removal,
    ) -> StoreResult<bool> {
        if !self.store.delete(ctx, document.id(), removal).await? {
            return Ok(false);
        }
        self.events.announce(GlobalEvent::deleted::<T>(&document.address()), document);
        Ok(true)
    }

    /// Takes the mark of the deleted document `id` away on behalf of `ctx` (`Restore`, asked by
    /// the store), then announces it restored; answers it as stored.
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::NotFound` for no such deleted document among it.
    pub(crate) async fn restore(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        let document = self.store.restore(ctx, id).await?;
        self.events.announce(GlobalEvent::restored::<T>(&document.address()), &document);
        Ok(document)
    }

    /// Takes `document`, made a moment ago and not to be kept, back in the kernel's name: marked
    /// deleted and announced so, as its making was announced, then purged for good - as what made
    /// it fails afterwards. What lies behind it is the caller's.
    ///
    /// # Errors
    /// Whatever the store reports.
    pub(crate) async fn discard(&self, document: &T) -> StoreResult<()> {
        let system = SecurityContext::system();
        self.delete(&system, document, &Removal::now(system.caller_id)).await?;
        self.purge_for_good(document.id()).await?;
        Ok(())
    }

    /// Removes the deleted document `id` for good on behalf of `ctx`, in the order of every purge
    /// of what has something behind it (`purge_behind`): its purge begun (`Purge`, asked by the
    /// store), then what lies behind it (`remove_behind`), then the document. Announces nothing, as
    /// no purge does. `false` if there was nothing deleted the caller could see.
    ///
    /// # Errors
    /// What `remove_behind` fails with, or whatever the store reports; the document stays deleted,
    /// its purge begun, for the next purge to finish (issue 496).
    pub async fn purge<E: From<StoreError>>(
        &self,
        ctx: &SecurityContext,
        id: T::Id,
        remove_behind: impl AsyncFnOnce(&T) -> Result<(), E>,
    ) -> Result<bool, E> {
        purge_behind(self.store.as_ref(), ctx, id, remove_behind).await
    }

    /// Removes `id` for good in the kernel's name, marked deleted first if it is not, since only
    /// what was deleted is purged (issue 164): for what goes with something purged - a plugin's
    /// document with what it belongs to -, or was made for something whose making failed. Announces
    /// nothing. `false` if there is none.
    ///
    /// # Errors
    /// Whatever else the store reports.
    pub async fn purge_for_good(&self, id: T::Id) -> StoreResult<bool> {
        let system = SecurityContext::system();
        let removal = Removal::now(system.caller_id);
        if self.store.delete(&system, id.clone(), &removal).await.optional()?.is_none() {
            return Ok(false);
        }
        self.store.purge(&system, id).await
    }
}
