//! The access checks every `EntityStore` makes (issue 11).
//!
//! The decision is the `SecurityManager`'s; a store only asks, and asks the
//! same way for every entity kind:
//!
//! - **Reading** asks with the stored attributes. An entity the caller may
//!   not read is reported as not found, so that ids cannot be probed for
//!   existence. Where it is classified - a channel, a workspace, a skill,
//!   an artifact (`Stored::classification`, issue 322) -, the
//!   `ConfidentialityPolicy` is asked next whether the caller is cleared
//!   for it, and who is not does not find it either: for every operation
//!   below, the deleted included, as for reading.
//! - **Listing** asks whether the caller may list at all, then filters the
//!   result by what they may read.
//! - **Creating** asks with the attributes of the new entity.
//! - **Changing and deleting** ask with the attributes *as stored*, never
//!   with those of a new value, which the caller controls - otherwise
//!   writing oneself in as owner would pass the check. An entity the caller
//!   may not even read is not found; one they may read but not change is
//!   refused.
//! - **What is marked deleted** (issue 164) is not there for any of them,
//!   before anything is asked: the store treats it as absent. And nothing
//!   created or changed may carry the mark, so that only deleting sets it.
//! - **Reading what was deleted** asks `ReadDeleted` with the stored
//!   attributes, and answers "not found" alike; **restoring** it asks that
//!   and then `Restore`, **purging** it that and then `Purge`. What is not
//!   marked is neither restored nor purged.
//! - **Names** (issue 250): what is `Scoped` is created, renamed and
//!   restored only under a name that no other one of its kind has in its
//!   scope (`name_free`) - checked against every document of the kind that
//!   is not deleted, also those the caller may not read, and answered with
//!   `NameTaken`, which tells that the name is taken, not by what.
//!
//! Every check goes through `Enforcement::require` (issue 95), with the
//! answer the store gives - "not found" for what the caller may not see -
//! except filtering a list, which is a question.

use crate::removal::Stored;
use std::future::Future;
use crate::store::query::{self, Cursor, Field, Page, Query};
use crate::confidentiality::{may_reach, ConfidentialityPolicy, ConfidentialityQuestions};
use crate::security::{ActionType, Enforcement, SecurityContext, SecurityManager};
use crate::store::{StoreError, StoreResult};


// Every method takes the kind of entity it asks about (issue 323): every
// kind is read, listed, created, changed and deleted the same way (issue
// 93), and a store implements the five operations once for all kinds.

pub(crate) struct Access<'a> {
    manager: &'a dyn SecurityManager,
    confidentiality: &'a dyn ConfidentialityPolicy,
    ctx: &'a SecurityContext,
}

impl<'a> Access<'a> {
    pub(crate) fn new(manager: &'a dyn SecurityManager, confidentiality: &'a dyn ConfidentialityPolicy, ctx: &'a SecurityContext) -> Self {
        Self { manager, confidentiality, ctx }
    }

    /// `action`, or `refusal()` if the caller may not.
    async fn require<E: Send>(&self, action: ActionType<'_>, refusal: impl FnOnce() -> E + Send) -> Result<(), E> {
        self.manager.require(self.ctx, action, refusal).await
    }

    /// Nothing if the caller is cleared for how `value` is classified
    /// (`Stored::classification`), `refusal()` otherwise.
    fn require_cleared<T: Stored, E>(&self, value: &T, refusal: impl FnOnce() -> E) -> Result<(), E> {
        match value.classification() {
            Some(classification) => self.confidentiality.require_cleared_for(self.ctx, classification, refusal),
            None => Ok(()),
        }
    }


    /// An action that concerns no single entity, such as listing.
    pub(crate) async fn allow(&self, action: ActionType<'_>) -> StoreResult<()> {
        self.manager.require(self.ctx, action, || StoreError::PermissionDenied).await
    }

    /// `found`, if it exists and the caller may read it; not found otherwise.
    pub(crate) async fn read<T: Stored>(&self, found: Option<T>, id: String) -> StoreResult<T> {
        let not_found = || StoreError::NotFound { entity: T::KIND.name(), id };
        let Some(value) = found.filter(present) else { return Err(not_found()) };
        self.require(ActionType::get(T::KIND, value.security_attributes()), not_found.clone()).await?;
        self.require_cleared(&value, not_found)?;
        Ok(value)
    }

    /// The page that `query` asks for (issue 293) among the entities
    /// `keys` name, each key with what the entity is found by: in the order
    /// of the keys, after its cursor, each entity loaded by `load` only when
    /// it is its turn - `None` if it does not meet the query's filter, which
    /// `load` judges, or is gone -, and of those the ones the caller may
    /// read - among what was deleted, if `deleted` -, until its limit. What
    /// follows the limit is not loaded.
    pub(crate) async fn page<T, F, K, L, R>(&self, keys: Vec<(String, K)>, query: &Query<F>, deleted: bool, mut load: L) -> StoreResult<Page<T>>
    where T: Stored, F: Field, L: FnMut(K) -> R, R: Future<Output = StoreResult<Option<T>>> {
        let mut items = Vec::new();
        let mut remaining = query::ordered_after(keys, query).peekable();
        while let Some((key, found_by)) = remaining.next() {
            let Some(candidate) = load(found_by).await? else { continue };
            let readable = match deleted {
                true => self.readable_deleted(&candidate).await,
                false => self.readable(&candidate).await,
            };
            if !readable {
                continue;
            }
            items.push(candidate);
            if query.limit.is_some_and(|limit| items.len() >= limit) {
                let next = remaining.peek().map(|_| Cursor::after(key));
                return Ok(Page { items, next });
            }
        }
        Ok(Page { items, next: None })
    }

    /// Whether the caller may read `value` among a list: it is present,
    /// they may read it (`Get`) and are cleared for it.
    pub(crate) async fn readable<T: Stored>(&self, value: &T) -> bool {
        present(value) && self.reachable(value, ActionType::get(T::KIND, value.security_attributes())).await
    }

    /// Whether the caller reaches `value`, asked as `read`: `may_reach`,
    /// with how it is classified.
    async fn reachable<T: Stored>(&self, value: &T, read: ActionType<'_>) -> bool {
        let classifications = value.classification().map_or(&[][..], std::slice::from_ref);
        may_reach(self.manager, self.confidentiality, self.ctx, read, classifications).await
    }

    /// Whether the caller may create `value`.
    pub(crate) async fn create<T: Stored>(&self, value: &T) -> StoreResult<()> {
        unmarked(value)?;
        valid(value)?;
        self.allow(ActionType::create(T::KIND, value.security_attributes())).await
    }

    /// Whether the caller may change `stored`, the entity as it is now, to
    /// `value`.
    pub(crate) async fn change<T: Stored>(&self, stored: Option<&T>, value: &T, id: String) -> StoreResult<()> {
        let not_found = || StoreError::NotFound { entity: T::KIND.name(), id };
        let Some(stored) = stored.filter(|s| present(*s)) else { return Err(not_found()) };
        self.require(ActionType::get(T::KIND, stored.security_attributes()), not_found.clone()).await?;
        self.require_cleared(stored, not_found)?;
        self.allow(ActionType::update(T::KIND, stored.security_attributes())).await?;
        unmarked(value)?;
        valid(value)
    }

    /// Whether to delete `stored`: `false` if there is nothing the caller
    /// could see to delete, an error if they may see but not delete it.
    pub(crate) async fn delete<T: Stored>(&self, stored: Option<&T>) -> StoreResult<bool> {
        let Some(stored) = stored.filter(|s| present(*s)) else { return Ok(false) };
        if self.require(ActionType::get(T::KIND, stored.security_attributes()), || ()).await.is_err() || self.require_cleared(stored, || ()).is_err() {
            return Ok(false);
        }
        self.allow(ActionType::delete(T::KIND, stored.security_attributes())).await.map(|_| true)
    }

    /// `found`, if it is marked deleted and the caller may read what was
    /// deleted of its T::KIND; not found otherwise.
    pub(crate) async fn read_deleted<T: Stored>(&self, found: Option<T>, id: String) -> StoreResult<T> {
        let not_found = || StoreError::NotFound { entity: T::KIND.name(), id };
        let Some(value) = found.filter(|v| !present(v)) else { return Err(not_found()) };
        self.require(ActionType::read_deleted(T::KIND, value.security_attributes()), not_found.clone()).await?;
        self.require_cleared(&value, not_found)?;
        Ok(value)
    }

    /// Whether the caller may read `value` among a list of what was
    /// deleted: it is marked deleted, they may read it as such
    /// (`ReadDeleted`) and are cleared for it.
    pub(crate) async fn readable_deleted<T: Stored>(&self, value: &T) -> bool {
        !present(value) && self.reachable(value, ActionType::read_deleted(T::KIND, value.security_attributes())).await
    }

    /// `stored`, if the caller may take its mark away: not found if it is
    /// not marked or they may not read it as deleted, refused if they may
    /// but not restore it.
    pub(crate) async fn restore<T: Stored>(&self, stored: Option<T>, id: String) -> StoreResult<T> {
        let stored = self.read_deleted(stored, id).await?;
        self.allow(ActionType::restore(T::KIND, stored.security_attributes())).await?;
        Ok(stored)
    }

    /// Whether to remove `stored` for good: `false` if it is not marked
    /// deleted or the caller may not read it as deleted, an error if they
    /// may but not purge it. Only what was deleted is purged, so that a
    /// purge never removes what was restored meanwhile.
    pub(crate) async fn purge<T: Stored>(&self, stored: Option<&T>) -> StoreResult<bool> {
        let Some(stored) = stored.filter(|s| !present(*s)) else { return Ok(false) };
        let attributes = stored.security_attributes();
        if self.require(ActionType::read_deleted(T::KIND, attributes), || ()).await.is_err() || self.require_cleared(stored, || ()).is_err() {
            return Ok(false);
        }
        self.allow(ActionType::purge(T::KIND, attributes)).await.map(|_| true)
    }
}

/// Whether `value` is there for the five operations: not marked deleted.
fn present<T: Stored>(value: &T) -> bool {
    value.deleted().is_none()
}

/// Refuses `value` if one of `others` that is not deleted has its name in
/// its scope (issue 250). `others` may hold `value`'s own document only as
/// it is stored: absent, deleted or under another name.
pub(crate) fn name_free<'a, T: Stored + 'a>(value: &T, others: impl IntoIterator<Item = &'a T>) -> StoreResult<()> {
    let Some(name) = value.unique_name() else { return Ok(()) };
    match others.into_iter().filter(|other| present(*other)).any(|other| other.unique_name().as_ref() == Some(&name)) {
        false => Ok(()),
        true => Err(StoreError::NameTaken { entity: T::KIND.name() }),
    }
}

/// Refuses `value` if it carries the mark of a deletion: only deleting
/// sets it (issue 164), not whoever writes a document.
fn unmarked<T: Stored>(value: &T) -> StoreResult<()> {
    match value.deleted() {
        None => Ok(()),
        Some(_) => Err(StoreError::Validation(format!("a {} is marked deleted only by deleting it", T::KIND))),
    }
}

/// Refuses `value` if it may not be stored as it is: its access list
/// (`SecurityAttributes::validate`, issue 23), and what its T::KIND says
/// (`Stored::validate`).
fn valid<T: Stored>(value: &T) -> StoreResult<()> {
    value.security_attributes().validate().map_err(StoreError::Validation)?;
    value.validate().map_err(StoreError::Validation)
}

#[cfg(test)]
mod tests {
    use crate::actor::ActorId;
    use crate::security::{ActionType, SecurityAttributes, SecurityContext, SecurityManager, Subject, Verb};
    use crate::removal::Stored;
    use crate::session::{Session, SessionId};
    use crate::store::{Entities, StoreError};
    use crate::store::query::{NoField, Query};
    use crate::store::memory::MemoryStore;
    use crate::user::UserId;
    use crate::value::name::Name;
    use std::sync::Arc;
    use crate::confidentiality::ConfidentialityRules;

    /// Owners do everything, whom the list names read, nobody acts for
    /// another.
    #[derive(Debug)]
    struct OwnersAndContributors;

    #[async_trait::async_trait]
    impl SecurityManager for OwnersAndContributors {
        async fn check(&self, ctx: &SecurityContext, action: ActionType<'_>) -> bool {
            let me = ctx.principal_id;
            match action {
                ActionType::Entity { kind: Session::KIND, verb: Verb::Get, attributes: a } => a.owner_id == me || a.acl.iter().any(|e| e.subject == Subject::Actor(me)),
                ActionType::Entity { kind: Session::KIND, verb: Verb::Create | Verb::Update | Verb::Delete, attributes: a } => a.owner_id == me,
                _ => true,
            }
        }
    }

    fn user() -> (ActorId, SecurityContext) {
        let id = ActorId::User(UserId::new());
        (id, SecurityContext::new(id, Default::default(), Default::default()))
    }

    fn session(owner: ActorId, contributors: Vec<ActorId>) -> Session {
        Session {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            workspace: None,
            id: SessionId::new(),
            name: crate::value::name::Name::new("s").unwrap(),
            participants: Vec::new(),
            channels: Vec::new(),
            commands: Default::default(),
            privacy: Default::default(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            security_attributes: contributors.into_iter().fold(SecurityAttributes::owned_by(owner), |a, c| a.granting(crate::security::Subject::Actor(c), crate::security::Grant::Read)),
            deleted: None,
        }
    }

    /// A page loads an entity only when it is its turn, and nothing after
    /// its limit (issue 293): what the caller may not read is loaded and
    /// passed over, what follows the last one is only seen to exist.
    #[tokio::test]
    async fn a_page_loads_nothing_after_its_limit() {
        let (alice, alice_ctx) = user();
        let (carol, _) = user();
        let sessions = [session(carol, Vec::new()), session(alice, Vec::new()), session(alice, Vec::new()), session(alice, Vec::new())];
        let keys: Vec<(String, usize)> = ["a", "b", "c", "d"].into_iter().map(String::from).zip(0..).collect();
        let mut loaded = Vec::new();
        let page = super::Access::new(&OwnersAndContributors, &ConfidentialityRules::default(), &alice_ctx)
            .page(keys, &Query::<NoField>::all().limited(2), false, |n: usize| {
                loaded.push(n);
                std::future::ready(Ok(Some(sessions[n].clone())))
            })
            .await
            .unwrap();
        assert_eq!(page.items.iter().map(|s| s.id).collect::<Vec<_>>(), [sessions[1].id, sessions[2].id]);
        assert_eq!(page.next.as_ref().map(|cursor| cursor.as_str()), Some("c"));
        assert_eq!(loaded, [0, 1, 2], "carol's is loaded and passed over, the last is never loaded");
    }

    /// Who may read a workspace but is not cleared for it does not find it
    /// (issue 322): not by its id, not in a list, not to change, delete,
    /// restore or purge - the deleted alike, so that the trash and the
    /// list agree.
    #[tokio::test]
    async fn what_the_caller_is_not_cleared_for_does_not_exist_for_them() {
        use crate::confidentiality::{Classification, Clearance, Compartments, Confidentiality, ConfidentialityRules};
        use crate::role::{RoleId, Roles};
        use crate::scope::Scope;
        use crate::security::DummySecurityManager;
        use crate::value::name::ScopedName;
        use crate::workspace::Workspace;
        let staff = RoleId::new();
        let roles = Roles::new([(staff, "staff".to_string(), vec![Clearance { level: Confidentiality::Internal, compartment: None }])]).unwrap();
        let rules = ConfidentialityRules::new(Compartments::new([]).unwrap()).with_roles(roles);
        let store = MemoryStore::with_policies(Arc::new(DummySecurityManager::new()), Arc::new(rules));
        let owner = UserId::new();
        let cleared = SecurityContext::new(ActorId::User(owner), Default::default(), Default::default()).with_roles(vec![staff], None);
        let uncleared = SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let workspace = Workspace::new(ScopedName::new("docs", Scope::UserScope { user_id: owner }).unwrap(), "main", Classification::general(Confidentiality::Internal), owner);
        store.create_workspace(&cleared, &workspace).await.unwrap();

        assert!(matches!(store.get_workspace(&uncleared, workspace.id).await, Err(StoreError::NotFound { .. })), "not by its id");
        assert!(store.list_workspaces(&uncleared, &Query::all()).await.unwrap().items.is_empty(), "not in a list");
        assert!(matches!(store.update_workspace(&uncleared, &workspace).await, Err(StoreError::NotFound { .. })), "not to change");
        assert!(!store.delete_workspace(&uncleared, workspace.id).await.unwrap(), "not to delete");
        assert!(store.get_workspace(&cleared, workspace.id).await.is_ok(), "who is cleared finds it");

        assert!(store.delete_workspace(&cleared, workspace.id).await.unwrap());
        assert!(store.list_deleted_workspaces(&uncleared, &Query::all()).await.unwrap().items.is_empty(), "nor in the trash");
        assert!(matches!(store.restore_workspace(&uncleared, workspace.id).await, Err(StoreError::NotFound { .. })), "nor to restore");
        assert!(!store.purge_workspace(&uncleared, workspace.id).await.unwrap(), "nor to purge");
        assert_eq!(store.list_deleted_workspaces(&cleared, &Query::all()).await.unwrap().items.len(), 1, "where it is, for who is cleared");
    }

    #[tokio::test]
    async fn what_the_caller_may_not_read_does_not_exist_for_them() {
        let store = MemoryStore::with_policies(Arc::new(OwnersAndContributors), Arc::new(ConfidentialityRules::default()));
        let (alice, alice_ctx) = user();
        let (bob, bob_ctx) = user();
        let (_, carol_ctx) = user();
        let hers = session(alice, vec![bob]);
        store.create_session(&alice_ctx, &hers).await.unwrap();

        assert!(store.get_session(&bob_ctx, hers.id).await.is_ok(), "a contributor reads it");
        assert!(matches!(store.get_session(&carol_ctx, hers.id).await, Err(StoreError::NotFound { .. })), "not refused: absent");
        assert_eq!(store.list_sessions(&carol_ctx, &Query::all()).await.map(|page| page.items).unwrap().len(), 0, "lists are filtered");
        assert_eq!(store.list_sessions(&bob_ctx, &Query::all()).await.map(|page| page.items).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn changes_are_checked_against_what_is_stored() {
        let store = MemoryStore::with_policies(Arc::new(OwnersAndContributors), Arc::new(ConfidentialityRules::default()));
        let (alice, alice_ctx) = user();
        let (bob, bob_ctx) = user();
        let (carol, carol_ctx) = user();
        let hers = session(alice, vec![bob]);
        store.create_session(&alice_ctx, &hers).await.unwrap();

        // Writing oneself in as owner does not make the change one's own.
        let mut forged = hers.clone();
        forged.security_attributes.owner_id = carol;
        assert!(matches!(store.update_session(&carol_ctx, &forged).await, Err(StoreError::NotFound { .. })));
        let mut forged = hers.clone();
        forged.security_attributes.owner_id = bob;
        assert!(matches!(store.update_session(&bob_ctx, &forged).await, Err(StoreError::PermissionDenied)), "a contributor may read, not change");
        assert!(store.update_session(&alice_ctx, &hers).await.is_ok());

        assert!(!store.delete_session(&carol_ctx, hers.id).await.unwrap(), "nothing carol could see to delete");
        assert!(matches!(store.delete_session(&bob_ctx, hers.id).await, Err(StoreError::PermissionDenied)));
        assert!(store.delete_session(&alice_ctx, hers.id).await.unwrap());
    }

    #[tokio::test]
    async fn what_was_deleted_is_the_owners_to_read_and_restore_and_the_systems_to_purge() {
        use crate::security::OwnershipSecurityManager;
        let store = MemoryStore::with_policies(Arc::new(OwnershipSecurityManager::new()), Arc::new(ConfidentialityRules::default()));
        let (alice, alice_ctx) = user();
        let (bob, bob_ctx) = user();
        let hers = session(alice, vec![bob]);
        store.create_session(&alice_ctx, &hers).await.unwrap();
        assert!(store.delete_session(&alice_ctx, hers.id).await.unwrap());
        assert_eq!(store.get_deleted_session(&alice_ctx, hers.id).await.unwrap().deleted.map(|d| d.by), Some(alice), "deleted by her");

        let seen = store.get_deleted_session(&bob_ctx, hers.id).await;
        assert!(matches!(seen, Err(StoreError::NotFound { .. })), "a contributor does not see the trash: {seen:?}");
        assert!(store.list_deleted_sessions(&bob_ctx, &Query::all()).await.map(|page| page.items).unwrap().is_empty());
        let restored = store.restore_session(&bob_ctx, hers.id).await;
        assert!(matches!(restored, Err(StoreError::NotFound { .. })), "nor restores from it: {restored:?}");
        assert!(!store.purge_session(&bob_ctx, hers.id).await.unwrap(), "nothing bob could see to purge");

        let purged = store.purge_session(&alice_ctx, hers.id).await;
        assert!(matches!(purged, Err(StoreError::PermissionDenied)), "the owner does not purge: {purged:?}");
        assert_eq!(store.list_deleted_sessions(&alice_ctx, &Query::all()).await.map(|page| page.items).unwrap().len(), 1, "the owner sees her trash");
        store.restore_session(&alice_ctx, hers.id).await.expect("and restores from it");
        assert!(store.delete_session(&alice_ctx, hers.id).await.unwrap());
        assert!(store.purge_session(&SecurityContext::system(), hers.id).await.unwrap(), "the system purges");
    }

    #[tokio::test]
    async fn nobody_creates_in_someone_elses_name() {
        let store = MemoryStore::with_policies(Arc::new(OwnersAndContributors), Arc::new(ConfidentialityRules::default()));
        let (alice, _) = user();
        let (_, bob_ctx) = user();
        assert!(matches!(store.create_session(&bob_ctx, &session(alice, Vec::new())).await, Err(StoreError::PermissionDenied)));
    }

    #[tokio::test]
    async fn users_are_read_by_everybody_and_changed_by_the_system_alone() {
        use crate::security::OwnershipSecurityManager;
        use crate::user::User;

        let store = MemoryStore::with_policies(Arc::new(OwnershipSecurityManager::new()), Arc::new(ConfidentialityRules::default()));
        let system = SecurityContext::system();
        let alice_id = UserId::new();
        let alice_ctx = SecurityContext::new(ActorId::User(alice_id), Default::default(), Default::default());
        let (_, bob_ctx) = user();
        let record = User { id: alice_id, username: Name::new("alice").unwrap(), email: String::new(), roles: vec![crate::role::RoleId::new()], deleted: None };

        let own = store.create_user(&alice_ctx, &record).await;
        assert!(matches!(own, Err(StoreError::PermissionDenied)), "nobody records themselves, got {own:?}");
        store.create_user(&system, &record).await.expect("the system records users");

        assert_eq!(store.get_user(&bob_ctx, alice_id).await.expect("another user reads her").username, "alice");
        assert_eq!(store.find_user_by_name(&bob_ctx, "alice").await.expect("and finds her by name").id, alice_id);
        assert_eq!(store.list_users(&bob_ctx, &Query::all()).await.map(|page| page.items).expect("and lists her").len(), 1);

        let promoted = User { roles: vec![crate::role::RoleId::new()], ..record.clone() };
        for ctx in [&alice_ctx, &bob_ctx] {
            let update = store.update_user(ctx, &promoted).await;
            assert!(matches!(update, Err(StoreError::PermissionDenied)), "not even she changes her roles, got {update:?}");
            let delete = store.delete_user(ctx, alice_id).await;
            assert!(matches!(delete, Err(StoreError::PermissionDenied)), "nor deletes her, got {delete:?}");
        }
        store.update_user(&system, &promoted).await.expect("the system changes users");
        assert!(store.delete_user(&system, alice_id).await.expect("the system deletes users"));
    }
}
