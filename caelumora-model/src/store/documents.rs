//! The documents of a kind as a backend keeps them, and the one
//! `EntityStore` over them (issue 446).
//!
//! A backend keeps documents: serialized, by their id, without asking
//! anything (`DocumentStorage`) - as a `ContentStorage` keeps content. What
//! is asked, in which order and what is answered is the model's, once for
//! every kind and every backend (`CheckedStore`, over `store::access`). A
//! storage is not generic, so that a backend is a trait object and opens the
//! storage of a kind it was never told the type of - a plugin's too.

use crate::removal::{Removal, RemovalId};
use crate::security::{ActionType, Admission, Policies, SecurityAttributes, SecurityContext};
use crate::store::access::Access;
use crate::store::query::{Cursor, FieldPath, Filter, Page, Query};
use crate::store::{
    Deleted, EntityStore, Presence, Revision, StoreError, StoreResult, StoreResultExt, Stored,
};
use crate::value::name::ScopedName;
use async_trait::async_trait;
use serde_json::Value;
use std::fmt::{self, Debug, Display};
use std::marker::PhantomData;
use std::sync::Arc;
use tracing::warn;
use uuid::Uuid;

/// A document as it is handed to a storage: its body, and what the storage
/// needs to know of it without reading it as its type - its id, its name
/// unique in its scope, the deletion whose mark it carries, who may reach it,
/// and its revision.
#[derive(Debug, Clone)]
pub struct Document {
    pub id: Uuid,
    /// Its revision (issue 492): the one it is written as, and what a write
    /// in its place is judged against.
    pub revision: Revision,
    /// Its name, if its kind keeps names unique in their scope
    /// (`Stored::unique_name`): no other document of the kind that is not
    /// deleted may have it. A storage compares names by `==`, or - where it
    /// needs one value, as a column of a unique index - by their keys
    /// (`ScopedName::key`, issue 447), which are equal exactly where the
    /// names are.
    pub name: Option<ScopedName>,
    /// The deletion whose mark it carries (issue 478); `None` if it is there.
    pub removal_id: Option<RemovalId>,
    /// Its owner and its access list, which a list is narrowed by
    /// (`Admission`, issue 332).
    pub security_attributes: SecurityAttributes,
    pub body: Value,
}

impl Document {
    /// `value` as the document of it.
    ///
    /// # Errors
    /// `StoreError::Backend` if `value` cannot be serialized.
    pub(crate) fn of<T: Stored>(value: &T) -> StoreResult<Self> {
        let body = serde_json::to_value(value)
            .map_err(|e| StoreError::Backend(format!("a {} cannot be serialized: {e}", T::KIND)))?;
        Ok(Self {
            id: value.id().into(),
            revision: value.revision(),
            name: value.unique_name(),
            removal_id: value.deleted().map(|removal| removal.id),
            security_attributes: value.security_attributes().clone(),
            body,
        })
    }

    /// The document of `body`, read as a `T`: what a storage that keeps
    /// bodies alone is given to tell the documents it reads
    /// (`DocumentOf`).
    ///
    /// # Errors
    /// `StoreError::Backend` if `body` is no `T`.
    pub(crate) fn read<T: Stored>(body: Value) -> StoreResult<Self> {
        let value: T = read_as(body.clone())?;
        Ok(Self {
            id: value.id().into(),
            revision: value.revision(),
            name: value.unique_name(),
            removal_id: value.deleted().map(|removal| removal.id),
            security_attributes: value.security_attributes().clone(),
            body,
        })
    }
}

/// How a storage that keeps bodies alone - the JSON files - tells what it
/// reads: a document of the kind it was opened for (`Document::read`).
pub type DocumentOf = fn(Value) -> StoreResult<Document>;

/// The documents of one kind, as a backend keeps them (issue 446): by id,
/// serialized, without asking anything. What is kept unique - an id, a name
/// in its scope - a storage keeps so in the step that writes, so that it
/// holds against every writer of the storage, also of another process; so
/// does the revision a write in place of a document is judged against
/// (issue 492).
#[async_trait]
pub trait DocumentStorage: Debug + Send + Sync {
    /// The body of the document `id`, deleted or not; `None` if there is
    /// none.
    async fn get(&self, id: Uuid) -> StoreResult<Option<Value>>;

    /// The documents after the id `after`, in the order of their ids - as `Uuid` orders them,
    /// bytewise -, at most `limit`, each with its id (issue 447): every one among `presence` - its
    /// mark compared by `Presence::holds_mark`, or in parts by `marked` and `removal_id` -, meeting
    /// `filter` and admitted by `admission`, if one is given (issue 332) - and others besides, if
    /// the storage cannot tell: a scan may hand out more than it is asked for, never less, since
    /// the store judges every document again. It
    /// hands out fewer than `limit` only where none follow. A document a storage cannot read as it
    /// is kept is left out and logged; any other failure fails the scan.
    async fn scan(
        &self,
        filter: Option<&Filter<FieldPath>>,
        admission: Option<&Admission>,
        presence: Presence,
        after: Option<Uuid>,
        limit: usize,
    ) -> StoreResult<Vec<(Uuid, Value)>>;

    /// The body of the document of `name` that is not marked deleted, if
    /// there is one - there is at most one (issue 250).
    async fn named(&self, name: &ScopedName) -> StoreResult<Option<Value>>;

    /// Writes `document`, which is new: `StoreError::Conflict` if one of
    /// its id is there, deleted or not - asked before its name, so that it
    /// is a conflict where both are taken (`seed::create_unless_taken`,
    /// issue 457) -; `StoreError::NameTaken` if another that is not deleted
    /// has its name.
    async fn insert(&self, document: Document) -> StoreResult<()>;

    /// Writes `document` in place of the one of its id, if that one is at
    /// the revision `read` - in the same step, so that it holds against
    /// every writer of the storage (issue 492) -: `StoreError::Outdated` if
    /// it is at another, or gone; `StoreError::NameTaken` if another that
    /// is not deleted has its name.
    async fn replace(&self, document: Document, read: Revision) -> StoreResult<()>;

    /// Removes the document `id` for good if it is at the revision `read`,
    /// in the same step: whether there was one; `StoreError::Outdated` if
    /// it is at another. What was restored or changed meanwhile stays.
    async fn remove(&self, id: Uuid, read: Revision) -> StoreResult<bool>;
}

/// How many documents a list asks a storage for at a time (issue 447):
/// enough that a page is rarely more than one scan, few enough that what
/// follows a page's limit is hardly read.
const SCAN_BATCH: usize = 100;

/// `body` read as a `T`.
fn read_as<T: Stored>(body: Value) -> StoreResult<T> {
    serde_json::from_value(body)
        .map_err(|e| StoreError::Backend(format!("a stored {} cannot be read: {e}", T::KIND)))
}

/// Writes `value` by `write` as the revision after the one it carries (issues 492, 500) - a new
/// one, never stored, as the one after the first -, handing `write` its document and the revision
/// it carried. `value` then carries the revision it was written as, or the one it had if it was
/// not written.
///
/// # Errors
/// `StoreError::Backend` if `value` cannot be serialized, or whatever `write` reports.
async fn written_as_next<T: Stored>(
    value: &mut T,
    write: impl AsyncFnOnce(Document, Revision) -> StoreResult<()>,
) -> StoreResult<()> {
    let read = value.revision();
    value.set_revision(read.next());
    let written = match Document::of(value) {
        Ok(document) => write(document, read).await,
        Err(e) => Err(e),
    };
    if written.is_err() {
        value.set_revision(read);
    }
    written
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
        f.debug_struct("CheckedStore")
            .field("kind", &T::KIND)
            .field("storage", &self.storage)
            .finish_non_exhaustive()
    }
}

impl<T: Stored> CheckedStore<T> {
    /// The store of the documents of `storage`, asking the `AccessPolicy`
    /// and the `ConfidentialityPolicy` of `policies`.
    pub(crate) fn new(storage: Arc<dyn DocumentStorage>, policies: &Policies) -> Self {
        Self { storage, policies: policies.clone(), kind: PhantomData }
    }

    fn access<'a>(&'a self, ctx: &'a SecurityContext) -> Access<'a> {
        Access::new(&self.policies, ctx)
    }

    /// The entity of `id` as it is stored, deleted or not.
    async fn stored(&self, id: &T::Id) -> StoreResult<Option<T>> {
        self.storage.get(id.clone().into()).await?.map(read_as).transpose()
    }

    /// The entity of `body`, the document `id`, if it meets `filter` -
    /// judged on the document, before it is read as its type. `None` as well
    /// if it cannot be read as its type, which is logged: one broken document
    /// does not break a list.
    fn meeting(id: impl Display, body: Value, filter: Option<&Filter<T::Field>>) -> Option<T> {
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

    async fn create(&self, ctx: &SecurityContext, value: &mut T) -> StoreResult<()> {
        self.access(ctx).create(value).await?;
        written_as_next(value, async |document, _| self.storage.insert(document).await).await
    }

    async fn get(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        let found = self.stored(&id).await?;
        self.access(ctx).read(found, id.to_string(), Presence::Present).await
    }

    async fn update(&self, ctx: &SecurityContext, value: &mut T) -> StoreResult<()> {
        let id = value.id();
        let stored = self.stored(&id).await?;
        self.access(ctx).change(stored.as_ref(), value, id.to_string()).await?;
        self.write(value).await
    }

    /// Writes `value` - read at its revision, changed - as the next one, in
    /// place of what was read (issue 492); `value` then carries the
    /// revision it was written as, or the one it had if it was not written.
    ///
    /// # Errors
    /// `StoreError::Outdated` if it was changed meanwhile, or whatever else
    /// the storage reports.
    async fn write(&self, value: &mut T) -> StoreResult<()> {
        written_as_next(value, async |document, read| self.storage.replace(document, read).await)
            .await
    }

    /// The page `query` asks for, among the entities of `presence`: read
    /// from the storage in batches (issue 447), narrowed to what the policy
    /// admits the caller to (issue 332), each document judged against the
    /// whole filter and asked about, until the limit counts what the caller
    /// may read (issue 293). A page that stops at its limit hands out the
    /// cursor after its last entity unless the storage showed that nothing
    /// follows.
    async fn page(
        &self,
        ctx: &SecurityContext,
        query: &Query<T::Field>,
        presence: Presence,
    ) -> StoreResult<Page<T>> {
        let access = self.access(ctx);
        access.allow(ActionType::List(T::KIND)).await?;
        let filter = query.filter.as_ref();
        let by_path = filter.map(Filter::by_path);
        let admission = access.admission::<T>(presence);
        let mut after = query.after.as_ref().map(Cursor::id);
        let mut items = Vec::new();
        loop {
            let batch = self
                .storage
                .scan(by_path.as_ref(), admission.as_ref(), presence, after, SCAN_BATCH)
                .await?;
            let more = batch.len() == SCAN_BATCH;
            let mut rest = batch.into_iter().peekable();
            while let Some((id, body)) = rest.next() {
                if let Some(candidate) = Self::meeting(id, body, filter) {
                    if access.readable(&candidate, presence).await {
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
impl<T: Stored> EntityStore<T> for CheckedStore<T> {
    async fn create(&self, ctx: &SecurityContext, value: &mut T) -> StoreResult<()> {
        CheckedStore::create(self, ctx, value).await
    }

    async fn get(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        CheckedStore::get(self, ctx, id).await
    }

    async fn update(&self, ctx: &SecurityContext, value: &mut T) -> StoreResult<()> {
        CheckedStore::update(self, ctx, value).await
    }

    /// Marks the entity deleted with `removal` (issues 164, 469): read,
    /// asked about and written as it was read - again if it was changed
    /// meanwhile (issue 492).
    async fn delete(
        &self,
        ctx: &SecurityContext,
        id: T::Id,
        removal: &Removal,
    ) -> StoreResult<bool> {
        loop {
            let stored = self.stored(&id).await?;
            if !self.access(ctx).delete(stored.as_ref(), removal).await? {
                return Ok(false);
            }
            let Some(mut marked) = stored else { return Ok(false) };
            marked.set_deleted(Some(removal.clone()));
            if self.write(&mut marked).await.written()?.is_some() {
                return Ok(true);
            }
        }
    }

    async fn list(&self, ctx: &SecurityContext, query: &Query<T::Field>) -> StoreResult<Page<T>> {
        self.page(ctx, query, Presence::Present).await
    }

    /// Read, asked about and written as it was read - again if it was
    /// changed meanwhile (issue 492).
    async fn restore(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        loop {
            let stored = self.stored(&id).await?;
            let mut restored = self.access(ctx).restore(stored, id.to_string()).await?;
            restored.set_deleted(None);
            if self.write(&mut restored).await.written()?.is_some() {
                return Ok(restored);
            }
        }
    }

    /// Read, asked about as `purge` asks, and written saying so (`Stored::set_purging`) - again if
    /// it was changed meanwhile (issue 492). One whose purge was begun before is answered as it is.
    async fn begin_purge(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<Option<T>> {
        loop {
            let stored = self.stored(&id).await?;
            if !self.access(ctx).purge(stored.as_ref()).await? {
                return Ok(None);
            }
            let Some(mut marked) = stored else { return Ok(None) };
            if marked.purging() {
                return Ok(Some(marked));
            }
            marked.set_purging();
            if self.write(&mut marked).await.written()?.is_some() {
                return Ok(Some(marked));
            }
        }
    }

    /// Read, asked about and removed as it was read - again if it was
    /// changed meanwhile (issue 492): restored, it is then not the
    /// caller's to purge, and stays.
    async fn purge(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<bool> {
        loop {
            let stored = self.stored(&id).await?;
            if !self.access(ctx).purge(stored.as_ref()).await? {
                return Ok(false);
            }
            let Some(stored) = stored else { return Ok(false) };
            if let Some(removed) =
                self.storage.remove(id.clone().into(), stored.revision()).await.written()?
            {
                return Ok(removed);
            }
        }
    }

    /// Read as deleted, it carries the mark (`Presence::Deleted`).
    async fn get_deleted(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<Deleted<T>> {
        let found = self.stored(&id).await?;
        let entity = self.access(ctx).read(found, id.to_string(), Presence::Deleted).await?;
        Deleted::of(entity)
            .ok_or_else(|| StoreError::NotFound { entity: T::KIND.name(), id: id.to_string() })
    }

    async fn list_deleted(
        &self,
        ctx: &SecurityContext,
        query: &Query<T::Field>,
    ) -> StoreResult<Page<Deleted<T>>> {
        let Page { items, next } = self.page(ctx, query, Presence::Deleted).await?;
        Ok(Page { items: items.into_iter().filter_map(Deleted::of).collect(), next })
    }

    async fn list_deleted_with(
        &self,
        ctx: &SecurityContext,
        removal_id: RemovalId,
        query: &Query<T::Field>,
    ) -> StoreResult<Page<T>> {
        self.page(ctx, query, Presence::DeletedWith(removal_id)).await
    }

    /// Asked as a list is (`List`), and the one found as an entity of a list
    /// is: not found if the caller may not read it.
    async fn find_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<T> {
        let access = self.access(ctx);
        access.allow(ActionType::List(T::KIND)).await?;
        let not_found = || StoreError::NotFound { entity: T::KIND.name(), id: name.to_string() };
        let Some(body) = self.storage.named(name).await? else { return Err(not_found()) };
        match Self::meeting(name, body, None) {
            Some(value) if access.readable(&value, Presence::Present).await => Ok(value),
            _ => Err(not_found()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::ActorId;
    use crate::group::{Group, GroupField};
    use crate::scope::Scope;
    use crate::store::query::ordered_after;
    use crate::store::testing::ContendedStore;
    use crate::store::EntityStores;
    use crate::user::UserId;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Keeps documents in a list and hands out every one a scan reaches,
    /// whatever it is asked: what a storage that cannot tell may do.
    #[derive(Debug, Default)]
    struct Careless {
        documents: Mutex<Vec<(Uuid, Value)>>,
        scans: AtomicUsize,
    }

    #[async_trait]
    impl DocumentStorage for Careless {
        async fn get(&self, id: Uuid) -> StoreResult<Option<Value>> {
            Ok(self
                .documents
                .lock()
                .unwrap()
                .iter()
                .find(|(found, _)| *found == id)
                .map(|(_, body)| body.clone()))
        }

        async fn scan(
            &self,
            _filter: Option<&Filter<FieldPath>>,
            _admission: Option<&Admission>,
            _presence: Presence,
            after: Option<Uuid>,
            limit: usize,
        ) -> StoreResult<Vec<(Uuid, Value)>> {
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

        async fn replace(&self, _document: Document, _read: Revision) -> StoreResult<()> {
            Ok(())
        }

        async fn remove(&self, _id: Uuid, _read: Revision) -> StoreResult<bool> {
            Ok(false)
        }
    }

    /// What deletes, restores and purges reads again what was changed
    /// meanwhile and goes through (issue 492); what updates is given the
    /// refusal, since only its caller can change again.
    #[tokio::test]
    async fn what_reads_what_it_writes_writes_again_when_it_was_changed_meanwhile() {
        let (stores, refusals) = ContendedStore::stores(Group::KIND);
        let groups = EntityStores::open(&stores, &Policies::permitting_all()).unwrap().groups;
        let system = SecurityContext::system();
        let alice = UserId::new();
        let mut team = Group::team(
            ScopedName::new("team", Scope::UserScope { user_id: alice }).unwrap(),
            alice,
        );
        groups.create(&system, &mut team).await.unwrap();

        refusals.store(1, Ordering::SeqCst);
        let refused = groups.update(&system, &mut team).await;
        assert!(matches!(refused, Err(StoreError::Outdated { .. })), "{refused:?}");
        let mark = Removal::now(ActorId::System);
        refusals.store(1, Ordering::SeqCst);
        assert!(groups.delete(&system, team.id, &mark).await.expect("deleted all the same"));
        assert_eq!(refusals.load(Ordering::SeqCst), 0, "refused once, then through");
        refusals.store(1, Ordering::SeqCst);
        groups.restore(&system, team.id).await.expect("restored all the same");
        assert_eq!(refusals.load(Ordering::SeqCst), 0);
        assert!(groups.delete(&system, team.id, &mark).await.unwrap());
        refusals.store(1, Ordering::SeqCst);
        assert!(groups.purge(&system, team.id).await.expect("purged all the same"));
        assert_eq!(refusals.load(Ordering::SeqCst), 0);
        assert!(groups.get_deleted(&system, team.id).await.is_err(), "gone");
    }

    /// A list judges what a scan hands out against its whole filter and the
    /// mark of a deletion (issue 447), reads in batches until its page is
    /// full, and hands out a cursor only where more may follow.
    #[tokio::test]
    async fn a_list_judges_what_a_storage_hands_out_and_reads_on_in_batches() {
        let storage = Arc::new(Careless::default());
        let groups = CheckedStore::<Group>::new(
            Arc::<Careless>::clone(&storage),
            &Policies::permitting_all(),
        );
        let system = SecurityContext::system();
        let (alice, bob) = (UserId::new(), UserId::new());
        let team = |index: usize, owner: UserId| {
            Group::team(
                ScopedName::new(format!("team {index}"), Scope::UserScope { user_id: owner })
                    .unwrap(),
                owner,
            )
        };
        let mut hers = Vec::new();
        for index in 0..2 * SCAN_BATCH {
            let owner = if index % 4 == 0 { alice } else { bob };
            let mut group = team(index, owner);
            groups.create(&system, &mut group).await.unwrap();
            if owner == alice {
                hers.push(group.id);
            }
        }
        let deleted = team(2 * SCAN_BATCH, alice);
        let mut marked = deleted.clone();
        marked.set_deleted(Some(Removal::now(ActorId::None)));
        storage.insert(Document::of(&marked).unwrap()).await.unwrap();
        hers.sort();

        let theirs = Query::filtered(Filter::eq(GroupField::Owner, ActorId::User(alice)));
        let page = groups.list(&system, &theirs).await.unwrap();
        assert_eq!(
            page.items.iter().map(|group| group.id).collect::<Vec<_>>(),
            hers,
            "hers alone, not the deleted one, in the order of their ids"
        );
        assert_eq!(
            (page.next, storage.scans.swap(0, Ordering::SeqCst)),
            (None, 3),
            "every batch read, the last one short"
        );

        let first = groups.list(&system, &theirs.clone().limited(1)).await.unwrap();
        assert_eq!(first.items.iter().map(|group| group.id).collect::<Vec<_>>(), hers[..1]);
        assert!(first.next.is_some(), "more may follow");
        assert_eq!(
            storage.scans.load(Ordering::SeqCst),
            1,
            "nothing read past what the page needed"
        );
    }
}
