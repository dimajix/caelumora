use crate::agent::{Agent, AgentChannelState, AgentId, AgentState, Personality};
use crate::artifact::Artifact;
use crate::channel::{Channel, ChannelField, ChannelId, ChannelState};
use crate::component::StoredComponents;
use crate::confidentiality::{ConfidentialityPolicy, ConfidentialityRules};
use crate::group::Group;
use crate::removal::{Deletable, Removal, Stored};
use crate::scope::{Scope, ScopeField};
use crate::security::{ActionType, DummySecurityManager, SecurityContext, SecurityManager};
use crate::session::{Session, SessionField, SessionId};
use crate::skill::Skill;
use crate::store::access::{name_free, Access};
use crate::store::query::{self, Filter, NoField, Page, Query};
use crate::store::{ComponentStore, Entities, EntityStore, EntityStores, Journaled, StateStore, StoreError, StoreResult};
use crate::template::SessionTemplate;
use crate::user::{User, UserId, UserProfile};
use crate::value::name::ScopedName;
use crate::workspace::Workspace;
use async_trait::async_trait;
use serde::Serialize;
use std::collections::HashMap;
use std::fmt::{self, Debug};
use std::future::ready;
use std::sync::{Arc, RwLock};
use tracing::info;

/// Every kind's entities, the preconfigured components and the runtime
/// state, in memory: a store per kind (`MemoryEntityStore`, issue 323),
/// and maps for the states.
#[derive(Debug)]
pub struct MemoryStore {
    personalities: Arc<MemoryEntityStore<Personality>>,
    session_templates: Arc<MemoryEntityStore<SessionTemplate>>,
    agents: MemoryEntityStore<Agent>,
    sessions: MemoryEntityStore<Session>,
    channels: MemoryEntityStore<Channel>,
    users: MemoryEntityStore<User>,
    user_profiles: Arc<MemoryEntityStore<UserProfile>>,
    workspaces: Arc<MemoryEntityStore<Workspace>>,
    skills: Arc<MemoryEntityStore<Skill>>,
    artifacts: Arc<MemoryEntityStore<Artifact>>,
    groups: Arc<MemoryEntityStore<Group>>,
    components: MemoryEntityStore<StoredComponents>,
    agent_states: RwLock<HashMap<AgentId, AgentState>>,
    agent_channel_states: RwLock<HashMap<(AgentId, ChannelId), AgentChannelState>>,
    channel_states: RwLock<HashMap<ChannelId, ChannelState>>,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::with_policies(Arc::new(DummySecurityManager::new()), Arc::new(ConfidentialityRules::default()))
    }
}

impl MemoryStore {
    /// A store that permits everything - for tests and throwaway setups.
    pub fn new() -> Self {
        Self::default()
    }

    /// A store that asks `security_manager` about every access to an
    /// entity, and `confidentiality` whether the caller is cleared for what
    /// is reached by clearance.
    pub fn with_policies(security_manager: Arc<dyn SecurityManager>, confidentiality: Arc<dyn ConfidentialityPolicy>) -> Self {
        info!("Creating memory store");
        let policies = (security_manager, confidentiality);
        Self {
            personalities: Arc::new(MemoryEntityStore::new(&policies)),
            session_templates: Arc::new(MemoryEntityStore::new(&policies)),
            agents: MemoryEntityStore::new(&policies),
            sessions: MemoryEntityStore::new(&policies),
            channels: MemoryEntityStore::new(&policies),
            users: MemoryEntityStore::new(&policies),
            user_profiles: Arc::new(MemoryEntityStore::new(&policies)),
            workspaces: Arc::new(MemoryEntityStore::new(&policies)),
            skills: Arc::new(MemoryEntityStore::new(&policies)),
            artifacts: Arc::new(MemoryEntityStore::new(&policies)),
            groups: Arc::new(MemoryEntityStore::new(&policies)),
            components: MemoryEntityStore::new(&policies),
            agent_states: Default::default(),
            agent_channel_states: Default::default(),
            channel_states: Default::default(),
        }
    }
}

impl MemoryStore {
    /// The store of every kind (issue 323): this store's, the kinds that
    /// have one of their own as such.
    pub fn entity_stores(self: &Arc<Self>) -> EntityStores {
        EntityStores {
            entities: Arc::<Self>::clone(self),
            skills: Arc::<MemoryEntityStore<Skill>>::clone(&self.skills),
            workspaces: Arc::<MemoryEntityStore<Workspace>>::clone(&self.workspaces),
            artifacts: Arc::<MemoryEntityStore<Artifact>>::clone(&self.artifacts),
            groups: Arc::<MemoryEntityStore<Group>>::clone(&self.groups),
            session_templates: Arc::<MemoryEntityStore<SessionTemplate>>::clone(&self.session_templates),
            user_profiles: Arc::<MemoryEntityStore<UserProfile>>::clone(&self.user_profiles),
            personalities: Arc::<MemoryEntityStore<Personality>>::clone(&self.personalities),
        }
    }
}

// A poisoned lock is used anyway: the locks are held only by the closures
// passed here, none of which can leave a map half-changed.
fn read_lock<T, F, R>(lock: &RwLock<T>, f: F) -> R
where
    F: FnOnce(&T) -> R,
{
    match lock.read() {
        Ok(guard) => f(&guard),
        Err(poisoned) => f(poisoned.get_ref()),
    }
}

fn write_lock<T, F, R>(lock: &RwLock<T>, f: F) -> R
where
    F: FnOnce(&mut T) -> R,
{
    match lock.write() {
        Ok(mut guard) => f(&mut guard),
        Err(mut poisoned) => f(poisoned.get_mut()),
    }
}

/// The entities of one kind in memory (issue 323), checked as every kind is
/// (`store::access`).
pub struct MemoryEntityStore<T: Stored> {
    security_manager: Arc<dyn SecurityManager>,
    /// Who is cleared for what is reached by clearance (issue 322).
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    entities: RwLock<HashMap<T::Id, T>>,
}

impl<T: Stored> Debug for MemoryEntityStore<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryEntityStore").field("kind", &T::KIND).finish_non_exhaustive()
    }
}

impl<T: Stored + Clone + Serialize + Send + Sync> MemoryEntityStore<T> {
    /// An empty store that asks the `SecurityManager` and the
    /// `ConfidentialityPolicy` of `policies`.
    fn new(policies: &(Arc<dyn SecurityManager>, Arc<dyn ConfidentialityPolicy>)) -> Self {
        Self { security_manager: Arc::clone(&policies.0), confidentiality: Arc::clone(&policies.1), entities: Default::default() }
    }

    fn access<'a>(&'a self, ctx: &'a SecurityContext) -> Access<'a> {
        Access::new(self.security_manager.as_ref(), self.confidentiality.as_ref(), ctx)
    }

    fn stored(&self, id: &T::Id) -> Option<T> {
        read_lock(&self.entities, |v| v.get(id).cloned())
    }

    /// Stores `value` as `update` and `restore` do: the others of its kind
    /// checked for its name and it written, in one step.
    fn write(&self, value: &T) -> StoreResult<()> {
        let id = value.id();
        write_lock(&self.entities, |v| {
            name_free(value, v.iter().filter(|(other, _)| **other != id).map(|(_, other)| other))?;
            v.insert(id, value.clone());
            Ok(())
        })
    }

    async fn create(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()> {
        self.access(ctx).create(value).await?;
        let id = value.id();
        write_lock(&self.entities, |v| {
            if v.contains_key(&id) {
                return Err(StoreError::Conflict(format!("{} {} already exists", T::KIND, id)));
            }
            name_free(value, v.values())?;
            v.insert(id, value.clone());
            Ok(())
        })
    }

    async fn get(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        self.access(ctx).read(self.stored(&id), id.to_string()).await
    }

    async fn update(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()> {
        let id = value.id();
        self.access(ctx).change(self.stored(&id).as_ref(), value, id.to_string()).await?;
        self.write(value)
    }

    /// Removes the entity for good: deleting a kind that is not marked.
    async fn remove(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<bool> {
        if !self.access(ctx).delete(self.stored(&id).as_ref()).await? {
            return Ok(false);
        }
        Ok(write_lock(&self.entities, |v| v.remove(&id).is_some()))
    }

    /// The entity of `id`, if it is there and meets `filter`.
    fn meeting(&self, id: &T::Id, filter: Option<&Filter<T::Field>>) -> StoreResult<Option<T>> {
        match self.stored(id) {
            Some(value) => query::meeting(value, filter),
            None => Ok(None),
        }
    }

    /// The page `query` asks for, among what is deleted if `deleted`.
    async fn page(&self, ctx: &SecurityContext, query: &Query<T::Field>, deleted: bool) -> StoreResult<Page<T>> {
        let access = self.access(ctx);
        access.allow(ActionType::List(T::KIND)).await?;
        let keys = read_lock(&self.entities, |v| v.keys().map(|id| (id.to_string(), id.clone())).collect());
        let filter = query.filter.as_ref();
        access.page(keys, query, deleted, |id: T::Id| ready(self.meeting(&id, filter))).await
    }
}

#[async_trait]
impl<T: Deletable + Clone + Serialize + Send + Sync + 'static> EntityStore<T> for MemoryEntityStore<T> {
    async fn create(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()> {
        MemoryEntityStore::create(self, ctx, value).await
    }

    async fn get(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        MemoryEntityStore::get(self, ctx, id).await
    }

    async fn update(&self, ctx: &SecurityContext, value: &T) -> StoreResult<()> {
        MemoryEntityStore::update(self, ctx, value).await
    }

    /// Marks the entity deleted, by the caller and now (issue 164).
    async fn delete(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<bool> {
        let stored = self.stored(&id);
        if !self.access(ctx).delete(stored.as_ref()).await? {
            return Ok(false);
        }
        let Some(mut marked) = stored else { return Ok(false) };
        marked.set_deleted(Some(Removal::now(ctx.caller_id)));
        write_lock(&self.entities, |v| v.insert(id, marked));
        Ok(true)
    }

    async fn list(&self, ctx: &SecurityContext, query: &Query<T::Field>) -> StoreResult<Page<T>> {
        self.page(ctx, query, false).await
    }

    async fn restore(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        let mut restored = self.access(ctx).restore(self.stored(&id), id.to_string()).await?;
        restored.set_deleted(None);
        self.write(&restored)?;
        Ok(restored)
    }

    async fn purge(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<bool> {
        if !self.access(ctx).purge(self.stored(&id).as_ref()).await? {
            return Ok(false);
        }
        // Only if it is still marked: restored while the purge was asked
        // about, it stays.
        Ok(write_lock(&self.entities, |v| {
            let marked = v.get(&id).is_some_and(|value| value.deleted().is_some());
            marked && v.remove(&id).is_some()
        }))
    }

    async fn get_deleted(&self, ctx: &SecurityContext, id: T::Id) -> StoreResult<T> {
        self.access(ctx).read_deleted(self.stored(&id), id.to_string()).await
    }

    async fn list_deleted(&self, ctx: &SecurityContext, query: &Query<T::Field>) -> StoreResult<Page<T>> {
        self.page(ctx, query, true).await
    }

    async fn find_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<T> {
        let all = self.page(ctx, &Query::all(), false).await?.items;
        all.into_iter().find(|value| value.unique_name().as_ref() == Some(name))
            .ok_or_else(|| StoreError::NotFound { entity: T::KIND.name(), id: name.to_string() })
    }
}

#[async_trait]
impl Entities for MemoryStore {
    async fn create_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
        self.agents.create(ctx, value).await
    }
    async fn get_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent> {
        self.agents.get(ctx, id).await
    }
    async fn update_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
        self.agents.update(ctx, value).await
    }
    async fn delete_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<bool> {
        self.agents.delete(ctx, id).await
    }
    async fn list_agents(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Agent>> {
        self.agents.list(ctx, query).await
    }
    async fn restore_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent> {
        self.agents.restore(ctx, id).await
    }
    async fn purge_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<bool> {
        self.agents.purge(ctx, id).await
    }
    async fn get_deleted_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent> {
        self.agents.get_deleted(ctx, id).await
    }
    async fn list_deleted_agents(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Agent>> {
        self.agents.list_deleted(ctx, query).await
    }
    async fn find_agent_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Agent> {
        self.agents.find_by_name(ctx, name).await
    }
    async fn create_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()> {
        self.sessions.create(ctx, value).await
    }
    async fn get_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session> {
        self.sessions.get(ctx, id).await
    }
    async fn update_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()> {
        self.sessions.update(ctx, value).await
    }
    async fn delete_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<bool> {
        self.sessions.delete(ctx, id).await
    }
    async fn list_sessions(&self, ctx: &SecurityContext, query: &Query<SessionField>) -> StoreResult<Page<Session>> {
        self.sessions.list(ctx, query).await
    }
    async fn restore_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session> {
        self.sessions.restore(ctx, id).await
    }
    async fn purge_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<bool> {
        self.sessions.purge(ctx, id).await
    }
    async fn get_deleted_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session> {
        self.sessions.get_deleted(ctx, id).await
    }
    async fn list_deleted_sessions(&self, ctx: &SecurityContext, query: &Query<SessionField>) -> StoreResult<Page<Session>> {
        self.sessions.list_deleted(ctx, query).await
    }
    async fn create_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()> {
        self.channels.create(ctx, value).await
    }
    async fn get_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<Channel> {
        self.channels.get(ctx, id).await
    }
    async fn update_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()> {
        self.channels.update(ctx, value).await
    }
    async fn delete_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<bool> {
        self.channels.delete(ctx, id).await
    }
    async fn list_channels(&self, ctx: &SecurityContext, query: &Query<ChannelField>) -> StoreResult<Page<Channel>> {
        self.channels.list(ctx, query).await
    }
    async fn restore_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<Channel> {
        self.channels.restore(ctx, id).await
    }
    async fn purge_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<bool> {
        self.channels.purge(ctx, id).await
    }
    async fn get_deleted_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<Channel> {
        self.channels.get_deleted(ctx, id).await
    }
    async fn list_deleted_channels(&self, ctx: &SecurityContext, query: &Query<ChannelField>) -> StoreResult<Page<Channel>> {
        self.channels.list_deleted(ctx, query).await
    }
    async fn create_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()> {
        self.users.create(ctx, value).await
    }
    async fn get_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<User> {
        self.users.get(ctx, id).await
    }
    async fn update_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()> {
        self.users.update(ctx, value).await
    }
    async fn delete_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool> {
        self.users.delete(ctx, id).await
    }
    async fn list_users(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<User>> {
        self.users.list(ctx, query).await
    }
    async fn restore_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<User> {
        self.users.restore(ctx, id).await
    }
    async fn purge_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool> {
        self.users.purge(ctx, id).await
    }
    async fn get_deleted_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<User> {
        self.users.get_deleted(ctx, id).await
    }
    async fn list_deleted_users(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<User>> {
        self.users.list_deleted(ctx, query).await
    }
    async fn find_user_by_name(&self, ctx: &SecurityContext, name: &str) -> StoreResult<User> {
        let not_found = || StoreError::NotFound { entity: User::KIND.name(), id: name.to_string() };
        let name = ScopedName::new(name, Scope::GlobalScope).map_err(|_| not_found())?;
        self.users.find_by_name(ctx, &name).await
    }
}

/// Preconfigured components, kept like the entities (issue 164).
#[async_trait]
impl ComponentStore for MemoryStore {
    async fn create_components(&self, ctx: &SecurityContext, value: &StoredComponents) -> StoreResult<()> {
        self.components.create(ctx, value).await
    }
    async fn get_components(&self, ctx: &SecurityContext, id: &str) -> StoreResult<StoredComponents> {
        self.components.get(ctx, id.to_string()).await
    }
    async fn update_components(&self, ctx: &SecurityContext, value: &StoredComponents) -> StoreResult<()> {
        self.components.update(ctx, value).await
    }
    async fn delete_components(&self, ctx: &SecurityContext, id: &str) -> StoreResult<bool> {
        self.components.remove(ctx, id.to_string()).await
    }
    async fn list_components(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<StoredComponents>> {
        self.components.page(ctx, query, false).await
    }
}

#[async_trait]
impl StateStore for MemoryStore {
    async fn get_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId) -> StoreResult<AgentChannelState> {
        Ok(read_lock(&self.agent_channel_states, |v| v.get(&(agent_id, channel_id)).cloned()).unwrap_or_default())
    }

    async fn put_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId, state: &mut AgentChannelState) -> StoreResult<()> {
        // Marked first: the copy kept is the state as stored, with nothing pending.
        state.mark_stored();
        write_lock(&self.agent_channel_states, |v| v.insert((agent_id, channel_id), state.clone()));
        Ok(())
    }

    async fn get_agent_state(&self, agent_id: AgentId) -> StoreResult<AgentState> {
        Ok(read_lock(&self.agent_states, |v| v.get(&agent_id).cloned()).unwrap_or_default())
    }

    async fn put_agent_state(&self, agent_id: AgentId, state: &mut AgentState) -> StoreResult<()> {
        // Marked first: the copy kept is the state as stored, with nothing pending.
        state.mark_stored();
        write_lock(&self.agent_states, |v| v.insert(agent_id, state.clone()));
        Ok(())
    }

    async fn get_channel_state(&self, channel_id: ChannelId) -> StoreResult<ChannelState> {
        read_lock(&self.channel_states, |v| v.get(&channel_id).cloned()).ok_or_else(|| StoreError::NotFound { entity: "channel_state", id: channel_id.to_string() })
    }

    async fn put_channel_state(&self, channel_id: ChannelId, state: &mut ChannelState) -> StoreResult<()> {
        // Marked first: the copy kept is the state as stored, with nothing pending.
        state.mark_stored();
        write_lock(&self.channel_states, |v| v.insert(channel_id, state.clone()));
        Ok(())
    }

    async fn delete_channel_state(&self, channel_id: ChannelId) -> StoreResult<()> {
        write_lock(&self.channel_states, |v| v.remove(&channel_id));
        write_lock(&self.agent_channel_states, |v| v.retain(|(_, cid), _| *cid != channel_id));
        Ok(())
    }

    async fn delete_agent_state(&self, agent_id: AgentId) -> StoreResult<()> {
        write_lock(&self.agent_states, |v| v.remove(&agent_id));
        write_lock(&self.agent_channel_states, |v| v.retain(|(aid, _), _| *aid != agent_id));
        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::suite;
    use crate::confidentiality::ConfidentialityRules;
    use crate::security::OwnershipSecurityManager;

    #[tokio::test]
    async fn behaves_as_an_entity_store() {
        suite::entity_store_behaves(&Arc::new(MemoryStore::new()).entity_stores()).await;
    }

    #[tokio::test]
    async fn answers_queries() {
        suite::queries_behave(&Arc::new(MemoryStore::new()).entity_stores()).await;
    }

    /// A limit counts what the caller may read (issue 293): the policy is
    /// asked as the list is read, and a page is filled from further on.
    #[tokio::test]
    async fn a_limit_counts_what_the_caller_may_read() {
        use crate::actor::ActorId;
        use crate::artifact::tests::version;
        use crate::artifact::{Artifact, ArtifactId};
        use crate::confidentiality::Classification;
        use crate::scope::Scope;
        use crate::security::OwnershipSecurityManager;
        let store = Arc::new(MemoryStore::with_policies(Arc::new(OwnershipSecurityManager::new()), Arc::new(ConfidentialityRules::default()))).entity_stores().artifacts;
        let (alice, bob) = (UserId::new(), UserId::new());
        let mut hers = Vec::new();
        for (index, owner) in [alice, bob, bob, alice, bob, alice].into_iter().enumerate() {
            let name = ScopedName::new(format!("a{index}.csv"), Scope::UserScope { user_id: owner }).unwrap();
            let artifact = Artifact::new(ArtifactId::new(), name, "files", Classification::CHANNEL_DEFAULT, owner, version(b"a,b", SessionId::new()));
            store.create(&SecurityContext::system(), &artifact).await.unwrap();
            if owner == alice {
                hers.push(artifact.id);
            }
        }
        hers.sort_by_key(|id| id.to_string());
        let ctx = SecurityContext::new(ActorId::User(alice), Default::default(), Default::default());
        let mut read = Vec::new();
        let mut query = Query::all().limited(1);
        loop {
            let page = store.list(&ctx, &query).await.unwrap();
            read.extend(page.items.iter().map(|a| a.id));
            match page.next {
                Some(cursor) => {
                    assert_eq!(page.items.len(), 1, "every page that goes on is full, however many of bob's lie between");
                    query = Query::all().limited(1).after(cursor);
                }
                // The last page may be empty: what followed was bob's.
                None => break,
            }
        }
        assert_eq!(read, hers, "hers alone, in the order of their ids");
    }

    #[tokio::test]
    async fn behaves_as_a_state_store() {
        suite::state_store_behaves(&MemoryStore::new()).await;
    }

    #[tokio::test]
    async fn behaves_as_a_component_store() {
        suite::component_store_behaves(&MemoryStore::new()).await;
    }

    #[tokio::test]
    async fn keeps_what_is_deleted_marked() {
        suite::deleting_marks(&Arc::new(MemoryStore::new()).entity_stores()).await;
    }

    #[tokio::test]
    async fn keeps_names_unique() {
        suite::names_are_unique(&Arc::new(MemoryStore::new()).entity_stores()).await;
        let owners = MemoryStore::with_policies(Arc::new(OwnershipSecurityManager::new()), Arc::new(ConfidentialityRules::default()));
        suite::names_are_unique_beyond_what_is_read(&Arc::new(owners).entity_stores()).await;
    }
}
