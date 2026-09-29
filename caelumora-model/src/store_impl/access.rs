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

use crate::security::{ActionType, Secured, SecurityAttributes, SecurityContext, SecurityManager};
use crate::store::{StoreError, StoreResult};


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

    async fn permits(&self, action: ActionType<'_>) -> bool {
        self.manager.check(self.ctx, action).await
    }

    /// An action that concerns no single entity, such as listing.
    pub(crate) async fn allow(&self, action: ActionType<'_>) -> StoreResult<()> {
        if self.permits(action).await { Ok(()) } else { Err(StoreError::PermissionDenied) }
    }

    /// `found`, if it exists and the caller may read it; not found otherwise.
    pub(crate) async fn read<T: Secured>(&self, found: Option<T>, read: impl Fn(&SecurityAttributes) -> ActionType<'_>, entity: &'static str, id: String) -> StoreResult<T> {
        match found {
            Some(value) if self.permits(read(value.security_attributes())).await => Ok(value),
            _ => Err(StoreError::NotFound { entity, id }),
        }
    }

    /// The entities of `values` the caller may read.
    pub(crate) async fn filter<T: Secured>(&self, values: Vec<T>, read: impl Fn(&SecurityAttributes) -> ActionType<'_>) -> Vec<T> {
        let mut readable = Vec::with_capacity(values.len());
        for value in values {
            if self.permits(read(value.security_attributes())).await {
                readable.push(value);
            }
        }
        readable
    }

    /// Whether the caller may create `value`.
    pub(crate) async fn create<T: Secured>(&self, value: &T, create: impl Fn(&SecurityAttributes) -> ActionType<'_>) -> StoreResult<()> {
        self.allow(create(value.security_attributes())).await
    }

    /// Whether the caller may change `stored`, the entity as it is now.
    pub(crate) async fn change<T: Secured>(&self, stored: Option<&T>, read: impl Fn(&SecurityAttributes) -> ActionType<'_>, change: impl Fn(&SecurityAttributes) -> ActionType<'_>, entity: &'static str, id: String) -> StoreResult<()> {
        let Some(stored) = stored else {
            return Err(StoreError::NotFound { entity, id });
        };
        if !self.permits(read(stored.security_attributes())).await {
            return Err(StoreError::NotFound { entity, id });
        }
        self.allow(change(stored.security_attributes())).await
    }

    /// Whether to delete `stored`: `false` if there is nothing the caller
    /// could see to delete, an error if they may see but not delete it.
    pub(crate) async fn delete<T: Secured>(&self, stored: Option<&T>, read: impl Fn(&SecurityAttributes) -> ActionType<'_>, delete: impl Fn(&SecurityAttributes) -> ActionType<'_>) -> StoreResult<bool> {
        let Some(stored) = stored else { return Ok(false) };
        if !self.permits(read(stored.security_attributes())).await {
            return Ok(false);
        }
        self.allow(delete(stored.security_attributes())).await.map(|_| true)
    }
}

/// The actions that guard one kind of entity, and its name in errors: every
/// kind is read, listed, created, changed and deleted the same way (issue
/// 93), and a store implements the five operations once for all kinds.
pub(crate) struct Actions {
    pub(crate) entity: &'static str,
    pub(crate) get: for<'a> fn(&'a SecurityAttributes) -> ActionType<'a>,
    pub(crate) list: ActionType<'static>,
    pub(crate) create: for<'a> fn(&'a SecurityAttributes) -> ActionType<'a>,
    pub(crate) update: for<'a> fn(&'a SecurityAttributes) -> ActionType<'a>,
    pub(crate) delete: for<'a> fn(&'a SecurityAttributes) -> ActionType<'a>,
}

pub(crate) const PERSONALITIES: Actions = Actions {
    entity: "personality",
    get: |a| ActionType::GetPersonality(a),
    list: ActionType::ListPersonalities,
    create: |a| ActionType::CreatePersonality(a),
    update: |a| ActionType::UpdatePersonality(a),
    delete: |a| ActionType::DeletePersonality(a),
};

pub(crate) const SESSION_TEMPLATES: Actions = Actions {
    entity: "session_template",
    get: |a| ActionType::GetSessionTemplate(a),
    list: ActionType::ListSessionTemplates,
    create: |a| ActionType::CreateSessionTemplate(a),
    update: |a| ActionType::UpdateSessionTemplate(a),
    delete: |a| ActionType::DeleteSessionTemplate(a),
};

pub(crate) const AGENTS: Actions = Actions {
    entity: "agent",
    get: |a| ActionType::GetAgent(a),
    list: ActionType::ListAgents,
    create: |a| ActionType::CreateAgent(a),
    update: |a| ActionType::UpdateAgent(a),
    delete: |a| ActionType::DeleteAgent(a),
};

pub(crate) const SESSIONS: Actions = Actions {
    entity: "session",
    get: |a| ActionType::GetSession(a),
    list: ActionType::ListSessions,
    create: |a| ActionType::CreateSession(a),
    update: |a| ActionType::UpdateSession(a),
    delete: |a| ActionType::DeleteSession(a),
};

pub(crate) const CHANNELS: Actions = Actions {
    entity: "channel",
    get: |a| ActionType::GetChannel(a),
    list: ActionType::ListChannels,
    create: |a| ActionType::CreateChannel(a),
    update: |a| ActionType::UpdateChannel(a),
    delete: |a| ActionType::DeleteChannel(a),
};

pub(crate) const COMPONENTS: Actions = Actions {
    entity: "component",
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
    use crate::store_impl::memory::MemoryStore;
    use crate::user::UserId;
    use std::sync::Arc;

    /// Owners do everything, contributors read, nobody acts for another.
    #[derive(Debug)]
    struct OwnersAndContributors;

    #[async_trait::async_trait]
    impl SecurityManager for OwnersAndContributors {
        async fn check(&self, ctx: &SecurityContext, action: ActionType<'_>) -> bool {
            let me = ctx.initiator_id;
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
            id: SessionId::new(),
            metadata: Default::default(),
            participants: Vec::new(),
            channels: Vec::new(),
            commands: Default::default(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            security_attributes: SecurityAttributes { owner_id: owner, contributors },
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
    async fn nobody_creates_in_someone_elses_name() {
        let store = MemoryStore::with_security_manager(Arc::new(OwnersAndContributors));
        let (alice, _) = user();
        let (_, bob_ctx) = user();
        assert!(matches!(store.create_session(&bob_ctx, &session(alice, Vec::new())).await, Err(StoreError::PermissionDenied)));
    }
}
