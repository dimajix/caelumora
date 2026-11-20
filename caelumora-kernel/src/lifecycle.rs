//! `Lifecycle`: the documents of one kind made, marked deleted and their mark taken away, each
//! announced as it is stored - the lifecycle of a kind as its manager carries it out (issue 593).

use crate::event::EventBus;
use caelumora_domain::event::{Announcing, GlobalEvent};
use caelumora_domain::removal::Removal;
use caelumora_domain::store::{Address, StoreResult, Stored};
use caelumora_security::SecurityContext;
use caelumora_store::EntityStore;
use std::fmt;
use std::sync::Arc;

/// The lifecycle of the documents of one kind, as its manager carries it out: each made, marked
/// deleted or its mark taken away in the store - which asks whether the caller may -, and
/// announced, so that whoever makes the change announces it, an undo as much as the change it
/// undoes (issue 593). Purging announces nothing, as no purge does.
pub(crate) struct Lifecycle<T: Stored> {
    store: Arc<dyn EntityStore<T>>,
    events: Arc<EventBus>,
    /// Where a document of the kind lies, which its events are about.
    address: fn(&T) -> Address<T>,
}

impl<T: Stored> fmt::Debug for Lifecycle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lifecycle").field("kind", &T::KIND).finish_non_exhaustive()
    }
}

impl<T: Stored> Lifecycle<T> {
    /// The lifecycle of the documents in `store`, announced on `events`, each lying where
    /// `address` says.
    pub(crate) fn new(
        store: &Arc<dyn EntityStore<T>>,
        events: &Arc<EventBus>,
        address: fn(&T) -> Address<T>,
    ) -> Self {
        Self { store: Arc::clone(store), events: Arc::clone(events), address }
    }

    /// Stores `document` as new on behalf of `ctx` (`Create…`, asked by the store), then announces
    /// it created.
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::PermissionDenied` and `NameTaken` among it.
    pub(crate) async fn create(&self, ctx: &SecurityContext, document: &mut T) -> StoreResult<()> {
        self.store.create(ctx, document).await?;
        self.events.announce(GlobalEvent::created::<T>(&(self.address)(document)), document);
        Ok(())
    }

    /// Marks `document` deleted with `removal` on behalf of `ctx` (`Delete…`, asked by the store),
    /// then announces it deleted, as it was; `false` if the store finds nothing to mark.
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::PermissionDenied` among it.
    pub(crate) async fn delete(
        &self,
        ctx: &SecurityContext,
        document: &T,
        removal: &Removal,
    ) -> StoreResult<bool> {
        if !self.store.delete(ctx, document.id(), removal).await? {
            return Ok(false);
        }
        self.events.announce(GlobalEvent::deleted::<T>(&(self.address)(document)), document);
        Ok(true)
    }

    /// Takes the mark of the deleted document `id` away on behalf of `ctx` (`Restore`, asked by
    /// the store), then announces it restored; answers it as stored.
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::NotFound` for no such deleted document among it.
    pub(crate) async fn restore(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        let document = self.store.restore(ctx, id).await?;
        self.events.announce(GlobalEvent::restored::<T>(&(self.address)(&document)), &document);
        Ok(document)
    }
}
