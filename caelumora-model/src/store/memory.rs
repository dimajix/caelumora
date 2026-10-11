use crate::agent::{Agent, AgentChannelState, AgentId, AgentState, Personality, PersonalityId};
use crate::channel::{Channel, ChannelId, ChannelState};
use crate::component::StoredComponents;
use crate::session::{Session, SessionId};
use crate::store::{ComponentStore, EntityStore, Journaled, StateStore, StoreError, StoreResult};
use crate::template::{SessionTemplate, SessionTemplateId};
use async_trait::async_trait;
use tracing::info;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use crate::removal::{Deletable, Removal, Stored};
use crate::scope::Scoped;
use crate::value::name::ScopedName;
use crate::security::{DummySecurityManager, SecurityManager};
use crate::store::access::{name_free, Access, Actions, AGENTS, ARTIFACTS, CHANNELS, COMPONENTS, GROUPS, PERSONALITIES, SESSIONS, SESSION_TEMPLATES, SKILLS, USERS, USER_PROFILES, WORKSPACES};
use crate::workspace::{Workspace, WorkspaceId};
use crate::skill::{Skill, SkillId};
use crate::group::{Group, GroupField, GroupId};
use crate::artifact::{Artifact, ArtifactId};
use std::fmt::Display;
use std::hash::Hash;
use crate::store::query::{self, Field, Filter, NoField, Page, Query};
use std::future::ready;
use serde::Serialize;
use crate::artifact::ArtifactField;
use crate::channel::ChannelField;
use crate::scope::ScopeField;
use crate::session::SessionField;

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
    workspaces: RwLock<HashMap<WorkspaceId, Workspace>>,
    skills: RwLock<HashMap<SkillId, Skill>>,
    artifacts: RwLock<HashMap<ArtifactId, Artifact>>,
    groups: RwLock<HashMap<GroupId, Group>>,
    agent_states: RwLock<HashMap<AgentId, AgentState>>,
    agent_channel_states: RwLock<HashMap<(AgentId, ChannelId), AgentChannelState>>,
    channel_states: RwLock<HashMap<ChannelId, ChannelState>>,
    components: RwLock<HashMap<String, StoredComponents>>,
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
            workspaces: Default::default(),
            skills: Default::default(),
            artifacts: Default::default(),
            groups: Default::default(),
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

/// What a write checks of the other entities of its kind, inside the lock
/// that writes: the entity as it is to be written, and the others.
type Others<'a, T> = &'a mut dyn Iterator<Item = &'a T>;

/// A write of a kind whose names are not unique checks nothing of the
/// others.
fn unnamed<T>(_: &T, _: Others<'_, T>) -> StoreResult<()> {
    Ok(())
}

/// The operations of every kind (issues 93, 164), on the map that holds
/// that kind and checked with its `Actions`. Those that write come as
/// three: the plain one, the one for a kind whose names are unique
/// (`…_named_in`, issue 250), and what both are made of (`…_with`).
impl MemoryStore {
    async fn create_in<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: K, value: &T) -> StoreResult<()>
    where K: Eq + Hash + Display + Send + Sync, T: Stored + Clone + Send + Sync {
        self.create_with(ctx, map, kind, key, value, unnamed).await
    }

    async fn create_named_in<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: K, value: &T) -> StoreResult<()>
    where K: Eq + Hash + Display + Send + Sync, T: Stored + Scoped + Clone + Send + Sync {
        self.create_with(ctx, map, kind, key, value, |value, others| name_free(value, others, kind)).await
    }

    async fn create_with<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: K, value: &T, others: impl FnOnce(&T, Others<'_, T>) -> StoreResult<()>) -> StoreResult<()>
    where K: Eq + Hash + Display + Send + Sync, T: Stored + Clone + Send + Sync {
        self.access(ctx).create(value, kind).await?;
        self.write_lock(map, |v| {
            if v.contains_key(&key) {
                return Err(StoreError::Conflict(format!("{} {} already exists", kind.entity, key)));
            }
            others(value, &mut v.values())?;
            v.insert(key, value.clone());
            Ok(())
        })
    }

    async fn get_in<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: &K) -> StoreResult<T>
    where K: Eq + Hash + Display + Send + Sync, T: Stored + Clone + Send + Sync {
        let found = self.read_lock(map, |v| v.get(key).cloned());
        self.access(ctx).read(found, kind, key.to_string()).await
    }

    async fn update_in<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: K, value: &T) -> StoreResult<()>
    where K: Eq + Hash + Display + Send + Sync, T: Stored + Clone + Send + Sync {
        self.update_with(ctx, map, kind, key, value, unnamed).await
    }

    async fn update_named_in<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: K, value: &T) -> StoreResult<()>
    where K: Eq + Hash + Display + Send + Sync, T: Stored + Scoped + Clone + Send + Sync {
        self.update_with(ctx, map, kind, key, value, |value, others| name_free(value, others, kind)).await
    }

    async fn update_with<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: K, value: &T, others: impl FnOnce(&T, Others<'_, T>) -> StoreResult<()>) -> StoreResult<()>
    where K: Eq + Hash + Display + Send + Sync, T: Stored + Clone + Send + Sync {
        let stored = self.read_lock(map, |v| v.get(&key).cloned());
        self.access(ctx).change(stored.as_ref(), value, kind, key.to_string()).await?;
        self.write_lock(map, |v| {
            others(value, &mut v.iter().filter(|(k, _)| **k != key).map(|(_, other)| other))?;
            v.insert(key, value.clone());
            Ok(())
        })
    }

    /// Marks the entity deleted, by the caller and now (issue 164).
    async fn delete_in<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: K) -> StoreResult<bool>
    where K: Eq + Hash + Display + Send + Sync, T: Deletable + Clone + Send + Sync {
        let stored = self.read_lock(map, |v| v.get(&key).cloned());
        if !self.access(ctx).delete(stored.as_ref(), kind).await? {
            return Ok(false);
        }
        let Some(mut marked) = stored else { return Ok(false) };
        marked.set_deleted(Some(Removal::now(ctx.caller_id)));
        self.write_lock(map, |v| v.insert(key, marked));
        Ok(true)
    }

    /// Removes the entity for good: deleting a kind that is not marked.
    async fn remove_in<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: &K) -> StoreResult<bool>
    where K: Eq + Hash + Display + Send + Sync, T: Stored + Clone + Send + Sync {
        let stored = self.read_lock(map, |v| v.get(key).cloned());
        if !self.access(ctx).delete(stored.as_ref(), kind).await? {
            return Ok(false);
        }
        Ok(self.write_lock(map, |v| v.remove(key).is_some()))
    }

    async fn restore_in<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: K) -> StoreResult<()>
    where K: Eq + Hash + Display + Send + Sync, T: Deletable + Clone + Send + Sync {
        self.restore_with(ctx, map, kind, key, unnamed).await
    }

    async fn restore_named_in<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: K) -> StoreResult<()>
    where K: Eq + Hash + Display + Send + Sync, T: Deletable + Scoped + Clone + Send + Sync {
        self.restore_with(ctx, map, kind, key, |value, others| name_free(value, others, kind)).await
    }

    async fn restore_with<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: K, others: impl FnOnce(&T, Others<'_, T>) -> StoreResult<()>) -> StoreResult<()>
    where K: Eq + Hash + Display + Send + Sync, T: Deletable + Clone + Send + Sync {
        let stored = self.read_lock(map, |v| v.get(&key).cloned());
        self.access(ctx).restore(stored.as_ref(), kind, key.to_string()).await?;
        let Some(mut restored) = stored else { return Ok(()) };
        restored.set_deleted(None);
        self.write_lock(map, |v| {
            others(&restored, &mut v.iter().filter(|(k, _)| **k != key).map(|(_, other)| other))?;
            v.insert(key, restored);
            Ok(())
        })
    }

    async fn purge_in<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: &K) -> StoreResult<bool>
    where K: Eq + Hash + Display + Send + Sync, T: Deletable + Clone + Send + Sync {
        let stored = self.read_lock(map, |v| v.get(key).cloned());
        if !self.access(ctx).purge(stored.as_ref(), kind).await? {
            return Ok(false);
        }
        // Only if it is still marked: restored while the purge was asked
        // about, it stays.
        Ok(self.write_lock(map, |v| {
            let marked = v.get(key).is_some_and(|value| value.deleted().is_some());
            marked && v.remove(key).is_some()
        }))
    }

    async fn get_deleted_in<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, key: &K) -> StoreResult<T>
    where K: Eq + Hash + Display + Send + Sync, T: Deletable + Clone + Send + Sync {
        let found = self.read_lock(map, |v| v.get(key).cloned());
        self.access(ctx).read_deleted(found, kind, key.to_string()).await
    }

    /// The keys of `map`, each as it orders a list and as it finds the
    /// entity, in no order.
    fn keys<K, T>(&self, map: &RwLock<HashMap<K, T>>) -> Vec<(String, K)>
    where K: Display + Clone {
        self.read_lock(map, |v| v.keys().map(|key| (key.to_string(), key.clone())).collect())
    }

    /// The entity of `key` in `map`, if it is there and meets `filter`.
    fn meeting_in<K, T, F>(&self, map: &RwLock<HashMap<K, T>>, key: &K, filter: Option<&Filter<F>>) -> StoreResult<Option<T>>
    where K: Eq + Hash, T: Serialize + Clone, F: Field {
        match self.read_lock(map, |v| v.get(key).cloned()) {
            Some(value) => query::meeting(value, filter),
            None => Ok(None),
        }
    }

    async fn list_deleted_in<K, T, F>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, query: &Query<F>) -> StoreResult<Page<T>>
    where K: Eq + Hash + Display + Clone + Send + Sync, T: Deletable + Serialize + Clone + Send + Sync, F: Field {
        let access = self.access(ctx);
        access.allow(kind.list).await?;
        let filter = query.filter.as_ref();
        access.page(self.keys(map), query, kind, true, |key: K| ready(self.meeting_in(map, &key, filter))).await
    }

    async fn list_in<K, T, F>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, query: &Query<F>) -> StoreResult<Page<T>>
    where K: Eq + Hash + Display + Clone + Send + Sync, T: Stored + Serialize + Clone + Send + Sync, F: Field {
        let access = self.access(ctx);
        access.allow(kind.list).await?;
        let filter = query.filter.as_ref();
        access.page(self.keys(map), query, kind, false, |key: K| ready(self.meeting_in(map, &key, filter))).await
    }

    /// The one of `name`, in exactly its scope, among those listed.
    async fn find_in<K, T>(&self, ctx: &SecurityContext, map: &RwLock<HashMap<K, T>>, kind: &Actions, name: &ScopedName) -> StoreResult<T>
    where K: Eq + Hash + Display + Clone + Send + Sync, T: Stored + Scoped + Serialize + Clone + Send + Sync {
        let all = self.list_in(ctx, map, kind, &Query::<NoField>::all()).await?.items;
        require_entity(all.into_iter().find(|d| d.scoped_name() == *name), kind.entity.name(), name.to_string())
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
        self.create_named_in(ctx, &self.personalities, &PERSONALITIES, value.id, value).await
    }
    async fn get_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<Personality> {
        self.get_in(ctx, &self.personalities, &PERSONALITIES, &id).await
    }
    async fn update_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()> {
        self.update_named_in(ctx, &self.personalities, &PERSONALITIES, value.id, value).await
    }
    async fn delete_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<bool> {
        self.delete_in(ctx, &self.personalities, &PERSONALITIES, id).await
    }
    async fn list_personalities(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Personality>> {
        self.list_in(ctx, &self.personalities, &PERSONALITIES, query).await
    }
    async fn restore_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<()> {
        self.restore_named_in(ctx, &self.personalities, &PERSONALITIES, id).await
    }
    async fn purge_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<bool> {
        self.purge_in(ctx, &self.personalities, &PERSONALITIES, &id).await
    }
    async fn get_deleted_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<Personality> {
        self.get_deleted_in(ctx, &self.personalities, &PERSONALITIES, &id).await
    }
    async fn list_deleted_personalities(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Personality>> {
        self.list_deleted_in(ctx, &self.personalities, &PERSONALITIES, query).await
    }
    async fn find_personality_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Personality> {
        self.find_in(ctx, &self.personalities, &PERSONALITIES, name).await
    }

    // Session templates
    async fn create_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()> {
        self.create_named_in(ctx, &self.session_templates, &SESSION_TEMPLATES, value.id, value).await
    }
    async fn get_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<SessionTemplate> {
        self.get_in(ctx, &self.session_templates, &SESSION_TEMPLATES, &id).await
    }
    async fn update_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()> {
        self.update_named_in(ctx, &self.session_templates, &SESSION_TEMPLATES, value.id, value).await
    }
    async fn delete_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<bool> {
        self.delete_in(ctx, &self.session_templates, &SESSION_TEMPLATES, id).await
    }
    async fn list_session_templates(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<SessionTemplate>> {
        self.list_in(ctx, &self.session_templates, &SESSION_TEMPLATES, query).await
    }
    async fn restore_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<()> {
        self.restore_named_in(ctx, &self.session_templates, &SESSION_TEMPLATES, id).await
    }
    async fn purge_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<bool> {
        self.purge_in(ctx, &self.session_templates, &SESSION_TEMPLATES, &id).await
    }
    async fn get_deleted_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<SessionTemplate> {
        self.get_deleted_in(ctx, &self.session_templates, &SESSION_TEMPLATES, &id).await
    }
    async fn list_deleted_session_templates(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<SessionTemplate>> {
        self.list_deleted_in(ctx, &self.session_templates, &SESSION_TEMPLATES, query).await
    }
    async fn find_session_template_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<SessionTemplate> {
        self.find_in(ctx, &self.session_templates, &SESSION_TEMPLATES, name).await
    }

    // Agents
    async fn create_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
        self.create_named_in(ctx, &self.agents, &AGENTS, value.id, value).await
    }
    async fn get_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent> {
        self.get_in(ctx, &self.agents, &AGENTS, &id).await
    }
    async fn update_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
        self.update_named_in(ctx, &self.agents, &AGENTS, value.id, value).await
    }
    async fn delete_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<bool> {
        self.delete_in(ctx, &self.agents, &AGENTS, id).await
    }
    async fn list_agents(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Agent>> {
        self.list_in(ctx, &self.agents, &AGENTS, query).await
    }
    async fn restore_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<()> {
        self.restore_named_in(ctx, &self.agents, &AGENTS, id).await
    }
    async fn purge_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<bool> {
        self.purge_in(ctx, &self.agents, &AGENTS, &id).await
    }
    async fn get_deleted_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent> {
        self.get_deleted_in(ctx, &self.agents, &AGENTS, &id).await
    }
    async fn list_deleted_agents(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Agent>> {
        self.list_deleted_in(ctx, &self.agents, &AGENTS, query).await
    }
    async fn find_agent_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Agent> {
        self.find_in(ctx, &self.agents, &AGENTS, name).await
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
        self.delete_in(ctx, &self.sessions, &SESSIONS, id).await
    }
    async fn list_sessions(&self, ctx: &SecurityContext, query: &Query<SessionField>) -> StoreResult<Page<Session>> {
        self.list_in(ctx, &self.sessions, &SESSIONS, query).await
    }
    async fn restore_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<()> {
        self.restore_in(ctx, &self.sessions, &SESSIONS, id).await
    }
    async fn purge_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<bool> {
        self.purge_in(ctx, &self.sessions, &SESSIONS, &id).await
    }
    async fn get_deleted_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session> {
        self.get_deleted_in(ctx, &self.sessions, &SESSIONS, &id).await
    }
    async fn list_deleted_sessions(&self, ctx: &SecurityContext, query: &Query<SessionField>) -> StoreResult<Page<Session>> {
        self.list_deleted_in(ctx, &self.sessions, &SESSIONS, query).await
    }

    // Channels (issue 70)
    async fn create_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()> {
        self.create_named_in(ctx, &self.channels, &CHANNELS, value.id, value).await
    }
    async fn get_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<Channel> {
        self.get_in(ctx, &self.channels, &CHANNELS, &id).await
    }
    async fn update_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()> {
        self.update_named_in(ctx, &self.channels, &CHANNELS, value.id, value).await
    }
    async fn delete_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<bool> {
        self.delete_in(ctx, &self.channels, &CHANNELS, id).await
    }
    async fn list_channels(&self, ctx: &SecurityContext, query: &Query<ChannelField>) -> StoreResult<Page<Channel>> {
        self.list_in(ctx, &self.channels, &CHANNELS, query).await
    }
    async fn restore_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<()> {
        self.restore_named_in(ctx, &self.channels, &CHANNELS, id).await
    }
    async fn purge_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<bool> {
        self.purge_in(ctx, &self.channels, &CHANNELS, &id).await
    }
    async fn get_deleted_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<Channel> {
        self.get_deleted_in(ctx, &self.channels, &CHANNELS, &id).await
    }
    async fn list_deleted_channels(&self, ctx: &SecurityContext, query: &Query<ChannelField>) -> StoreResult<Page<Channel>> {
        self.list_deleted_in(ctx, &self.channels, &CHANNELS, query).await
    }

    // Users - found by name as well, since a username is an identity
    // (issue 93).
    async fn create_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()> {
        self.create_named_in(ctx, &self.users, &USERS, value.id, value).await
    }
    async fn get_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<User> {
        self.get_in(ctx, &self.users, &USERS, &id).await
    }
    async fn update_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()> {
        self.update_named_in(ctx, &self.users, &USERS, value.id, value).await
    }
    async fn delete_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool> {
        self.delete_in(ctx, &self.users, &USERS, id).await
    }
    async fn list_users(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<User>> {
        self.list_in(ctx, &self.users, &USERS, query).await
    }
    async fn restore_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<()> {
        self.restore_named_in(ctx, &self.users, &USERS, id).await
    }
    async fn purge_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool> {
        self.purge_in(ctx, &self.users, &USERS, &id).await
    }
    async fn get_deleted_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<User> {
        self.get_deleted_in(ctx, &self.users, &USERS, &id).await
    }
    async fn list_deleted_users(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<User>> {
        self.list_deleted_in(ctx, &self.users, &USERS, query).await
    }
    async fn find_user_by_name(&self, ctx: &SecurityContext, name: &str) -> StoreResult<User> {
        let users = self.list_in(ctx, &self.users, &USERS, &Query::<NoField>::all()).await?.items;
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
        self.delete_in(ctx, &self.user_profiles, &USER_PROFILES, id).await
    }
    async fn list_user_profiles(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<UserProfile>> {
        self.list_in(ctx, &self.user_profiles, &USER_PROFILES, query).await
    }
    async fn restore_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<()> {
        self.restore_in(ctx, &self.user_profiles, &USER_PROFILES, id).await
    }
    async fn purge_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<bool> {
        self.purge_in(ctx, &self.user_profiles, &USER_PROFILES, &id).await
    }
    async fn get_deleted_user_profile(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<UserProfile> {
        self.get_deleted_in(ctx, &self.user_profiles, &USER_PROFILES, &id).await
    }
    async fn list_deleted_user_profiles(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<UserProfile>> {
        self.list_deleted_in(ctx, &self.user_profiles, &USER_PROFILES, query).await
    }

    // Workspaces (issue 253)
    async fn create_workspace(&self, ctx: &SecurityContext, value: &Workspace) -> StoreResult<()> {
        self.create_named_in(ctx, &self.workspaces, &WORKSPACES, value.id, value).await
    }
    async fn get_workspace(&self, ctx: &SecurityContext, id: WorkspaceId) -> StoreResult<Workspace> {
        self.get_in(ctx, &self.workspaces, &WORKSPACES, &id).await
    }
    async fn update_workspace(&self, ctx: &SecurityContext, value: &Workspace) -> StoreResult<()> {
        self.update_named_in(ctx, &self.workspaces, &WORKSPACES, value.id, value).await
    }
    async fn delete_workspace(&self, ctx: &SecurityContext, id: WorkspaceId) -> StoreResult<bool> {
        self.delete_in(ctx, &self.workspaces, &WORKSPACES, id).await
    }
    async fn list_workspaces(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Workspace>> {
        self.list_in(ctx, &self.workspaces, &WORKSPACES, query).await
    }
    async fn restore_workspace(&self, ctx: &SecurityContext, id: WorkspaceId) -> StoreResult<()> {
        self.restore_named_in(ctx, &self.workspaces, &WORKSPACES, id).await
    }
    async fn purge_workspace(&self, ctx: &SecurityContext, id: WorkspaceId) -> StoreResult<bool> {
        self.purge_in(ctx, &self.workspaces, &WORKSPACES, &id).await
    }
    async fn get_deleted_workspace(&self, ctx: &SecurityContext, id: WorkspaceId) -> StoreResult<Workspace> {
        self.get_deleted_in(ctx, &self.workspaces, &WORKSPACES, &id).await
    }
    async fn list_deleted_workspaces(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Workspace>> {
        self.list_deleted_in(ctx, &self.workspaces, &WORKSPACES, query).await
    }
    async fn find_workspace_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Workspace> {
        self.find_in(ctx, &self.workspaces, &WORKSPACES, name).await
    }

    // Skills (issue 22)
    async fn create_skill(&self, ctx: &SecurityContext, value: &Skill) -> StoreResult<()> {
        self.create_named_in(ctx, &self.skills, &SKILLS, value.id, value).await
    }
    async fn get_skill(&self, ctx: &SecurityContext, id: SkillId) -> StoreResult<Skill> {
        self.get_in(ctx, &self.skills, &SKILLS, &id).await
    }
    async fn update_skill(&self, ctx: &SecurityContext, value: &Skill) -> StoreResult<()> {
        self.update_named_in(ctx, &self.skills, &SKILLS, value.id, value).await
    }
    async fn delete_skill(&self, ctx: &SecurityContext, id: SkillId) -> StoreResult<bool> {
        self.delete_in(ctx, &self.skills, &SKILLS, id).await
    }
    async fn list_skills(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Skill>> {
        self.list_in(ctx, &self.skills, &SKILLS, query).await
    }
    async fn restore_skill(&self, ctx: &SecurityContext, id: SkillId) -> StoreResult<()> {
        self.restore_named_in(ctx, &self.skills, &SKILLS, id).await
    }
    async fn purge_skill(&self, ctx: &SecurityContext, id: SkillId) -> StoreResult<bool> {
        self.purge_in(ctx, &self.skills, &SKILLS, &id).await
    }
    async fn get_deleted_skill(&self, ctx: &SecurityContext, id: SkillId) -> StoreResult<Skill> {
        self.get_deleted_in(ctx, &self.skills, &SKILLS, &id).await
    }
    async fn list_deleted_skills(&self, ctx: &SecurityContext, query: &Query<ScopeField>) -> StoreResult<Page<Skill>> {
        self.list_deleted_in(ctx, &self.skills, &SKILLS, query).await
    }
    async fn find_skill_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Skill> {
        self.find_in(ctx, &self.skills, &SKILLS, name).await
    }

    // Artifacts (issue 271)
    async fn create_artifact(&self, ctx: &SecurityContext, value: &Artifact) -> StoreResult<()> {
        self.create_named_in(ctx, &self.artifacts, &ARTIFACTS, value.id, value).await
    }
    async fn get_artifact(&self, ctx: &SecurityContext, id: ArtifactId) -> StoreResult<Artifact> {
        self.get_in(ctx, &self.artifacts, &ARTIFACTS, &id).await
    }
    async fn update_artifact(&self, ctx: &SecurityContext, value: &Artifact) -> StoreResult<()> {
        self.update_named_in(ctx, &self.artifacts, &ARTIFACTS, value.id, value).await
    }
    async fn delete_artifact(&self, ctx: &SecurityContext, id: ArtifactId) -> StoreResult<bool> {
        self.delete_in(ctx, &self.artifacts, &ARTIFACTS, id).await
    }
    async fn list_artifacts(&self, ctx: &SecurityContext, query: &Query<ArtifactField>) -> StoreResult<Page<Artifact>> {
        self.list_in(ctx, &self.artifacts, &ARTIFACTS, query).await
    }
    async fn restore_artifact(&self, ctx: &SecurityContext, id: ArtifactId) -> StoreResult<()> {
        self.restore_named_in(ctx, &self.artifacts, &ARTIFACTS, id).await
    }
    async fn purge_artifact(&self, ctx: &SecurityContext, id: ArtifactId) -> StoreResult<bool> {
        self.purge_in(ctx, &self.artifacts, &ARTIFACTS, &id).await
    }
    async fn get_deleted_artifact(&self, ctx: &SecurityContext, id: ArtifactId) -> StoreResult<Artifact> {
        self.get_deleted_in(ctx, &self.artifacts, &ARTIFACTS, &id).await
    }
    async fn list_deleted_artifacts(&self, ctx: &SecurityContext, query: &Query<ArtifactField>) -> StoreResult<Page<Artifact>> {
        self.list_deleted_in(ctx, &self.artifacts, &ARTIFACTS, query).await
    }
    async fn find_artifact_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Artifact> {
        self.find_in(ctx, &self.artifacts, &ARTIFACTS, name).await
    }

    // Groups (issue 286)
    async fn create_group(&self, ctx: &SecurityContext, value: &Group) -> StoreResult<()> {
        self.create_named_in(ctx, &self.groups, &GROUPS, value.id, value).await
    }
    async fn get_group(&self, ctx: &SecurityContext, id: GroupId) -> StoreResult<Group> {
        self.get_in(ctx, &self.groups, &GROUPS, &id).await
    }
    async fn update_group(&self, ctx: &SecurityContext, value: &Group) -> StoreResult<()> {
        self.update_named_in(ctx, &self.groups, &GROUPS, value.id, value).await
    }
    async fn delete_group(&self, ctx: &SecurityContext, id: GroupId) -> StoreResult<bool> {
        self.delete_in(ctx, &self.groups, &GROUPS, id).await
    }
    async fn list_groups(&self, ctx: &SecurityContext, query: &Query<GroupField>) -> StoreResult<Page<Group>> {
        self.list_in(ctx, &self.groups, &GROUPS, query).await
    }
    async fn restore_group(&self, ctx: &SecurityContext, id: GroupId) -> StoreResult<()> {
        self.restore_named_in(ctx, &self.groups, &GROUPS, id).await
    }
    async fn purge_group(&self, ctx: &SecurityContext, id: GroupId) -> StoreResult<bool> {
        self.purge_in(ctx, &self.groups, &GROUPS, &id).await
    }
    async fn get_deleted_group(&self, ctx: &SecurityContext, id: GroupId) -> StoreResult<Group> {
        self.get_deleted_in(ctx, &self.groups, &GROUPS, &id).await
    }
    async fn list_deleted_groups(&self, ctx: &SecurityContext, query: &Query<GroupField>) -> StoreResult<Page<Group>> {
        self.list_deleted_in(ctx, &self.groups, &GROUPS, query).await
    }
    async fn find_group_by_name(&self, ctx: &SecurityContext, name: &ScopedName) -> StoreResult<Group> {
        self.find_in(ctx, &self.groups, &GROUPS, name).await
    }
}

/// Preconfigured components, kept like the entities (issue 164).
#[async_trait]
impl ComponentStore for MemoryStore {
    async fn create_components(&self, ctx: &SecurityContext, value: &StoredComponents) -> StoreResult<()> {
        self.create_in(ctx, &self.components, &COMPONENTS, value.id().to_string(), value).await
    }
    async fn get_components(&self, ctx: &SecurityContext, id: &str) -> StoreResult<StoredComponents> {
        self.get_in(ctx, &self.components, &COMPONENTS, &id.to_string()).await
    }
    async fn update_components(&self, ctx: &SecurityContext, value: &StoredComponents) -> StoreResult<()> {
        self.update_in(ctx, &self.components, &COMPONENTS, value.id().to_string(), value).await
    }
    async fn delete_components(&self, ctx: &SecurityContext, id: &str) -> StoreResult<bool> {
        self.remove_in(ctx, &self.components, &COMPONENTS, &id.to_string()).await
    }
    async fn list_components(&self, ctx: &SecurityContext, query: &Query<NoField>) -> StoreResult<Page<StoredComponents>> {
        self.list_in(ctx, &self.components, &COMPONENTS, query).await
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

    async fn delete_agent_state(&self, agent_id: AgentId) -> StoreResult<()> {
        self.write_lock(&self.agent_states, |v| v.remove(&agent_id));
        self.write_lock(&self.agent_channel_states, |v| v.retain(|(aid, _), _| *aid != agent_id));
        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::suite;

    #[tokio::test]
    async fn behaves_as_an_entity_store() {
        suite::entity_store_behaves(&MemoryStore::new()).await;
    }

    #[tokio::test]
    async fn answers_queries() {
        suite::queries_behave(&MemoryStore::new()).await;
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
        let store = MemoryStore::with_security_manager(Arc::new(OwnershipSecurityManager::new()));
        let (alice, bob) = (UserId::new(), UserId::new());
        let mut hers = Vec::new();
        for (index, owner) in [alice, bob, bob, alice, bob, alice].into_iter().enumerate() {
            let name = ScopedName::new(format!("a{index}.csv"), Scope::UserScope { user_id: owner }).unwrap();
            let artifact = Artifact::new(ArtifactId::new(), name, "files", Classification::CHANNEL_DEFAULT, owner, version(b"a,b", SessionId::new()));
            store.create_artifact(&SecurityContext::system(), &artifact).await.unwrap();
            if owner == alice {
                hers.push(artifact.id);
            }
        }
        hers.sort_by_key(|id| id.to_string());
        let ctx = SecurityContext::new(ActorId::User(alice), Default::default(), Default::default());
        let mut read = Vec::new();
        let mut query = Query::all().limited(1);
        loop {
            let page = store.list_artifacts(&ctx, &query).await.unwrap();
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
        suite::deleting_marks(&MemoryStore::new()).await;
    }

    #[tokio::test]
    async fn keeps_names_unique() {
        suite::names_are_unique(&MemoryStore::new()).await;
        let owners = MemoryStore::with_security_manager(Arc::new(crate::security::OwnershipSecurityManager::new()));
        suite::names_are_unique_beyond_what_is_read(&owners).await;
    }
}
