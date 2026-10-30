//! What the policy learns of actors from the store (issues 23, 286): the
//! group an access list names, with who belongs to it.

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
            tracing::error!(
                "The memberships were bound to a store of groups twice; the first stays"
            );
        }
    }

    fn store(&self) -> Option<Arc<dyn EntityStore<Group>>> {
        let store = self.store.get().and_then(Weak::upgrade);
        if store.is_none() {
            tracing::error!(
                "Memberships were asked for before a store of groups was bound; nobody is a member"
            );
        }
        store
    }

    /// The group `id` - also one that was deleted: who belonged to it does
    /// not change with that, and the participants of a deleted session or
    /// channel are told of the deletion.
    async fn find(store: &dyn EntityStore<Group>, id: GroupId) -> StoreResult<Option<Group>> {
        let system = SecurityContext::system();
        match store.get(&system, id).await.optional()? {
            Some(group) => Ok(Some(group)),
            None => {
                Ok(store.get_deleted(&system, id).await.optional()?.map(|deleted| deleted.entity))
            }
        }
    }
}

#[async_trait]
impl Memberships for StoreMemberships {
    async fn group(&self, id: GroupId) -> Option<Group> {
        let store = self.store()?;
        Self::find(store.as_ref(), id).await.unwrap_or_else(|e| {
            tracing::warn!("Cannot read who belongs to group {id}; nobody counts as a member: {e}");
            None
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::ActorId;
    use crate::removal::Removal;
    use crate::store::Persistence;
    use crate::user::UserId;
    use crate::value::name::ScopedName;

    #[tokio::test]
    async fn a_group_is_read_also_once_it_was_deleted() {
        let memberships = StoreMemberships::new();
        let alice = UserId::new();
        let bob = ActorId::User(UserId::new());
        let mut team = Group {
            members: vec![bob],
            ..Group::team(ScopedName::new("project", Default::default()).unwrap(), alice)
        };
        assert!(memberships.group(team.id).await.is_none(), "none before a store is bound");
        let store = Persistence::in_memory().entity_stores().unwrap().groups;
        memberships.bind(&store);
        store.create(&SecurityContext::system(), &mut team).await.unwrap();
        let kept = memberships.group(team.id).await.expect("the group");
        assert_eq!((kept.members, kept.deleted), (vec![bob], None));
        assert!(memberships.group(GroupId::new()).await.is_none(), "none that does not exist");
        store
            .delete(&SecurityContext::system(), team.id, &Removal::now(ActorId::System))
            .await
            .unwrap();
        let deleted = memberships.group(team.id).await.expect("the deleted group");
        assert_eq!(deleted.members, [bob], "who belonged to what was deleted still did");
        assert!(deleted.deleted.is_some(), "and that it was deleted is said");
    }
}
