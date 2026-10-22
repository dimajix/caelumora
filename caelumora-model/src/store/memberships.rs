//! What the policy learns of actors from the store (issues 23, 286): who
//! belongs to a group an access list names.

use crate::actor::ActorId;
use crate::group::{Group, GroupId};
use crate::security::{Memberships, SecurityContext};
use crate::store::{EntityStore, StoreResult, StoreResultExt};
use async_trait::async_trait;
use std::sync::{Arc, OnceLock, Weak};

/// `Memberships` read from the store of groups in the system's name, each
/// time - the one group asked about, never a scan.
///
/// The policy is made before the store, which takes it, so the store is
/// bound afterwards (`bind`) and held weakly: the store holds the policy,
/// the policy this. Until it is bound, and once the store is gone, nobody is
/// a member of anything - the policy refuses rather than grants.
#[derive(Debug, Default)]
pub struct StoreMemberships {
    store: OnceLock<Weak<dyn EntityStore<Group>>>,
}

impl StoreMemberships {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads from `store` from now on. Bound once; binding again is
    /// ignored, with an error in the log.
    pub fn bind(&self, store: &Arc<dyn EntityStore<Group>>) {
        if self.store.set(Arc::downgrade(store)).is_err() {
            tracing::error!("The memberships were bound to a store of groups twice; the first stays");
        }
    }

    fn store(&self) -> Option<Arc<dyn EntityStore<Group>>> {
        let store = self.store.get().and_then(Weak::upgrade);
        if store.is_none() {
            tracing::error!("Memberships were asked for before a store of groups was bound; nobody is a member");
        }
        store
    }

    /// The members of `group` - also of one that was deleted: who belonged
    /// to it does not change with that, and the participants of a deleted
    /// session or channel are told of the deletion.
    async fn members(store: &dyn EntityStore<Group>, group: GroupId) -> StoreResult<Vec<ActorId>> {
        let system = SecurityContext::system();
        Ok(match store.get(&system, group).await.optional()? {
            Some(group) => group.members,
            None => store.get_deleted(&system, group).await.optional()?.map(|g| g.members).unwrap_or_default(),
        })
    }
}

#[async_trait]
impl Memberships for StoreMemberships {
    async fn members_of(&self, group: GroupId) -> Vec<ActorId> {
        let Some(store) = self.store() else { return Vec::new() };
        Self::members(store.as_ref(), group).await.unwrap_or_else(|e| {
            tracing::warn!("Cannot read who belongs to group {group}; nobody counts as a member: {e}");
            Vec::new()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Persistence;
    use crate::user::UserId;
    use crate::value::name::ScopedName;

    #[tokio::test]
    async fn the_members_of_a_group_are_read_also_of_one_that_was_deleted() {
        let memberships = StoreMemberships::new();
        let alice = UserId::new();
        let bob = ActorId::User(UserId::new());
        let team = Group { members: vec![bob], ..Group::team(ScopedName::new("project", Default::default()).unwrap(), alice) };
        assert!(memberships.members_of(team.id).await.is_empty(), "nobody before a store is bound");
        let store = Persistence::in_memory().entity_stores().unwrap().groups;
        memberships.bind(&store);
        store.create(&SecurityContext::system(), &team).await.unwrap();
        assert_eq!(memberships.members_of(team.id).await, [bob]);
        assert!(memberships.members_of(GroupId::new()).await.is_empty(), "nobody of a group that does not exist");
        store.delete(&SecurityContext::system(), team.id).await.unwrap();
        assert_eq!(memberships.members_of(team.id).await, [bob], "who belonged to what was deleted still did");
    }

}
