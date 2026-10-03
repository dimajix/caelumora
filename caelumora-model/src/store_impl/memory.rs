use crate::agent::{Agent, AgentChannelState, AgentId, AgentState, Personality, PersonalityId};
use crate::channel::{Channel, ChannelId, ChannelState};
use crate::component::ComponentDescriptor;
use crate::session::{Session, SessionId};
use crate::store::{EntityStore, Journaled, StateStore, StoreError, StoreResult};
use crate::template::{SessionTemplate, SessionTemplateId};
use async_trait::async_trait;
use tracing::info;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use crate::security::{DummySecurityManager, Secured, SecurityManager};
use crate::store_impl::access::{Access, Actions, AGENTS, CHANNELS, COMPONENTS, PERSONALITIES, SESSIONS, SESSION_TEMPLATES, USERS, USER_PROFILES};
use std::fmt::Display;
use std::hash::Hash;

#[derive(Debug)]
pub struct MemoryStore {
    security_manager: Arc<dyn SecurityManager>,
    personalities: RwLock<HashMap<PersonalityId, Personality>>,
    session_templates: RwLock<HashMap<SessionTemplateId, SessionTemplate>>,
    agents: RwLock<HashMap<AgentId, Agent>>,
    sessions: RwLock<HashMap<SessionId, Session>>,
    channels: RwLock<HashMap<ChannelId, Channel>>,
    users: RwLock<HashMap<UserId, User>>,
    user_profiles: RwLock<HashMap<UserId, UserProfile>>,
    agent_states: RwLock<HashMap<AgentId, AgentState>>,
    agent_channel_states: RwLock<HashMap<(AgentId, ChannelId), AgentChannelState>>,
    channel_states: RwLock<HashMap<ChannelId, ChannelState>>,
    components: RwLock<HashMap<String, ComponentDescriptor>>,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::with_security_manager(Arc::new(DummySecurityManager::new()))
    }
}

impl MemoryStore {
    /// A store that permits everything - for tests and throwaway setups.
    pub fn new() -> Self {
        Self::default()
    }

    /// A store that asks `security_manager` about every access to an entity.
    pub fn with_security_manager(security_manager: Arc<dyn SecurityManager>) -> Self {
        info!("Creating memory store");
        Self {
            security_manager,
            personalities: Default::default(),
            session_templates: Default::default(),
            agents: Default::default(),
            sessions: Default::default(),
            channels: Default::default(),
            users: Default::default(),
            user_profiles: Default::default(),
            agent_states: Default::default(),
            agent_channel_states: Default::default(),
            channel_states: Default::default(),
            components: Default::default(),
        }
    }

    fn access<'a>(&'a self, ctx: &'a SecurityContext) -> Access<'a> {
        Access::new(self.security_manager.as_ref(), ctx)
    }

    // A poisoned lock is used anyway: the locks are held only by the
    // closures passed here, none of which can leave a map half-changed.
    fn read_lock<T, F, R>(&self, lock: &RwLock<T>, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        match lock.read() {
            Ok(guard) => f(&guard),
            Err(poisoned) => f(poisoned.get_ref()),
        }
    }

    fn write_lock<T, F, R>(&self, lock: &RwLock<T>, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        match lock.write() {
            Ok(mut guard) => f(&mut guard),
            Err(mut poisoned) => f(poisoned.get_mut()),
        }
    }
}

/// The five operations of every entity kind (issue 93), on the map that
/// holds that kind and checked with its `Actions`.
impl MemoryStore {
    async fn create_in<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: K, value: &T) -> StoreResult<()>
    where K: Eq + Hash + Display + Send + Sync, T: Secured + Clone + Send + Sync {
        self.access(ctx).create(value, kind.create).await?;
        self.write_lock(map, |v| {
            if v.contains_key(&key) {
                return Err(StoreError::Conflict(format!("{} {} already exists", kind.entity, key)));
            }
            v.insert(key, value.clone());
            Ok(())
        })
    }

    async fn get_in<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: &K) -> StoreResult<T>
    where K: Eq + Hash + Display + Send + Sync, T: Secured + Clone + Send + Sync {
        let found = self.read_lock(map, |v| v.get(key).cloned());
        self.access(ctx).read(found, kind.get, kind.entity, key.to_string()).await
    }

    async fn update_in<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: K, value: &T) -> StoreResult<()>
    where K: Eq + Hash + Display + Send + Sync, T: Secured + Clone + Send + Sync {
        let stored = self.read_lock(map, |v| v.get(&key).cloned());
        self.access(ctx).change(stored.as_ref(), kind.get, kind.update, kind.entity, key.to_string()).await?;
        self.write_lock(map, |v| v.insert(key, value.clone()));
        Ok(())
    }

    async fn delete_in<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: &K) -> StoreResult<bool>
    where K: Eq + Hash + Display + Send + Sync, T: Secured + Clone + Send + Sync {
        let stored = self.read_lock(map, |v| v.get(key).cloned());
        if !self.access(ctx).delete(stored.as_ref(), kind.get, kind.delete).await? {
            return Ok(false);
        }
        Ok(self.write_lock(map, |v| v.remove(key).is_some()))
    }

    async fn list_in<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions) -> StoreResult<Vec<T>>
    where K: Eq + Hash + Send + Sync, T: Secured + Clone + Send + Sync {
        let access = self.access(ctx);
        access.allow(kind.list).await?;
        let all = self.read_lock(map, |v| v.values().cloned().collect());
        Ok(access.filter(all, kind.get).await)
    }
}

fn require_entity<T>(value: Option<T>, entity: &'static str, id: String) -> StoreResult<T> {
    value.ok_or(StoreError::NotFound { entity, id })
}

use crate::security::SecurityContext;
use crate::user::{User, UserId, UserProfile};

#[async_trait]
impl EntityStore for MemoryStore {
    // Personalities
    async fn create_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()> {
        self.create_in(ctx, &self.personalities, &PERSONALITIES, value.id, value).await
    }
    async fn get_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<Personality> {
        self.get_in(ctx, &self.personalities, &PERSONALITIES, &id).await
    }
    async fn update_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()> {
        self.update_in(ctx, &self.personalities, &PERSONALITIES, value.id, value).await
    }
    async fn delete_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<bool> {
        self.delete_in(ctx, &self.personalities, &PERSONALITIES, &id).await
    }
    async fn list_personalities(&self, ctx: &SecurityContext) -> StoreResult<Vec<Personality>> {
        self.list_in(ctx, &self.personalities, &PERSONALITIES).await
    }

    // Session templates
    async fn create_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()> {
        self.create_in(ctx, &self.session_templates, &SESSION_TEMPLATES, value.id, value).await
    }
    async fn get_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<SessionTemplate> {
        self.get_in(ctx, &self.session_templates, &SESSION_TEMPLATES, &id).await
    }
    async fn update_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()> {
        self.update_in(ctx, &self.session_templates, &SESSION_TEMPLATES, value.id, value).await
    }
    async fn delete_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<bool> {
        self.delete_in(ctx, &self.session_templates, &SESSION_TEMPLATES, &id).await
    }
    async fn list_session_templates(&self, ctx: &SecurityContext) -> StoreResult<Vec<SessionTemplate>> {
        self.list_in(ctx, &self.session_templates, &SESSION_TEMPLATES).await
    }

    // Agents
    async fn create_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
        self.create_in(ctx, &self.agents, &AGENTS, value.id, value).await
    }
    async fn get_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent> {
        self.get_in(ctx, &self.agents, &AGENTS, &id).await
    }
    async fn update_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
        self.update_in(ctx, &self.agents, &AGENTS, value.id, value).await
    }
    async fn delete_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<bool> {
        self.delete_in(ctx, &self.agents, &AGENTS, &id).await
    }
    async fn list_agents(&self, ctx: &SecurityContext) -> StoreResult<Vec<Agent>> {
        self.list_in(ctx, &self.agents, &AGENTS).await
    }

    // Sessions - their state is the `StateStore`'s, which may be another
    // store: whoever deletes a session deletes it there.
    async fn create_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()> {
        self.create_in(ctx, &self.sessions, &SESSIONS, value.id, value).await
    }
    async fn get_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session> {
        self.get_in(ctx, &self.sessions, &SESSIONS, &id).await
    }
    async fn update_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()> {
        self.update_in(ctx, &self.sessions, &SESSIONS, value.id, value).await
    }
    async fn delete_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<bool> {
        self.delete_in(ctx, &self.sessions, &SESSIONS, &id).await
    }
    async fn list_sessions(&self, ctx: &SecurityContext) -> StoreResult<Vec<Session>> {
        self.list_in(ctx, &self.sessions, &SESSIONS).await
    }

    // Channels (issue 70)
    async fn create_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()> {
        self.create_in(ctx, &self.channels, &CHANNELS, value.id, value).await
    }
    async fn get_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<Channel> {
        self.get_in(ctx, &self.channels, &CHANNELS, &id).await
    }
    async fn update_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()> {
        self.update_in(ctx, &self.channels, &CHANNELS, value.id, value).await
    }
    async fn delete_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<bool> {
        self.delete_in(ctx, &self.channels, &CHANNELS, &id).await
    }
    async fn list_channels(&self, ctx: &SecurityContext) -> StoreResult<Vec<Channel>> {
        self.list_in(ctx, &self.channels, &CHANNELS).await
    }

    // Users - found by name as well, since a username is an identity
    // (issue 93).
    async fn create_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()> {
        self.create_in(ctx, &self.users, &USERS, value.id, value).await
    }
    async fn get_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<User> {
        self.get_in(ctx, &self.users, &USERS, &id).await
    }
    async fn update_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()> {
        self.update_in(ctx, &self.users, &USERS, value.id, value).await
    }
    async fn delete_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool> {
        self.delete_in(ctx, &self.users, &USERS, &id).await
    }
    async fn list_users(&self, ctx: &SecurityContext) -> StoreResult<Vec<User>> {
        self.list_in(ctx, &self.users, &USERS).await
    }
    async fn find_user_by_name(&self, ctx: &SecurityContext, name: &str) -> StoreResult<User> {
        let users = self.list_in(ctx, &self.users, &USERS).await?;
        require_entity(users.into_iter().find(|u| u.username == name), "user", name.to_string())
    }

    // User profiles (issue 176)
    async fn create_user_profile(&self, ctx: &SecurityContext, value: &UserProfile) -> StoreResult<()> {
        self.create_in(ctx, &self.user_profiles, &USER_PROFILES, value.user_id(), value).await
    }
    async fn get_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<UserProfile> {
        self.get_in(ctx, &self.user_profiles, &USER_PROFILES, &id).await
    }
    async fn update_user_profile(&self, ctx: &SecurityContext, value: &UserProfile) -> StoreResult<()> {
        self.update_in(ctx, &self.user_profiles, &USER_PROFILES, value.user_id(), value).await
    }
    async fn delete_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool> {
        self.delete_in(ctx, &self.user_profiles, &USER_PROFILES, &id).await
    }
    async fn list_user_profiles(&self, ctx: &SecurityContext) -> StoreResult<Vec<UserProfile>> {
        self.list_in(ctx, &self.user_profiles, &USER_PROFILES).await
    }

    // Components
    async fn create_component(&self, ctx: &SecurityContext, value: &ComponentDescriptor) -> StoreResult<()> {
        self.create_in(ctx, &self.components, &COMPONENTS, value.id.clone(), value).await
    }
    async fn get_component(&self, ctx: &SecurityContext, id: &str) -> StoreResult<ComponentDescriptor> {
        self.get_in(ctx, &self.components, &COMPONENTS, &id.to_string()).await
    }
    async fn update_component(&self, ctx: &SecurityContext, value: &ComponentDescriptor) -> StoreResult<()> {
        self.update_in(ctx, &self.components, &COMPONENTS, value.id.clone(), value).await
    }
    async fn delete_component(&self, ctx: &SecurityContext, id: &str) -> StoreResult<bool> {
        self.delete_in(ctx, &self.components, &COMPONENTS, &id.to_string()).await
    }
    async fn list_components(&self, ctx: &SecurityContext) -> StoreResult<Vec<ComponentDescriptor>> {
        self.list_in(ctx, &self.components, &COMPONENTS).await
    }
}

#[async_trait]
impl StateStore for MemoryStore {
    async fn get_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId) -> StoreResult<AgentChannelState> {
        Ok(self.read_lock(&self.agent_channel_states, |v| v.get(&(agent_id, channel_id)).cloned()).unwrap_or_default())
    }

    async fn put_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId, state: &mut AgentChannelState) -> StoreResult<()> {
        // Marked first: the copy kept is the state as stored, with nothing pending.
        state.mark_stored();
        self.write_lock(&self.agent_channel_states, |v| v.insert((agent_id, channel_id), state.clone()));
        Ok(())
    }

    async fn get_agent_state(&self, agent_id: AgentId) -> StoreResult<AgentState> {
        Ok(self.read_lock(&self.agent_states, |v| v.get(&agent_id).cloned()).unwrap_or_default())
    }

    async fn put_agent_state(&self, agent_id: AgentId, state: &mut AgentState) -> StoreResult<()> {
        // Marked first: the copy kept is the state as stored, with nothing pending.
        state.mark_stored();
        self.write_lock(&self.agent_states, |v| v.insert(agent_id, state.clone()));
        Ok(())
    }

    async fn get_channel_state(&self, channel_id: ChannelId) -> StoreResult<ChannelState> {
        require_entity(self.read_lock(&self.channel_states, |v| v.get(&channel_id).cloned()), "channel_state", channel_id.to_string())
    }

    async fn put_channel_state(&self, channel_id: ChannelId, state: &mut ChannelState) -> StoreResult<()> {
        // Marked first: the copy kept is the state as stored, with nothing pending.
        state.mark_stored();
        self.write_lock(&self.channel_states, |v| v.insert(channel_id, state.clone()));
        Ok(())
    }

    async fn delete_channel_state(&self, channel_id: ChannelId) -> StoreResult<()> {
        self.write_lock(&self.channel_states, |v| v.remove(&channel_id));
        self.write_lock(&self.agent_channel_states, |v| v.retain(|(_, cid), _| *cid != channel_id));
        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store_impl::suite;

    #[tokio::test]
    async fn behaves_as_an_entity_store() {
        suite::entity_store_behaves(&MemoryStore::new()).await;
    }

    #[tokio::test]
    async fn behaves_as_a_state_store() {
        suite::state_store_behaves(&MemoryStore::new()).await;
    }
}
