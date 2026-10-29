//! The access checks every `EntityStore` makes (issue 11).
//!
//! The decision is the `AccessPolicy`'s; a store only asks, and asks the
//! same way for every entity kind:
//!
//! - **Reading** asks with the stored attributes. An entity the caller may
//!   not read is reported as not found, so that ids cannot be probed for
//!   existence. Where it is classified - a channel, a workspace, a skill,
//!   an artifact (`Stored::classification`, issue 322) -, the
//!   `ConfidentialityPolicy` is asked next whether the caller is cleared
//!   for it, and who is not does not find it either: for every operation
//!   below, the deleted included, as for reading.
//! - **Listing** asks whether the caller may list at all, has the storage
//!   narrow it to what the policy admits them to by the attributes alone
//!   (`Admission`, issue 332), then filters what it hands out by what they
//!   may read.
//! - **Creating** asks with the attributes of the new entity.
//! - **Changing and deleting** ask with the attributes *as stored*, never
//!   with those of a new value, which the caller controls - otherwise
//!   writing oneself in as owner would pass the check. An entity the caller
//!   may not even read is not found; one they may read but not change is
//!   refused.
//! - **What is marked deleted** (issue 164) is not there for any of them, before anything is
//!   asked: the store treats it as absent. And nothing created or changed may carry the mark, so
//!   that only deleting sets it. Deleting sets the mark it is given (issue 469), which names the
//!   caller - another only if the caller is the system, which deletes what goes with something
//!   deleted by whoever deleted that.
//! - **Reading what was deleted** asks `ReadDeleted` with the stored
//!   attributes, and answers "not found" alike; **restoring** it asks that
//!   and then `Restore`, **purging** it that and then `Purge`. What is not
//!   marked is neither restored nor purged. Reading and listing are the
//!   same check for both, told apart by the `Presence` they are about
//!   (issue 459), which names the verb.
//! - **Names** (issue 250): what is `Scoped` is created, renamed and
//!   restored only under a name that no other one of its kind has in its
//!   scope - kept by the storage in the step that writes
//!   (`DocumentStorage`, issue 446), against every document of the kind that
//!   is not deleted, also those the caller may not read, and answered with
//!   `NameTaken`, which tells that the name is taken, not by what.
//!
//! Every check goes through `Enforcement::require` (issue 95), with the
//! answer the store gives - "not found" for what the caller may not see -
//! except filtering a list, which is a question.

use crate::actor::ActorId;
use crate::confidentiality::{may_reach, ConfidentialityPolicy, ConfidentialityQuestions};
use crate::removal::Removal;
use crate::security::{AccessPolicy, ActionType, Admission, Enforcement, Refused, SecurityContext};
use crate::store::{Presence, StoreError, StoreResult, Stored};
use std::slice;

// Every method takes the kind of entity it asks about (issue 323): every
// kind is read, listed, created, changed and deleted the same way (issue
// 93), and a store implements the five operations once for all kinds.

pub(crate) struct Access<'a> {
    access_policy: &'a dyn AccessPolicy,
    confidentiality: &'a dyn ConfidentialityPolicy,
    ctx: &'a SecurityContext,
}

impl<'a> Access<'a> {
    pub(crate) fn new(
        access_policy: &'a dyn AccessPolicy,
        confidentiality: &'a dyn ConfidentialityPolicy,
        ctx: &'a SecurityContext,
    ) -> Self {
        Self { access_policy, confidentiality, ctx }
    }

    /// `action`, or the error `refusal` makes of what was refused if the
    /// caller may not.
    async fn require<E: Send>(
        &self,
        action: ActionType<'_>,
        refusal: impl FnOnce(Refused) -> E + Send,
    ) -> Result<(), E> {
        self.access_policy.require(self.ctx, action, refusal).await
    }

    /// Nothing if the caller is cleared for how `value` is classified
    /// (`Stored::classification`), `refusal()` otherwise.
    fn require_cleared<T: Stored, E>(
        &self,
        value: &T,
        refusal: impl FnOnce() -> E,
    ) -> Result<(), E> {
        match value.classification() {
            Some(classification) => {
                self.confidentiality.require_cleared_for(self.ctx, classification, refusal)
            }
            None => Ok(()),
        }
    }

    /// An action that concerns no single entity, such as listing.
    pub(crate) async fn allow(&self, action: ActionType<'_>) -> StoreResult<()> {
        self.access_policy.require(self.ctx, action, StoreError::PermissionDenied).await
    }

    /// Nothing if `value` is among `presence`, the caller may read it as such
    /// (`Presence::verb`) and is cleared for it; `refusal()` otherwise.
    async fn require_reached<T: Stored, E: Send>(
        &self,
        value: &T,
        presence: Presence,
        refusal: impl Fn() -> E + Send + Sync,
    ) -> Result<(), E> {
        if !presence.holds(value) {
            return Err(refusal());
        }
        self.require(
            ActionType::entity(T::KIND, presence.verb(), value.security_attributes()),
            |_| refusal(),
        )
        .await?;
        self.require_cleared(value, refusal)
    }

    /// `found`, if it exists, is among `presence` and the caller may read it
    /// as such; not found otherwise.
    pub(crate) async fn read<T: Stored>(
        &self,
        found: Option<T>,
        id: String,
        presence: Presence,
    ) -> StoreResult<T> {
        let not_found = || StoreError::NotFound { entity: T::KIND.name(), id: id.clone() };
        let Some(value) = found else { return Err(not_found()) };
        self.require_reached(&value, presence, not_found).await?;
        Ok(value)
    }

    /// Whether the caller may read `value` among a list of `presence`: it is
    /// among them, they may read it as such (`Presence::verb`) and are
    /// cleared for it.
    pub(crate) async fn readable<T: Stored>(&self, value: &T, presence: Presence) -> bool {
        presence.holds(value)
            && self
                .reachable(
                    value,
                    ActionType::entity(T::KIND, presence.verb(), value.security_attributes()),
                )
                .await
    }

    /// What the policy admits the caller to among a list of `presence`, by
    /// the attributes alone (issue 332): the admission of what `readable`
    /// asks.
    pub(crate) fn admission<T: Stored>(&self, presence: Presence) -> Option<Admission> {
        self.access_policy.admission(self.ctx, T::KIND, presence.verb())
    }

    /// Whether the caller reaches `value`, asked as `read`: `may_reach`,
    /// with how it is classified.
    async fn reachable<T: Stored>(&self, value: &T, read: ActionType<'_>) -> bool {
        let classifications = value.classification().map_or(&[][..], slice::from_ref);
        may_reach(self.access_policy, self.confidentiality, self.ctx, read, classifications).await
    }

    /// Whether the caller may create `value`.
    pub(crate) async fn create<T: Stored>(&self, value: &T) -> StoreResult<()> {
        unmarked(value)?;
        valid(value)?;
        self.allow(ActionType::create(T::KIND, value.security_attributes())).await
    }

    /// Whether the caller may change `stored`, the entity as it is now, to
    /// `value`.
    pub(crate) async fn change<T: Stored>(
        &self,
        stored: Option<&T>,
        value: &T,
        id: String,
    ) -> StoreResult<()> {
        let not_found = || StoreError::NotFound { entity: T::KIND.name(), id: id.clone() };
        let Some(stored) = stored else { return Err(not_found()) };
        self.require_reached(stored, Presence::Present, not_found).await?;
        self.allow(ActionType::update(T::KIND, stored.security_attributes())).await?;
        unmarked(value)?;
        valid(value)
    }

    /// Whether to delete `stored` with the mark `removal`: `false` if there is nothing the caller
    /// could see to delete, an error if they may see but not delete it, or if `removal` names
    /// another than the caller.
    pub(crate) async fn delete<T: Stored>(
        &self,
        stored: Option<&T>,
        removal: &Removal,
    ) -> StoreResult<bool> {
        let Some(stored) = stored else { return Ok(false) };
        if self.require_reached(stored, Presence::Present, || ()).await.is_err() {
            return Ok(false);
        }
        self.allow(ActionType::delete(T::KIND, stored.security_attributes())).await?;
        self.by_caller::<T>(removal)?;
        Ok(true)
    }

    /// Refuses `removal` if it names another than the caller, unless the caller is the system: it
    /// deletes what goes with something deleted, by whoever deleted that (issue 469).
    fn by_caller<T: Stored>(&self, removal: &Removal) -> StoreResult<()> {
        if removal.by == self.ctx.caller_id || self.ctx.caller_id == ActorId::System {
            return Ok(());
        }
        Err(StoreError::Validation(format!("a {} is marked deleted by its caller", T::KIND)))
    }

    /// `stored`, if the caller may take its mark away: not found if it is
    /// not marked or they may not read it as deleted, refused if they may
    /// but not restore it.
    pub(crate) async fn restore<T: Stored>(&self, stored: Option<T>, id: String) -> StoreResult<T> {
        let stored = self.read(stored, id, Presence::Deleted).await?;
        self.allow(ActionType::restore(T::KIND, stored.security_attributes())).await?;
        Ok(stored)
    }

    /// Whether to remove `stored` for good: `false` if it is not marked
    /// deleted or the caller may not read it as deleted, an error if they
    /// may but not purge it. Only what was deleted is purged, so that a
    /// purge never removes what was restored meanwhile.
    pub(crate) async fn purge<T: Stored>(&self, stored: Option<&T>) -> StoreResult<bool> {
        let Some(stored) = stored else { return Ok(false) };
        if self.require_reached(stored, Presence::Deleted, || ()).await.is_err() {
            return Ok(false);
        }
        self.allow(ActionType::purge(T::KIND, stored.security_attributes())).await.map(|_| true)
    }
}

/// Refuses `value` if it carries the mark of a deletion: only deleting
/// sets it (issue 164), not whoever writes a document.
fn unmarked<T: Stored>(value: &T) -> StoreResult<()> {
    match value.deleted() {
        None => Ok(()),
        Some(_) => Err(StoreError::Validation(format!(
            "a {} is marked deleted only by deleting it",
            T::KIND
        ))),
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
    use crate::confidentiality::ConfidentialityRules;
    use crate::project::ProjectId;
    use crate::removal::Removal;
    use crate::role::RoleId;
    use crate::security::{
        AccessPolicy, ActionType, Grant, OwnershipPolicy, SecurityAttributes, SecurityContext,
        Subject, Verb,
    };
    use crate::session::{Session, SessionId};
    use crate::store::query::Query;
    use crate::store::Persistence;
    use crate::store::{StoreError, Stored};
    use crate::user::UserId;
    use crate::value::name::Name;
    use chrono::Utc;
    use std::sync::Arc;

    /// Owners do everything, whom the list names read, nobody acts for
    /// another.
    #[derive(Debug)]
    struct OwnersAndContributors;

    #[async_trait::async_trait]
    impl AccessPolicy for OwnersAndContributors {
        async fn check(&self, ctx: &SecurityContext, action: ActionType<'_>) -> bool {
            let me = ctx.principal_id;
            match action {
                ActionType::Entity { kind: Session::KIND, verb: Verb::Get, attributes: a } => {
                    a.owner_id == me || a.acl.iter().any(|e| e.subject == Subject::Actor(me))
                }
                ActionType::Entity {
                    kind: Session::KIND,
                    verb: Verb::Create | Verb::Update | Verb::Delete,
                    attributes: a,
                } => a.owner_id == me,
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
            summary: None,
            briefing: None,
            language: None,
            time_zone: None,
            approval_rules: Vec::new(),
            workspace: None,
            project: ProjectId::new(),
            id: SessionId::new(),
            name: Name::new("s").unwrap(),
            participants: Vec::new(),
            channels: Vec::new(),
            commands: Default::default(),
            privacy: Default::default(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            security_attributes: contributors
                .into_iter()
                .fold(SecurityAttributes::owned_by(owner), |a, c| {
                    a.granting(Subject::Actor(c), Grant::Read)
                }),
            deleted: None,
        }
    }

    /// Who may read a workspace but is not cleared for it does not find it
    /// (issue 322): not by its id, not in a list, not to change, delete,
    /// restore or purge - the deleted alike, so that both lists agree.
    #[tokio::test]
    async fn what_the_caller_is_not_cleared_for_does_not_exist_for_them() {
        use crate::confidentiality::{Classification, Clearance, Compartments, Confidentiality};
        use crate::role::Roles;
        use crate::scope::Scope;
        use crate::security::AllowAllPolicy;
        use crate::value::name::ScopedName;
        use crate::workspace::Workspace;
        let staff = RoleId::new();
        let roles = Roles::new([(
            staff,
            "staff".to_string(),
            vec![Clearance { level: Confidentiality::Internal, compartment: None }],
        )])
        .unwrap();
        let rules = ConfidentialityRules::new(Compartments::new([]).unwrap()).with_roles(roles);
        let store = Persistence::in_memory_with(Arc::new(AllowAllPolicy::new()), Arc::new(rules))
            .entity_stores()
            .unwrap()
            .workspaces;
        let owner = UserId::new();
        let cleared =
            SecurityContext::new(ActorId::User(owner), Default::default(), Default::default())
                .with_roles(vec![staff], None);
        let uncleared =
            SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let workspace = Workspace::new(
            ScopedName::new("docs", Scope::UserScope { user_id: owner }).unwrap(),
            "main",
            Classification::general(Confidentiality::Internal),
            owner,
        );
        store.create(&cleared, &workspace).await.unwrap();

        assert!(
            matches!(store.get(&uncleared, workspace.id).await, Err(StoreError::NotFound { .. })),
            "not by its id"
        );
        assert!(
            store.list(&uncleared, &Query::all()).await.unwrap().items.is_empty(),
            "not in a list"
        );
        assert!(
            matches!(store.update(&uncleared, &workspace).await, Err(StoreError::NotFound { .. })),
            "not to change"
        );
        assert!(
            !store
                .delete(&uncleared, workspace.id, &Removal::now(uncleared.caller_id))
                .await
                .unwrap(),
            "not to delete"
        );
        assert!(store.get(&cleared, workspace.id).await.is_ok(), "who is cleared finds it");

        assert!(store
            .delete(&cleared, workspace.id, &Removal::now(cleared.caller_id))
            .await
            .unwrap());
        assert!(
            store.list_deleted(&uncleared, &Query::all()).await.unwrap().items.is_empty(),
            "nor among the deleted"
        );
        assert!(
            matches!(
                store.restore(&uncleared, workspace.id).await,
                Err(StoreError::NotFound { .. })
            ),
            "nor to restore"
        );
        assert!(!store.purge(&uncleared, workspace.id).await.unwrap(), "nor to purge");
        assert_eq!(
            store.list_deleted(&cleared, &Query::all()).await.unwrap().items.len(),
            1,
            "where it is, for who is cleared"
        );
    }

    #[tokio::test]
    async fn what_the_caller_may_not_read_does_not_exist_for_them() {
        let store = Persistence::in_memory_with(
            Arc::new(OwnersAndContributors),
            Arc::new(ConfidentialityRules::default()),
        )
        .entity_stores()
        .unwrap()
        .sessions;
        let (alice, alice_ctx) = user();
        let (bob, bob_ctx) = user();
        let (_, carol_ctx) = user();
        let hers = session(alice, vec![bob]);
        store.create(&alice_ctx, &hers).await.unwrap();

        assert!(store.get(&bob_ctx, hers.id).await.is_ok(), "a contributor reads it");
        assert!(
            matches!(store.get(&carol_ctx, hers.id).await, Err(StoreError::NotFound { .. })),
            "not refused: absent"
        );
        assert_eq!(
            store.list(&carol_ctx, &Query::all()).await.map(|page| page.items).unwrap().len(),
            0,
            "lists are filtered"
        );
        assert_eq!(
            store.list(&bob_ctx, &Query::all()).await.map(|page| page.items).unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn changes_are_checked_against_what_is_stored() {
        let store = Persistence::in_memory_with(
            Arc::new(OwnersAndContributors),
            Arc::new(ConfidentialityRules::default()),
        )
        .entity_stores()
        .unwrap()
        .sessions;
        let (alice, alice_ctx) = user();
        let (bob, bob_ctx) = user();
        let (carol, carol_ctx) = user();
        let hers = session(alice, vec![bob]);
        store.create(&alice_ctx, &hers).await.unwrap();

        // Writing oneself in as owner does not make the change one's own.
        let mut forged = hers.clone();
        forged.security_attributes.owner_id = carol;
        assert!(matches!(
            store.update(&carol_ctx, &forged).await,
            Err(StoreError::NotFound { .. })
        ));
        let mut forged = hers.clone();
        forged.security_attributes.owner_id = bob;
        assert!(
            matches!(store.update(&bob_ctx, &forged).await, Err(StoreError::PermissionDenied(_))),
            "a contributor may read, not change"
        );
        assert!(store.update(&alice_ctx, &hers).await.is_ok());

        assert!(
            !store.delete(&carol_ctx, hers.id, &Removal::now(carol_ctx.caller_id)).await.unwrap(),
            "nothing carol could see to delete"
        );
        assert!(matches!(
            store.delete(&bob_ctx, hers.id, &Removal::now(bob_ctx.caller_id)).await,
            Err(StoreError::PermissionDenied(_))
        ));
        assert!(store
            .delete(&alice_ctx, hers.id, &Removal::now(alice_ctx.caller_id))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn what_was_deleted_is_the_owners_to_read_and_restore_and_the_systems_to_purge() {
        let store = Persistence::in_memory_with(
            Arc::new(OwnershipPolicy::new()),
            Arc::new(ConfidentialityRules::default()),
        )
        .entity_stores()
        .unwrap()
        .sessions;
        let (alice, alice_ctx) = user();
        let (bob, bob_ctx) = user();
        let hers = session(alice, vec![bob]);
        store.create(&alice_ctx, &hers).await.unwrap();
        assert!(store
            .delete(&alice_ctx, hers.id, &Removal::now(alice_ctx.caller_id))
            .await
            .unwrap());
        assert_eq!(
            store.get_deleted(&alice_ctx, hers.id).await.unwrap().removal.by,
            alice,
            "deleted by her"
        );

        let seen = store.get_deleted(&bob_ctx, hers.id).await;
        assert!(
            matches!(seen, Err(StoreError::NotFound { .. })),
            "a contributor does not see what was deleted: {seen:?}"
        );
        assert!(store
            .list_deleted(&bob_ctx, &Query::all())
            .await
            .map(|page| page.items)
            .unwrap()
            .is_empty());
        let restored = store.restore(&bob_ctx, hers.id).await;
        assert!(
            matches!(restored, Err(StoreError::NotFound { .. })),
            "nor restores from it: {restored:?}"
        );
        assert!(!store.purge(&bob_ctx, hers.id).await.unwrap(), "nothing bob could see to purge");

        let purged = store.purge(&alice_ctx, hers.id).await;
        assert!(
            matches!(purged, Err(StoreError::PermissionDenied(_))),
            "the owner does not purge: {purged:?}"
        );
        assert_eq!(
            store
                .list_deleted(&alice_ctx, &Query::all())
                .await
                .map(|page| page.items)
                .unwrap()
                .len(),
            1,
            "the owner sees what she deleted"
        );
        store.restore(&alice_ctx, hers.id).await.expect("and restores it");
        assert!(store
            .delete(&alice_ctx, hers.id, &Removal::now(alice_ctx.caller_id))
            .await
            .unwrap());
        assert!(
            store.purge(&SecurityContext::system(), hers.id).await.unwrap(),
            "the system purges"
        );
    }

    /// Issue 469: a deletion sets the mark it is given - the one mark of a deletion, given to all
    /// that goes with what is deleted -, which names the caller; the system's names another.
    #[tokio::test]
    async fn a_deletion_sets_the_mark_it_is_given_by_its_caller() {
        let store = Persistence::in_memory_with(
            Arc::new(OwnershipPolicy::new()),
            Arc::new(ConfidentialityRules::default()),
        )
        .entity_stores()
        .unwrap()
        .sessions;
        let system = SecurityContext::system();
        let (alice, alice_ctx) = user();
        let (bob, _) = user();
        let (hers, other) = (session(alice, Vec::new()), session(alice, Vec::new()));
        store.create(&alice_ctx, &hers).await.unwrap();
        store.create(&alice_ctx, &other).await.unwrap();

        let forged = store.delete(&alice_ctx, hers.id, &Removal::now(bob)).await;
        assert!(
            matches!(forged, Err(StoreError::Validation(_))),
            "not in another's name: {forged:?}"
        );
        let removal = Removal::now(alice_ctx.caller_id);
        assert!(store.delete(&alice_ctx, hers.id, &removal).await.unwrap());
        assert!(
            store.delete(&system, other.id, &removal).await.unwrap(),
            "the system marks what goes with it, by her"
        );
        for id in [hers.id, other.id] {
            assert_eq!(
                store.get_deleted(&system, id).await.unwrap().removal,
                removal,
                "the mark given"
            );
        }
    }

    #[tokio::test]
    async fn nobody_creates_in_someone_elses_name() {
        let store = Persistence::in_memory_with(
            Arc::new(OwnersAndContributors),
            Arc::new(ConfidentialityRules::default()),
        )
        .entity_stores()
        .unwrap()
        .sessions;
        let (alice, _) = user();
        let (_, bob_ctx) = user();
        assert!(matches!(
            store.create(&bob_ctx, &session(alice, Vec::new())).await,
            Err(StoreError::PermissionDenied(_))
        ));
    }

    #[tokio::test]
    async fn users_are_read_by_everybody_and_changed_by_the_system_alone() {
        use crate::scope::Scope;
        use crate::user::User;
        use crate::value::name::ScopedName;

        let store = Persistence::in_memory_with(
            Arc::new(OwnershipPolicy::new()),
            Arc::new(ConfidentialityRules::default()),
        )
        .entity_stores()
        .unwrap()
        .users;
        let system = SecurityContext::system();
        let alice_id = UserId::new();
        let alice_ctx =
            SecurityContext::new(ActorId::User(alice_id), Default::default(), Default::default());
        let (_, bob_ctx) = user();
        let record = User {
            id: alice_id,
            username: Name::new("alice").unwrap(),
            email: String::new(),
            roles: vec![RoleId::new()],
            deleted: None,
        };

        let own = store.create(&alice_ctx, &record).await;
        assert!(
            matches!(own, Err(StoreError::PermissionDenied(_))),
            "nobody records themselves, got {own:?}"
        );
        store.create(&system, &record).await.expect("the system records users");

        assert_eq!(
            store.get(&bob_ctx, alice_id).await.expect("another user reads her").username,
            "alice"
        );
        assert_eq!(
            store
                .find_by_name(&bob_ctx, &ScopedName::new("alice", Scope::GlobalScope).unwrap())
                .await
                .expect("and finds her by name")
                .id,
            alice_id
        );
        assert_eq!(
            store
                .list(&bob_ctx, &Query::all())
                .await
                .map(|page| page.items)
                .expect("and lists her")
                .len(),
            1
        );

        let promoted = User { roles: vec![RoleId::new()], ..record.clone() };
        for ctx in [&alice_ctx, &bob_ctx] {
            let update = store.update(ctx, &promoted).await;
            assert!(
                matches!(update, Err(StoreError::PermissionDenied(_))),
                "not even she changes her roles, got {update:?}"
            );
            let delete = store.delete(ctx, alice_id, &Removal::now(ctx.caller_id)).await;
            assert!(
                matches!(delete, Err(StoreError::PermissionDenied(_))),
                "nor deletes her, got {delete:?}"
            );
        }
        store.update(&system, &promoted).await.expect("the system changes users");
        assert!(store
            .delete(&system, alice_id, &Removal::now(system.caller_id))
            .await
            .expect("the system deletes users"));
    }
}
