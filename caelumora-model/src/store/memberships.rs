//! What the policy learns of actors from the store (issue 23): whether
//! someone is a participant of a session or channel an access list names,
//! and which roles count for them.

use crate::actor::ActorId;
use crate::channel::Participant;
use crate::role::{user_roles, RoleId};
use crate::security::{Membership, Memberships, SecurityContext};
use crate::store::{EntityStore, StoreResult, StoreResultExt};
use async_trait::async_trait;
use std::sync::{Arc, OnceLock, Weak};

/// `Memberships` read from the entity store in the system's name, each time
/// - the one session, channel, agent or user asked about, never a scan.
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

    /// The participants of `place` - also of one that was deleted: who took
    /// part does not change with it, and they are told of the deletion.
    async fn participants(store: &dyn EntityStore, place: Membership) -> StoreResult<Vec<Participant>> {
        let system = SecurityContext::system();
        Ok(match place {
            Membership::Session(id) => match store.get_session(&system, id).await.optional()? {
                Some(session) => session.participants,
                None => store.get_deleted_session(&system, id).await.optional()?.map(|s| s.participants).unwrap_or_default(),
            },
            Membership::Channel(id) => match store.get_channel(&system, id).await.optional()? {
                Some(channel) => channel.participants,
                None => store.get_deleted_channel(&system, id).await.optional()?.map(|c| c.participants).unwrap_or_default(),
            },
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
    async fn is_member_of(&self, actor: ActorId, place: Membership) -> bool {
        let Some(store) = self.store() else { return false };
        match Self::participants(store.as_ref(), place).await {
            Ok(participants) => participants.iter().any(|p| matches!(p, Participant::User(_) | Participant::Agent(_)) && p.actor_id() == actor),
            Err(e) => {
                tracing::warn!("Cannot read who takes part in {place:?}; nobody counts as a member: {e}");
                false
            }
        }
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
    use crate::channel::Channel;
    use crate::confidentiality::Classification;
    use crate::security::SecurityAttributes;
    use crate::store::memory::MemoryStore;
    use crate::user::{User, UserId};
    use crate::value::name::{Name, ScopedName};

    #[tokio::test]
    async fn a_member_is_who_takes_part_now_also_in_what_was_deleted() {
        let memberships = StoreMemberships::new();
        let (alice, bob) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        assert!(!memberships.is_member_of(alice, Membership::Channel(Default::default())).await, "nobody before a store is bound");
        let store: Arc<dyn EntityStore> = Arc::new(MemoryStore::new());
        memberships.bind(&store);
        let channel = Channel::build(Default::default(), alice, Name::new("talk").unwrap(), Default::default(), Classification::CHANNEL_DEFAULT, None, vec![Participant::User(match bob { ActorId::User(id) => id, _ => unreachable!() })]);
        store.create_channel(&SecurityContext::system(), &channel).await.unwrap();
        let place = Membership::Channel(channel.id);
        assert!(memberships.is_member_of(bob, place).await, "a participant");
        assert!(!memberships.is_member_of(alice, place).await, "not its owner, who is none");
        let approver = channel.participants.iter().find(|p| matches!(p, Participant::Approver(_))).map(Participant::actor_id).unwrap();
        assert!(!memberships.is_member_of(approver, place).await, "nor its approver");
        store.delete_channel(&SecurityContext::system(), channel.id).await.unwrap();
        assert!(memberships.is_member_of(bob, place).await, "who took part in what was deleted still did");
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
