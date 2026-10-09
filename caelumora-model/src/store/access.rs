//! The access checks every `EntityStore` makes (issue 11).
//!
//! The decision is the `SecurityManager`'s; a store only asks, and asks the
//! same way for every entity kind:
//!
//! - **Reading** asks with the stored attributes. An entity the caller may
//!   not read is reported as not found, so that ids cannot be probed for
//!   existence.
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
use crate::scope::Scoped;
use crate::security::{ActionType, Enforcement, SecurityAttributes, SecurityContext, SecurityManager};
use crate::store::{EntityKind, StoreError, StoreResult};


// Every method takes the actions to ask about as functions from an
// entity's attributes to the action, e.g. `|a| ActionType::GetSession(a)`.

pub(crate) struct Access<'a> {
    manager: &'a dyn SecurityManager,
    ctx: &'a SecurityContext,
}

impl<'a> Access<'a> {
    pub(crate) fn new(manager: &'a dyn SecurityManager, ctx: &'a SecurityContext) -> Self {
        Self { manager, ctx }
    }

    /// `action`, or `refusal()` if the caller may not.
    async fn require<E: Send>(&self, action: ActionType<'_>, refusal: impl FnOnce() -> E + Send) -> Result<(), E> {
        self.manager.require(self.ctx, action, refusal).await
    }

    /// An action that concerns no single entity, such as listing.
    pub(crate) async fn allow(&self, action: ActionType<'_>) -> StoreResult<()> {
        self.manager.require(self.ctx, action, || StoreError::PermissionDenied).await
    }

    /// `found`, if it exists and the caller may read it; not found otherwise.
    pub(crate) async fn read<T: Stored>(&self, found: Option<T>, kind: &Actions, id: String) -> StoreResult<T> {
        let not_found = || StoreError::NotFound { entity: kind.entity.name(), id };
        let Some(value) = found.filter(present) else { return Err(not_found()) };
        self.require((kind.get)(value.security_attributes()), not_found).await?;
        Ok(value)
    }

    /// The entities of `values` the caller may read.
    pub(crate) async fn filter<T: Stored>(&self, values: Vec<T>, kind: &Actions) -> Vec<T> {
        let mut readable = Vec::with_capacity(values.len());
        for value in values.into_iter().filter(present) {
            if self.manager.check(self.ctx, (kind.get)(value.security_attributes())).await {
                readable.push(value);
            }
        }
        readable
    }

    /// Whether the caller may create `value`.
    pub(crate) async fn create<T: Stored>(&self, value: &T, kind: &Actions) -> StoreResult<()> {
        unmarked(value, kind)?;
        valid(value)?;
        self.allow((kind.create)(value.security_attributes())).await
    }

    /// Whether the caller may change `stored`, the entity as it is now, to
    /// `value`.
    pub(crate) async fn change<T: Stored>(&self, stored: Option<&T>, value: &T, kind: &Actions, id: String) -> StoreResult<()> {
        let not_found = || StoreError::NotFound { entity: kind.entity.name(), id };
        let Some(stored) = stored.filter(|s| present(*s)) else { return Err(not_found()) };
        self.require((kind.get)(stored.security_attributes()), not_found).await?;
        self.allow((kind.update)(stored.security_attributes())).await?;
        unmarked(value, kind)?;
        valid(value)
    }

    /// Whether to delete `stored`: `false` if there is nothing the caller
    /// could see to delete, an error if they may see but not delete it.
    pub(crate) async fn delete<T: Stored>(&self, stored: Option<&T>, kind: &Actions) -> StoreResult<bool> {
        let Some(stored) = stored.filter(|s| present(*s)) else { return Ok(false) };
        if self.require((kind.get)(stored.security_attributes()), || ()).await.is_err() {
            return Ok(false);
        }
        self.allow((kind.delete)(stored.security_attributes())).await.map(|_| true)
    }

    /// `found`, if it is marked deleted and the caller may read what was
    /// deleted of its kind; not found otherwise.
    pub(crate) async fn read_deleted<T: Stored>(&self, found: Option<T>, kind: &Actions, id: String) -> StoreResult<T> {
        let not_found = || StoreError::NotFound { entity: kind.entity.name(), id };
        let Some(value) = found.filter(|v| !present(v)) else { return Err(not_found()) };
        self.require(ActionType::ReadDeleted(kind.entity, value.security_attributes()), not_found).await?;
        Ok(value)
    }

    /// The entities of `values` that are marked deleted and the caller may
    /// read as such.
    pub(crate) async fn filter_deleted<T: Stored>(&self, values: Vec<T>, kind: &Actions) -> Vec<T> {
        let mut readable = Vec::new();
        for value in values.into_iter().filter(|v| !present(v)) {
            if self.manager.check(self.ctx, ActionType::ReadDeleted(kind.entity, value.security_attributes())).await {
                readable.push(value);
            }
        }
        readable
    }

    /// Whether the caller may take the mark of `stored` away: not found if
    /// it is not marked or they may not read it as deleted, refused if they
    /// may but not restore it.
    pub(crate) async fn restore<T: Stored>(&self, stored: Option<&T>, kind: &Actions, id: String) -> StoreResult<()> {
        let not_found = || StoreError::NotFound { entity: kind.entity.name(), id };
        let Some(stored) = stored.filter(|s| !present(*s)) else { return Err(not_found()) };
        self.require(ActionType::ReadDeleted(kind.entity, stored.security_attributes()), not_found).await?;
        self.allow(ActionType::Restore(kind.entity, stored.security_attributes())).await
    }

    /// Whether to remove `stored` for good: `false` if it is not marked
    /// deleted or the caller may not read it as deleted, an error if they
    /// may but not purge it. Only what was deleted is purged, so that a
    /// purge never removes what was restored meanwhile.
    pub(crate) async fn purge<T: Stored>(&self, stored: Option<&T>, kind: &Actions) -> StoreResult<bool> {
        let Some(stored) = stored.filter(|s| !present(*s)) else { return Ok(false) };
        let attributes = stored.security_attributes();
        if self.require(ActionType::ReadDeleted(kind.entity, attributes), || ()).await.is_err() {
            return Ok(false);
        }
        self.allow(ActionType::Purge(kind.entity, attributes)).await.map(|_| true)
    }
}

/// Whether `value` is there for the five operations: not marked deleted.
fn present<T: Stored>(value: &T) -> bool {
    value.deleted().is_none()
}

/// Refuses `value` if one of `others` that is not deleted has its name in
/// its scope (issue 250). `others` may hold `value`'s own document only as
/// it is stored: absent, deleted or under another name.
pub(crate) fn name_free<'a, T: Stored + Scoped + 'a>(value: &T, others: impl IntoIterator<Item = &'a T>, kind: &Actions) -> StoreResult<()> {
    let name = value.scoped_name();
    match others.into_iter().filter(|other| present(*other)).any(|other| other.scoped_name() == name) {
        false => Ok(()),
        true => Err(StoreError::NameTaken { entity: kind.entity.name() }),
    }
}

/// Refuses `value` if it carries the mark of a deletion: only deleting
/// sets it (issue 164), not whoever writes a document.
fn unmarked<T: Stored>(value: &T, kind: &Actions) -> StoreResult<()> {
    match value.deleted() {
        None => Ok(()),
        Some(_) => Err(StoreError::Validation(format!("a {} is marked deleted only by deleting it", kind.entity))),
    }
}

/// Refuses `value` if it may not be stored as it is (`Stored::validate`).
fn valid<T: Stored>(value: &T) -> StoreResult<()> {
    value.validate().map_err(StoreError::Validation)
}

/// The actions that guard one kind of entity, and its name in errors: every
/// kind is read, listed, created, changed and deleted the same way (issue
/// 93), and a store implements the five operations once for all kinds.
pub(crate) struct Actions {
    pub(crate) entity: EntityKind,
    pub(crate) get: for<'a> fn(&'a SecurityAttributes) -> ActionType<'a>,
    pub(crate) list: ActionType<'static>,
    pub(crate) create: for<'a> fn(&'a SecurityAttributes) -> ActionType<'a>,
    pub(crate) update: for<'a> fn(&'a SecurityAttributes) -> ActionType<'a>,
    pub(crate) delete: for<'a> fn(&'a SecurityAttributes) -> ActionType<'a>,
}

pub(crate) const PERSONALITIES: Actions = Actions {
    entity: EntityKind::Personality,
    get: |a| ActionType::GetPersonality(a),
    list: ActionType::ListPersonalities,
    create: |a| ActionType::CreatePersonality(a),
    update: |a| ActionType::UpdatePersonality(a),
    delete: |a| ActionType::DeletePersonality(a),
};

pub(crate) const SESSION_TEMPLATES: Actions = Actions {
    entity: EntityKind::SessionTemplate,
    get: |a| ActionType::GetSessionTemplate(a),
    list: ActionType::ListSessionTemplates,
    create: |a| ActionType::CreateSessionTemplate(a),
    update: |a| ActionType::UpdateSessionTemplate(a),
    delete: |a| ActionType::DeleteSessionTemplate(a),
};

pub(crate) const AGENTS: Actions = Actions {
    entity: EntityKind::Agent,
    get: |a| ActionType::GetAgent(a),
    list: ActionType::ListAgents,
    create: |a| ActionType::CreateAgent(a),
    update: |a| ActionType::UpdateAgent(a),
    delete: |a| ActionType::DeleteAgent(a),
};

pub(crate) const SESSIONS: Actions = Actions {
    entity: EntityKind::Session,
    get: |a| ActionType::GetSession(a),
    list: ActionType::ListSessions,
    create: |a| ActionType::CreateSession(a),
    update: |a| ActionType::UpdateSession(a),
    delete: |a| ActionType::DeleteSession(a),
};

pub(crate) const CHANNELS: Actions = Actions {
    entity: EntityKind::Channel,
    get: |a| ActionType::GetChannel(a),
    list: ActionType::ListChannels,
    create: |a| ActionType::CreateChannel(a),
    update: |a| ActionType::UpdateChannel(a),
    delete: |a| ActionType::DeleteChannel(a),
};

pub(crate) const USERS: Actions = Actions {
    entity: EntityKind::User,
    get: |a| ActionType::GetUser(a),
    list: ActionType::ListUsers,
    create: |a| ActionType::CreateUser(a),
    update: |a| ActionType::UpdateUser(a),
    delete: |a| ActionType::DeleteUser(a),
};

pub(crate) const USER_PROFILES: Actions = Actions {
    entity: EntityKind::UserProfile,
    get: |a| ActionType::GetUserProfile(a),
    list: ActionType::ListUserProfiles,
    create: |a| ActionType::CreateUserProfile(a),
    update: |a| ActionType::UpdateUserProfile(a),
    delete: |a| ActionType::DeleteUserProfile(a),
};

pub(crate) const WORKSPACES: Actions = Actions {
    entity: EntityKind::Workspace,
    get: |a| ActionType::GetWorkspace(a),
    list: ActionType::ListWorkspaces,
    create: |a| ActionType::CreateWorkspace(a),
    update: |a| ActionType::UpdateWorkspace(a),
    delete: |a| ActionType::DeleteWorkspace(a),
};

pub(crate) const SKILLS: Actions = Actions {
    entity: EntityKind::Skill,
    get: |a| ActionType::GetSkill(a),
    list: ActionType::ListSkills,
    create: |a| ActionType::CreateSkill(a),
    update: |a| ActionType::UpdateSkill(a),
    delete: |a| ActionType::DeleteSkill(a),
};

pub(crate) const ARTIFACTS: Actions = Actions {
    entity: EntityKind::Artifact,
    get: |a| ActionType::GetArtifact(a),
    list: ActionType::ListArtifacts,
    create: |a| ActionType::CreateArtifact(a),
    update: |a| ActionType::UpdateArtifact(a),
    delete: |a| ActionType::DeleteArtifact(a),
};

pub(crate) const COMPONENTS: Actions = Actions {
    entity: EntityKind::Component,
    get: |a| ActionType::GetComponent(a),
    list: ActionType::ListComponents,
    create: |a| ActionType::CreateComponent(a),
    update: |a| ActionType::UpdateComponent(a),
    delete: |a| ActionType::DeleteComponent(a),
};

#[cfg(test)]
mod tests {
    use crate::actor::ActorId;
    use crate::security::{ActionType, SecurityAttributes, SecurityContext, SecurityManager};
    use crate::session::{Session, SessionId};
    use crate::store::{EntityStore, StoreError};
    use crate::store::memory::MemoryStore;
    use crate::user::UserId;
    use crate::value::name::Name;
    use std::sync::Arc;

    /// Owners do everything, contributors read, nobody acts for another.
    #[derive(Debug)]
    struct OwnersAndContributors;

    #[async_trait::async_trait]
    impl SecurityManager for OwnersAndContributors {
        async fn check(&self, ctx: &SecurityContext, action: ActionType<'_>) -> bool {
            let me = ctx.principal_id;
            match action {
                ActionType::GetSession(a) => a.owner_id == me || a.contributors.contains(&me),
                ActionType::CreateSession(a) | ActionType::UpdateSession(a) | ActionType::DeleteSession(a) => a.owner_id == me,
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
            security_attributes: SecurityAttributes { owner_id: owner, contributors },
            deleted: None,
        }
    }

    #[tokio::test]
    async fn what_the_caller_may_not_read_does_not_exist_for_them() {
        let store = MemoryStore::with_security_manager(Arc::new(OwnersAndContributors));
        let (alice, alice_ctx) = user();
        let (bob, bob_ctx) = user();
        let (_, carol_ctx) = user();
        let hers = session(alice, vec![bob]);
        store.create_session(&alice_ctx, &hers).await.unwrap();

        assert!(store.get_session(&bob_ctx, hers.id).await.is_ok(), "a contributor reads it");
        assert!(matches!(store.get_session(&carol_ctx, hers.id).await, Err(StoreError::NotFound { .. })), "not refused: absent");
        assert_eq!(store.list_sessions(&carol_ctx).await.unwrap().len(), 0, "lists are filtered");
        assert_eq!(store.list_sessions(&bob_ctx).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn changes_are_checked_against_what_is_stored() {
        let store = MemoryStore::with_security_manager(Arc::new(OwnersAndContributors));
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
        let store = MemoryStore::with_security_manager(Arc::new(OwnershipSecurityManager::new()));
        let (alice, alice_ctx) = user();
        let (bob, bob_ctx) = user();
        let hers = session(alice, vec![bob]);
        store.create_session(&alice_ctx, &hers).await.unwrap();
        assert!(store.delete_session(&alice_ctx, hers.id).await.unwrap());
        assert_eq!(store.get_deleted_session(&alice_ctx, hers.id).await.unwrap().deleted.map(|d| d.by), Some(alice), "deleted by her");

        let seen = store.get_deleted_session(&bob_ctx, hers.id).await;
        assert!(matches!(seen, Err(StoreError::NotFound { .. })), "a contributor does not see the trash: {seen:?}");
        assert!(store.list_deleted_sessions(&bob_ctx).await.unwrap().is_empty());
        let restored = store.restore_session(&bob_ctx, hers.id).await;
        assert!(matches!(restored, Err(StoreError::NotFound { .. })), "nor restores from it: {restored:?}");
        assert!(!store.purge_session(&bob_ctx, hers.id).await.unwrap(), "nothing bob could see to purge");

        let purged = store.purge_session(&alice_ctx, hers.id).await;
        assert!(matches!(purged, Err(StoreError::PermissionDenied)), "the owner does not purge: {purged:?}");
        assert_eq!(store.list_deleted_sessions(&alice_ctx).await.unwrap().len(), 1, "the owner sees her trash");
        store.restore_session(&alice_ctx, hers.id).await.expect("and restores from it");
        assert!(store.delete_session(&alice_ctx, hers.id).await.unwrap());
        assert!(store.purge_session(&SecurityContext::system(), hers.id).await.unwrap(), "the system purges");
    }

    #[tokio::test]
    async fn nobody_creates_in_someone_elses_name() {
        let store = MemoryStore::with_security_manager(Arc::new(OwnersAndContributors));
        let (alice, _) = user();
        let (_, bob_ctx) = user();
        assert!(matches!(store.create_session(&bob_ctx, &session(alice, Vec::new())).await, Err(StoreError::PermissionDenied)));
    }

    #[tokio::test]
    async fn users_are_read_by_everybody_and_changed_by_the_system_alone() {
        use crate::security::OwnershipSecurityManager;
        use crate::user::User;

        let store = MemoryStore::with_security_manager(Arc::new(OwnershipSecurityManager::new()));
        let system = SecurityContext::system();
        let alice_id = UserId::new();
        let alice_ctx = SecurityContext::new(ActorId::User(alice_id), Default::default(), Default::default());
        let (_, bob_ctx) = user();
        let record = User { id: alice_id, username: Name::new("alice").unwrap(), email: String::new(), roles: vec!["user".to_string()], deleted: None };

        let own = store.create_user(&alice_ctx, &record).await;
        assert!(matches!(own, Err(StoreError::PermissionDenied)), "nobody records themselves, got {own:?}");
        store.create_user(&system, &record).await.expect("the system records users");

        assert_eq!(store.get_user(&bob_ctx, alice_id).await.expect("another user reads her").username, "alice");
        assert_eq!(store.find_user_by_name(&bob_ctx, "alice").await.expect("and finds her by name").id, alice_id);
        assert_eq!(store.list_users(&bob_ctx).await.expect("and lists her").len(), 1);

        let promoted = User { roles: vec!["admin".to_string()], ..record.clone() };
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
