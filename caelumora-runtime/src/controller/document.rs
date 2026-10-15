use arc_swap::{ArcSwap, Guard};
use caelumora_model::confidentiality::Classification;
use caelumora_model::event::{Announcing, EventPublisher, GlobalEvent};
use caelumora_model::security::{Grant, SecurityContext, Subject};
use caelumora_model::removal::Deletable;
use caelumora_model::store::{Address, EntityStore, StoreResult, Stored, Updatable};
use chrono::Utc;
use std::fmt::{self, Debug};
use std::sync::Arc;
use tokio::sync::{Mutex, MutexGuard};
use tracing::{info, warn};

/// The document a controller is the one writer of in this instance, as it
/// stands (issue 358): every change of it is stored first - which is where
/// the permission is checked (issue 11) -, then taken over live and
/// announced, under a lock, so that no change overwrites another. What a
/// controller does besides - a group that follows the participants, an
/// event of its own - it does between the steps of a `Writing`.
pub(super) struct LiveDocument<T: Updatable + Deletable> {
    address: Address<T>,
    current: ArcSwap<T>,
    store: Arc<dyn EntityStore<T>>,
    event_publisher: Arc<dyn EventPublisher>,
    write_lock: Mutex<()>,
}

impl<T: Updatable + Deletable + Debug> Debug for LiveDocument<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveDocument").field("current", &self.current).finish_non_exhaustive()
    }
}

impl<T: Updatable + Deletable> LiveDocument<T>
where
    Address<T>: PartialEq,
{
    /// `document`, which lies at `address`, written to `store` and
    /// announced through `event_publisher`.
    pub(super) fn new(address: Address<T>, document: T, store: Arc<dyn EntityStore<T>>, event_publisher: Arc<dyn EventPublisher>) -> Self {
        Self { address, current: ArcSwap::from_pointee(document), store, event_publisher, write_lock: Mutex::new(()) }
    }

    /// Where it lies: its id, or the id of the entity it lies in and its
    /// own.
    pub(super) fn address(&self) -> &Address<T> {
        &self.address
    }

    /// The document as it stands.
    pub(super) fn load(&self) -> Guard<Arc<T>> {
        self.current.load()
    }

    /// Changes the document as `update` says, on behalf of `ctx`: applied
    /// (`Updatable::apply`), stored, taken over and announced, as a
    /// [`Writing`] does it. Returns it as it now stands.
    ///
    /// # Errors
    /// `StoreError::Invalid` if what `update` says does not fit, otherwise
    /// whatever the store reports, `StoreError::PermissionDenied` included;
    /// the document is then unchanged.
    pub(super) async fn update(&self, ctx: &SecurityContext, update: &T::Update) -> StoreResult<Arc<T>> {
        let writing = self.write().await;
        let document = writing.apply(ctx, update).await?;
        writing.announce_update(update, &document);
        Ok(document)
    }

    /// The document as `ctx` reaches it in the store now - read and cleared
    /// for, as the views reach it -, not the live copy, which a change made
    /// elsewhere may not have reached yet.
    ///
    /// # Errors
    /// `StoreError::NotFound` if `ctx` does not reach it, or whatever else
    /// the store reports.
    pub(super) async fn reached(&self, ctx: &SecurityContext) -> StoreResult<T> {
        let id = self.current.load().id();
        self.store.get(ctx, id).await
    }

    /// Takes the document over as it is stored now, read in the runtime's
    /// name: for a change made elsewhere whose announcement does not carry
    /// all of it.
    ///
    /// # Errors
    /// Whatever the store reports; the live copy is then unchanged.
    pub(super) async fn reload(&self) -> StoreResult<()> {
        let document = self.reached(&SecurityContext::system()).await?;
        self.current.store(Arc::new(document));
        Ok(())
    }

    /// Begins a change, under the lock until the `Writing` is dropped.
    pub(super) async fn write(&self) -> Writing<'_, T> {
        let lock = self.write_lock.lock().await;
        let had = self.current.load().classification().cloned();
        Writing { document: self, had, _lock: lock }
    }

    /// Announces `event` about the document as it stands, without a change
    /// of it (`Announcing::announce`).
    pub(super) fn announce(&self, event: GlobalEvent) {
        self.event_publisher.announce(event, &**self.current.load());
    }

    /// Takes over a change made elsewhere - already stored there - once it
    /// is announced: `event`, if it is an update of this document. An
    /// update carries the resulting value, so taking over a change of this
    /// controller's own again changes nothing.
    pub(super) fn take_over(&self, event: &GlobalEvent) {
        take_over(&self.current, &self.address, event);
    }
}

/// Takes `event` over into `current`, the live copy of the document at
/// `address`, if it is an update of it. One that does not fit is left out
/// rather than taken over half: it was checked where it was made.
pub(super) fn take_over<T: Updatable>(current: &ArcSwap<T>, address: &Address<T>, event: &GlobalEvent)
where
    Address<T>: PartialEq,
{
    let Some(update) = event.about::<T>().filter(|(at, _)| at == address).and_then(|(_, change)| change.update::<T::Update>()) else {
        return;
    };
    let mut document = (**current.load()).clone();
    match document.apply(&update) {
        Ok(()) => current.store(Arc::new(document)),
        Err(e) => warn!(kind = %T::KIND, id = %document.id(), error = %e, "A change made elsewhere does not fit the document"),
    }
}

/// Notes in the log that `document` was changed on behalf of `ctx`, as every
/// controller does once per change - without what changed, which may be
/// what someone wrote.
pub(super) fn log_changed<T: Stored>(ctx: &SecurityContext, document: &T) {
    info!(kind = %T::KIND, id = %document.id(), by = %ctx.caller_id, "Document changed");
}

/// A change of a `LiveDocument` under way, holding its lock: the document
/// to change (`current`, or `reached` as stored), stored and taken over
/// (`store`, `apply`), then announced (`announce`, `announce_update`).
pub(super) struct Writing<'a, T: Updatable + Deletable> {
    document: &'a LiveDocument<T>,
    /// How the document was classified as the change began: announced to
    /// who reached it so, too, so that who no longer does learns so (issue
    /// 312).
    had: Option<Classification>,
    _lock: MutexGuard<'a, ()>,
}

impl<T: Updatable + Deletable> Writing<'_, T>
where
    Address<T>: PartialEq,
{
    /// A copy of the document as it stands, to change.
    pub(super) fn current(&self) -> T {
        (**self.document.current.load()).clone()
    }

    /// The document as `ctx` reaches it in the store now
    /// (`LiveDocument::reached`), to change in place of the live copy: for
    /// a change that only who reaches it may begin. Announced to who
    /// reached it as stored.
    ///
    /// # Errors
    /// As `LiveDocument::reached`.
    pub(super) async fn reached(&mut self, ctx: &SecurityContext) -> StoreResult<T> {
        let document = self.document.reached(ctx).await?;
        self.had = document.classification().cloned();
        Ok(document)
    }

    /// The document with `update` applied (`Updatable::apply`), stored and
    /// taken over as `store` does.
    ///
    /// # Errors
    /// `StoreError::Invalid` if what `update` says does not fit, otherwise
    /// as `store`.
    pub(super) async fn apply(&self, ctx: &SecurityContext, update: &T::Update) -> StoreResult<Arc<T>> {
        let mut document = self.current();
        document.apply(update)?;
        self.store(ctx, document).await
    }

    /// Stores `document` as changed now by `ctx` (`Updatable::touch`) and
    /// takes it over live; not yet announced.
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::PermissionDenied` included;
    /// the document is then unchanged.
    pub(super) async fn store(&self, ctx: &SecurityContext, mut document: T) -> StoreResult<Arc<T>> {
        document.touch(Utc::now());
        self.document.store.update(ctx, &document).await?;
        log_changed(ctx, &document);
        let document = Arc::new(document);
        self.document.current.store(Arc::clone(&document));
        Ok(document)
    }

    /// Announces `event` about `document`, as it was stored: to whoever may
    /// read it and, if its kind is classified, is cleared for how it was
    /// classified as the change began or how it is now.
    pub(super) fn announce(&self, event: GlobalEvent, document: &T) {
        self.document.event_publisher.publish_classified(event, document.security_attributes().clone(), &self.classifications(document));
    }

    /// Announces `event` about `document` as `announce` does, and to
    /// `subject` besides, who may no longer read it: one who left learns so
    /// (issue 23). The grant is never stored.
    pub(super) fn announce_also_to(&self, event: GlobalEvent, document: &T, subject: Subject) {
        let attributes = document.security_attributes().clone().granting(subject, Grant::Read);
        self.document.event_publisher.publish_classified(event, attributes, &self.classifications(document));
    }

    /// Announces `update` of `document`, as `announce` does.
    pub(super) fn announce_update(&self, update: &T::Update, document: &T) {
        self.announce(GlobalEvent::updated::<T>(&self.document.address, update), document);
    }

    /// How `document` was classified as the change began and how it is
    /// now, once if they are the same; none for a kind that is not
    /// classified.
    fn classifications(&self, document: &T) -> Vec<Classification> {
        match (&self.had, document.classification()) {
            (Some(had), Some(now)) if had != now => vec![had.clone(), now.clone()],
            (_, now) => now.cloned().into_iter().collect(),
        }
    }
}
