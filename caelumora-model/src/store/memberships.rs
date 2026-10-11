//! What the policy learns of actors from the store (issues 23, 286): who
//! belongs to a group an access list names, and which roles count for
//! someone.

use crate::actor::ActorId;
use crate::group::GroupId;
use crate::role::{user_roles, RoleId};
use crate::security::{Memberships, SecurityContext};
use crate::store::{EntityStore, StoreResult, StoreResultExt};
use async_trait::async_trait;
use std::sync::{Arc, OnceLock, Weak};

/// `Memberships` read from the entity store in the system's name, each time
/// - the one group, agent or user asked about, never a scan.
///
/// The policy is made before the store, which takes it, so the store is
/// bound afterwards (`bind`) and held weakly: the store holds the policy,
/// the policy this. Until it is bound, and once the store is gone, nobody is
/// a member of anything and holds no role - the policy refuses rather than
/// grants.
#[derive(Debug, Default)]
pub struct StoreMemberships {
    store: OnceLock<Weak<dyn EntityStore>>,
}

impl StoreMemberships {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads from `store` from now on. Bound once; binding again is
    /// ignored, with an error in the log.
    pub fn bind(&self, store: &Arc<dyn EntityStore>) {
        if self.store.set(Arc::downgrade(store)).is_err() {
            tracing::error!("The memberships were bound to an entity store twice; the first stays");
        }
    }

    fn store(&self) -> Option<Arc<dyn EntityStore>> {
        let store = self.store.get().and_then(Weak::upgrade);
        if store.is_none() {
            tracing::error!("Memberships were asked for before an entity store was bound; nobody is a member");
        }
        store
    }

    /// The members of `group` - also of one that was deleted: who belonged
    /// to it does not change with that, and the participants of a deleted
    /// session or channel are told of the deletion.
    async fn members(store: &dyn EntityStore, group: GroupId) -> StoreResult<Vec<ActorId>> {
        let system = SecurityContext::system();
        Ok(match store.get_group(&system, group).await.optional()? {
            Some(group) => group.members,
            None => store.get_deleted_group(&system, group).await.optional()?.map(|g| g.members).unwrap_or_default(),
        })
    }

    /// The roles that count for an agent: its owner's, narrowed by its
    /// personality's limiting roles. None for one whose personality is
    /// gone - what it was limited to cannot be told.
    async fn agent_roles(store: &dyn EntityStore, agent: crate::agent::AgentId) -> StoreResult<Vec<RoleId>> {
        let system = SecurityContext::system();
        let Some(agent) = store.get_agent(&system, agent).await.optional()? else { return Ok(Vec::new()) };
        let owners = user_roles(store, agent.security_attributes.owner_id).await?;
        let Some(personality) = store.get_personality(&system, agent.personality_id).await.optional()? else { return Ok(Vec::new()) };
        Ok(match personality.limiting_roles() {
            Some(limit) => owners.into_iter().filter(|role| limit.contains(role)).collect(),
            None => owners,
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

    async fn roles_of(&self, actor: ActorId) -> Vec<RoleId> {
        let Some(store) = self.store() else { return Vec::new() };
        let roles = match actor {
            ActorId::Agent(agent) => Self::agent_roles(store.as_ref(), agent).await,
            other => user_roles(store.as_ref(), other).await,
        };
        roles.unwrap_or_else(|e| {
            tracing::warn!("Cannot read the roles of {actor}; none count: {e}");
            Vec::new()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, Personality, PersonalityId, PersonalityKind};
    use crate::group::Group;
    use crate::security::SecurityAttributes;
    use crate::store::memory::MemoryStore;
    use crate::user::{User, UserId};
    use crate::value::name::{Name, ScopedName};

    #[tokio::test]
    async fn the_members_of_a_group_are_read_also_of_one_that_was_deleted() {
        let memberships = StoreMemberships::new();
        let alice = UserId::new();
        let bob = ActorId::User(UserId::new());
        let team = Group { members: vec![bob], ..Group::team(ScopedName::new("project", Default::default()).unwrap(), alice) };
        assert!(memberships.members_of(team.id).await.is_empty(), "nobody before a store is bound");
        let store: Arc<dyn EntityStore> = Arc::new(MemoryStore::new());
        memberships.bind(&store);
        store.create_group(&SecurityContext::system(), &team).await.unwrap();
        assert_eq!(memberships.members_of(team.id).await, [bob]);
        assert!(memberships.members_of(GroupId::new()).await.is_empty(), "nobody of a group that does not exist");
        store.delete_group(&SecurityContext::system(), team.id).await.unwrap();
        assert_eq!(memberships.members_of(team.id).await, [bob], "who belonged to what was deleted still did");
    }

    #[tokio::test]
    async fn an_agent_holds_its_owners_roles_narrowed_by_its_personality() {
        let store: Arc<dyn EntityStore> = Arc::new(MemoryStore::new());
        let memberships = StoreMemberships::new();
        memberships.bind(&store);
        let (hr, staff) = (RoleId::new(), RoleId::new());
        let system = SecurityContext::system();
        let owner = User { id: UserId::new(), username: Name::new("alice").unwrap(), email: String::new(), roles: vec![hr, staff], deleted: None };
        store.create_user(&system, &owner).await.unwrap();
        let personality = |roles: Option<Vec<RoleId>>| Personality {
            id: PersonalityId::new(), name: ScopedName::new("helper", Default::default()).unwrap(), kind: PersonalityKind::Internal, roles,
            description: None, introduction: None, components: Default::default(), security_attributes: SecurityAttributes::default(),
            reasoning_visibility: Default::default(), deleted: None,
        };
        let (limited, free) = (personality(Some(vec![staff])), personality(None));
        for p in [&limited, &free] {
            store.create_personality(&system, &Personality { name: ScopedName::new(p.id.to_string(), Default::default()).unwrap(), ..p.clone() }).await.unwrap();
        }
        let agent = |p: &Personality| Agent::new(ScopedName::new(format!("a{}", p.id), Default::default()).unwrap(), p, ActorId::User(owner.id));
        let (narrowed, whole) = (agent(&limited), agent(&free));
        for a in [&narrowed, &whole] {
            store.create_agent(&system, a).await.unwrap();
        }
        assert_eq!(memberships.roles_of(ActorId::User(owner.id)).await, vec![hr, staff], "a user's own");
        assert_eq!(memberships.roles_of(ActorId::Agent(narrowed.id)).await, vec![staff], "an agent's owner's, narrowed");
        assert_eq!(memberships.roles_of(ActorId::Agent(whole.id)).await, vec![hr, staff], "unless its personality names none");
    }
}
