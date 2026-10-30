//! `LiveDocument`: the document a controller is the one writer of, as it stands (issue 358), and
//! the `Writing` of a change of it.

use crate::group::prune_gone_groups;
use arc_swap::{ArcSwap, Guard};
use caelumora_model::confidentiality::Classification;
use caelumora_model::event::{Announcing, EventPublisher, GlobalEvent};
use caelumora_model::group::Group;
use caelumora_model::security::{AclEntry, SecuredMut, SecurityContext, Subject};
use caelumora_model::store::{
    Address, EntityStore, StoreResult, StoreResultExt, Stored, Updatable,
};
use chrono::Utc;
use std::fmt::{self, Debug};
use std::sync::Arc;
use tokio::sync::{Mutex, MutexGuard};
use tracing::{info, warn};

/// The document a controller is the one writer of in this instance, as it stands (issue 358):
/// every change of it is stored first - which is where the permission is checked (issue 11) -,
/// then taken over live and announced. The revision keeps a change from overwriting another (issue
/// 492); the lock held meanwhile keeps the live copy and the announcements in the order the changes
/// were stored. Every change drops from its access lists the entries naming a group that is gone
/// (issue 347). What a controller does besides - a group that follows the participants, an event
/// of its own - it does between the steps of a `Writing`.
pub(super) struct LiveDocument<T: Updatable> {
    address: Address<T>,
    current: ArcSwap<T>,
    store: Arc<dyn EntityStore<T>>,
    groups: Arc<dyn EntityStore<Group>>,
    event_publisher: Arc<dyn EventPublisher>,
    write_lock: Mutex<()>,
}

impl<T: Updatable + Debug> Debug for LiveDocument<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveDocument").field("current", &self.current).finish_non_exhaustive()
    }
}

impl<T: Updatable + SecuredMut> LiveDocument<T>
where
    Address<T>: PartialEq,
{
    /// `document`, which lies at `address`, written to `store` and announced through
    /// `event_publisher`; whether a group its access lists name is gone is looked up in `groups`.
    pub(super) fn new(
        address: Address<T>,
        document: T,
        store: Arc<dyn EntityStore<T>>,
        groups: Arc<dyn EntityStore<Group>>,
        event_publisher: Arc<dyn EventPublisher>,
    ) -> Self {
        Self {
            address,
            current: ArcSwap::from_pointee(document),
            store,
            groups,
            event_publisher,
            write_lock: Mutex::new(()),
        }
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
    pub(super) async fn update(
        &self,
        ctx: &SecurityContext,
        update: &T::Update,
    ) -> StoreResult<Arc<T>> {
        let mut writing = self.write().await;
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
    /// name, while its caller holds the lock - `take_stored`, or a
    /// `Writing` whose change was outdated.
    ///
    /// # Errors
    /// Whatever the store reports; the live copy is then unchanged.
    async fn reload(&self) -> StoreResult<()> {
        let document = self.reached(&SecurityContext::system()).await?;
        self.current.store(Arc::new(document));
        Ok(())
    }

    /// Takes the document over as it is stored now, as `reload` does, under
    /// the lock: for a change made elsewhere, announced or lost (issue 502),
    /// which a change of this instance under way must not be set back by.
    ///
    /// # Errors
    /// Whatever the store reports; the live copy is then unchanged.
    pub(super) async fn take_stored(&self) -> StoreResult<()> {
        let _lock = self.write_lock.lock().await;
        self.reload().await
    }

    /// Begins a change, under the lock until the `Writing` is dropped.
    pub(super) async fn write(&self) -> Writing<'_, T> {
        let lock = self.write_lock.lock().await;
        Writing { document: self, had: self.classification(), _lock: lock }
    }

    /// How the document is classified as it stands; `None` for a kind that
    /// is not.
    fn classification(&self) -> Option<Classification> {
        self.current.load().classification().cloned()
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
pub(super) fn take_over<T: Updatable>(
    current: &ArcSwap<T>,
    address: &Address<T>,
    event: &GlobalEvent,
) where
    Address<T>: PartialEq,
{
    let Some(update) = event
        .about::<T>()
        .filter(|(at, _)| at == address)
        .and_then(|(_, change)| change.update::<T::Update>())
    else {
        return;
    };
    let mut document = (**current.load()).clone();
    match document.apply(&update) {
        Ok(()) => current.store(Arc::new(document)),
        Err(e) => {
            warn!(kind = %T::KIND, id = %document.id(), error = %e, "A change made elsewhere does not fit the document")
        }
    }
}

/// Notes in the log that `document` was changed on behalf of `ctx`, as every
/// controller does once per change - without what changed, which may be
/// what someone wrote.
pub(super) fn log_changed<T: Stored>(ctx: &SecurityContext, document: &T) {
    info!(kind = %T::KIND, id = %document.id(), by = %ctx.caller_id, "Document changed");
}

/// A change of a `LiveDocument` under way, holding its lock: the document to change (`current`, or
/// `reached` as stored), changed, stored and taken over (`change`, `apply`), then announced
/// (`announce`, `announce_update`) - or both at once for its access list (`change_access`).
pub(super) struct Writing<'a, T: Updatable> {
    document: &'a LiveDocument<T>,
    /// How the document was classified as the change began: announced to
    /// who reached it so, too, so that who no longer does learns so (issue
    /// 312).
    had: Option<Classification>,
    _lock: MutexGuard<'a, ()>,
}

impl<T: Updatable + SecuredMut> Writing<'_, T>
where
    Address<T>: PartialEq,
{
    /// A copy of the document as it stands, to change.
    pub(super) fn current(&self) -> T {
        (**self.document.current.load()).clone()
    }

    /// The document as `ctx` reaches it in the store now
    /// (`LiveDocument::reached`), taken over live, so that the change begins
    /// from it: for a change that only who reaches it may begin. Announced
    /// to who reached it as stored.
    ///
    /// # Errors
    /// As `LiveDocument::reached`.
    pub(super) async fn reached(&mut self, ctx: &SecurityContext) -> StoreResult<T> {
        let document = self.document.reached(ctx).await?;
        self.document.current.store(Arc::new(document.clone()));
        self.had = document.classification().cloned();
        Ok(document)
    }

    /// Changes the document as `change` says, on behalf of `ctx`: applied to a copy of the document
    /// as it stands, its access lists without the entries naming a group that is gone
    /// (`prune_gone_groups`, issue 347), stored as changed now (`Updatable::touch`) - which is
    /// where the permission is checked - and taken over live; not yet announced. Changed elsewhere
    /// meanwhile (`StoreError::Outdated`, issue 492), the document is read again as stored - which
    /// the change then begins from, as `reached` begins it - and `change` applied once more, to
    /// that alone. Answers what `change` answered, and the document as it now stands.
    ///
    /// # Errors
    /// What `change` reports, or whatever the store reports,
    /// `StoreError::PermissionDenied` included; the document is then
    /// unchanged.
    pub(super) async fn change<R>(
        &mut self,
        ctx: &SecurityContext,
        mut change: impl FnMut(&mut T) -> StoreResult<R>,
    ) -> StoreResult<(R, Arc<T>)> {
        loop {
            let mut document = self.current();
            let answer = change(&mut document)?;
            prune_gone_groups(self.document.groups.as_ref(), document.access_lists_mut()).await;
            document.touch(Utc::now());
            if self.document.store.update(ctx, &mut document).await.written()?.is_none() {
                self.document.reload().await?;
                self.had = self.document.classification();
                continue;
            }
            log_changed(ctx, &document);
            let document = Arc::new(document);
            self.document.current.store(Arc::clone(&document));
            return Ok((answer, document));
        }
    }

    /// The document with `update` applied (`Updatable::apply`), as `change`
    /// changes it.
    ///
    /// # Errors
    /// `StoreError::Invalid` if what `update` says does not fit, otherwise
    /// as `change`.
    pub(super) async fn apply(
        &mut self,
        ctx: &SecurityContext,
        update: &T::Update,
    ) -> StoreResult<Arc<T>> {
        self.change(ctx, |document| Ok(document.apply(update)?))
            .await
            .map(|((), document)| document)
    }

    /// Changes the document's access list as `change` says, as `change` changes the document, and
    /// announces `update`, which says so without the list (`…UpdateEvent::AccessChanged`).
    ///
    /// # Errors
    /// As `change`.
    pub(super) async fn change_access(
        &mut self,
        ctx: &SecurityContext,
        update: &T::Update,
        mut change: impl FnMut(&mut Vec<AclEntry>),
    ) -> StoreResult<()> {
        let ((), document) = self
            .change(ctx, |document| {
                change(&mut document.security_attributes_mut().acl);
                Ok(())
            })
            .await?;
        self.announce_update(update, &document);
        Ok(())
    }

    /// Announces `event` about `document`, as it was stored: to whoever may
    /// read it and, if its kind is classified, is cleared for how it was
    /// classified as the change began or how it is now.
    pub(super) fn announce(&self, event: GlobalEvent, document: &T) {
        self.announce_also_to(event, document, []);
    }

    /// Announces `event` about `document` as `announce` does, and to
    /// `subjects` besides, who may no longer read it: one who left learns so
    /// (issue 23). The grants are never stored.
    pub(super) fn announce_also_to(
        &self,
        event: GlobalEvent,
        document: &T,
        subjects: impl IntoIterator<Item = Subject>,
    ) {
        self.document.event_publisher.publish_classified(
            event,
            document.security_attributes().clone().granting_read(subjects),
            &self.classifications(document),
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::{Agent, PersonalityId};
    use caelumora_model::group::GroupId;
    use caelumora_model::scope::Scope;
    use caelumora_model::security::Grant;
    use caelumora_model::store::Persistence;
    use caelumora_model::user::UserId;
    use caelumora_model::value::name::ScopedName;
    use std::time::Duration;
    use tokio::time;

    fn assistant() -> Agent {
        Agent::new(
            ScopedName::new("assistant", Scope::GlobalScope).unwrap(),
            PersonalityId::new(),
            ActorId::User(UserId::new()),
        )
    }

    /// Issue 499: every change of a document - of any kind a controller writes - drops from its
    /// access list the entries naming a group that is gone (issue 347), and keeps the others.
    #[tokio::test]
    async fn a_change_drops_the_entries_naming_a_group_that_is_gone() {
        let stores = Persistence::in_memory().entity_stores().unwrap();
        let system = SecurityContext::system();
        let owner = UserId::new();
        let team = Group::team(
            ScopedName::new("team", Scope::UserScope { user_id: owner }).unwrap(),
            owner,
        );
        stores.groups.create(&system, &team).await.unwrap();
        let mut agent = assistant();
        let there = Subject::Group(team.id);
        let gone = Subject::Group(GroupId::new());
        for subject in [there, gone] {
            agent.security_attributes.acl.push(AclEntry { subject, grant: Grant::Read });
        }
        stores.agents.create(&system, &agent).await.unwrap();
        let document = LiveDocument::new(
            agent.id,
            agent,
            Arc::clone(&stores.agents),
            stores.groups,
            Arc::new(()),
        );

        let ((), changed) = document.write().await.change(&system, |_| Ok(())).await.unwrap();

        let named: Vec<Subject> =
            changed.security_attributes.acl.iter().map(|entry| entry.subject).collect();
        assert_eq!(named, [there], "only the entry naming a group never there is dropped");
        let stored = stores.agents.get(&system, changed.id).await.unwrap();
        assert_eq!(stored.security_attributes.acl, changed.security_attributes.acl, "as stored");
    }

    /// Issue 502: taking a document over as stored waits for a change under
    /// way, so that what it read before the change cannot set it back.
    #[tokio::test]
    async fn taking_over_what_is_stored_waits_for_a_change_under_way() {
        let agent = assistant();
        let stores = Persistence::in_memory().entity_stores().unwrap();
        stores.agents.create(&SecurityContext::system(), &agent).await.unwrap();
        let document =
            LiveDocument::new(agent.id, agent, stores.agents, stores.groups, Arc::new(()));

        let writing = document.write().await;
        let taking = document.take_stored();
        tokio::pin!(taking);
        assert!(
            time::timeout(Duration::from_millis(50), &mut taking).await.is_err(),
            "waits while the change holds the lock"
        );
        drop(writing);
        taking.await.expect("taken over once it is done");
    }
}
