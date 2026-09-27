use crate::agent::{Agent, AgentChannelState, AgentId, AgentState, Personality, PersonalityId};
use crate::channel::{ChannelId, ChannelState};
use crate::component::ComponentDescriptor;
use crate::meta::Scope;
use crate::session::{Session, SessionId, SessionState};
use crate::store::{EntityStore, StateStore, StoreError, StoreResult};
use crate::template::{SessionTemplate, SessionTemplateId};
use async_trait::async_trait;
use tracing::info;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use crate::security::{ActionType, DummySecurityManager, SecurityManager};
use crate::store_impl::access::Access;

#[derive(Debug)]
pub struct MemoryStore {
    security_manager: Arc<dyn SecurityManager>,
    personalities: RwLock<HashMap<PersonalityId, Personality>>,
    session_templates: RwLock<HashMap<String, SessionTemplate>>,
    agents: RwLock<HashMap<AgentId, Agent>>,
    sessions: RwLock<HashMap<SessionId, Session>>,
    users: RwLock<HashMap<UserId, User>>,
    agent_states: RwLock<HashMap<AgentId, AgentState>>,
    agent_channel_states: RwLock<HashMap<(AgentId, ChannelId), AgentChannelState>>,
    session_states: RwLock<HashMap<SessionId, SessionState>>,
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
            users: Default::default(),
            agent_states: Default::default(),
            agent_channel_states: Default::default(),
            session_states: Default::default(),
            channel_states: Default::default(),
            components: Default::default(),
        }
    }

    fn access<'a>(&'a self, ctx: &'a SecurityContext) -> Access<'a> {
        Access::new(self.security_manager.as_ref(), ctx)
    }

    fn read_lock<T, F, R>(&self, lock: &RwLock<T>, f: F) -> StoreResult<R>
    where
        F: FnOnce(&T) -> R,
    {
        match lock.read() {
            Ok(guard) => Ok(f(&guard)),
            Err(poisoned) => Ok(f(poisoned.get_ref())),
        }
    }

    fn write_lock<T, F, R>(&self, lock: &RwLock<T>, f: F) -> StoreResult<R>
    where
        F: FnOnce(&mut T) -> R,
    {
        match lock.write() {
            Ok(mut guard) => Ok(f(&mut guard)),
            Err(mut poisoned) => Ok(f(poisoned.get_mut())),
        }
    }
}

fn require_entity<T>(value: Option<T>, entity: &'static str, id: String) -> StoreResult<T> {
    value.ok_or(StoreError::NotFound { entity, id })
}

use crate::security::SecurityContext;
use crate::user::{User, UserId};

#[async_trait]
impl EntityStore for MemoryStore {
    async fn get_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<Personality> {
        let found = self.read_lock(&self.personalities, |v| v.get(&id).cloned())?;
        self.access(ctx).read(found, |a| ActionType::GetPersonality(a), "agent_template", id.to_string()).await
    }

    async fn find_personality_by_name(&self, ctx: &SecurityContext, name: &str, scope: &Scope) -> StoreResult<Personality> {
        self.access(ctx).allow(ActionType::FindPersonality).await?;
        require_entity(
            self.list_personalities(ctx).await?
                .into_iter()
                .find(|t| t.metadata.name == name && scope.contains(&t.metadata.scope)),
            "agent_template",
            name.to_string(),
        )
    }

    async fn list_personalities(&self, ctx: &SecurityContext) -> StoreResult<Vec<Personality>> {
        let access = self.access(ctx);
        access.allow(ActionType::ListPersonalities).await?;
        let all = self.read_lock(&self.personalities, |v| v.values().cloned().collect())?;
        Ok(access.filter(all, |a| ActionType::GetPersonality(a)).await)
    }

    async fn put_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()> {
        let stored = self.read_lock(&self.personalities, |v| v.get(&value.id).cloned())?;
        self.access(ctx).put(
            stored.as_ref(), value,
            |a| ActionType::GetPersonality(a), |a| ActionType::CreatePersonality(a), |a| ActionType::UpdatePersonality(a),
            "agent_template", value.id.to_string(),
        ).await?;
        self.write_lock(&self.personalities, |v| {
            v.insert(value.id, value.clone());
        })
    }

    async fn delete_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<bool> {
        let stored = self.read_lock(&self.personalities, |v| v.get(&id).cloned())?;
        if !self.access(ctx).delete(stored.as_ref(), |a| ActionType::GetPersonality(a), |a| ActionType::DeletePersonality(a)).await? {
            return Ok(false);
        }
        self.write_lock(&self.personalities, |v| v.remove(&id).is_some())
    }

    async fn find_session_template_by_name(&self, ctx: &SecurityContext, name: &str, scope: &Scope) -> StoreResult<SessionTemplate> {
        self.access(ctx).allow(ActionType::FindSessionTemplate).await?;
        require_entity(
            self.list_session_templates(ctx).await?
                .into_iter()
                .find(|t| t.metadata.name == name && scope.contains(&t.metadata.scope)),
            "session_template",
            name.to_string(),
        )
    }

    async fn list_session_templates(&self, ctx: &SecurityContext) -> StoreResult<Vec<SessionTemplate>> {
        let access = self.access(ctx);
        access.allow(ActionType::ListSessionTemplates).await?;
        let all = self.read_lock(&self.session_templates, |v| v.values().cloned().collect())?;
        Ok(access.filter(all, |a| ActionType::GetSessionTemplate(a)).await)
    }

    async fn put_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()> {
        let stored = self.read_lock(&self.session_templates, |v| v.get(&value.metadata.name).cloned())?;
        self.access(ctx).put(
            stored.as_ref(), value,
            |a| ActionType::GetSessionTemplate(a), |a| ActionType::CreateSessionTemplate(a), |a| ActionType::UpdateSessionTemplate(a),
            "session_template", value.id.to_string(),
        ).await?;
        self.write_lock(&self.session_templates, |v| {
            v.insert(value.metadata.name.clone(), value.clone());
        })
    }

    async fn delete_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<bool> {
        let stored = self.read_lock(&self.session_templates, |v| v.values().find(|t| t.id == id).cloned())?;
        if !self.access(ctx).delete(stored.as_ref(), |a| ActionType::GetSessionTemplate(a), |a| ActionType::DeleteSessionTemplate(a)).await? {
            return Ok(false);
        }
        let Some(template) = stored else { return Ok(false) };
        self.write_lock(&self.session_templates, |v| v.remove(&template.metadata.name).is_some())
    }

    // Agents (durable definitions)
    async fn create_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
        self.access(ctx).create(value, |a| ActionType::CreateAgent(a)).await?;
        self.write_lock(&self.agents, |v| {
            if v.contains_key(&value.id) {
                return Err(StoreError::Conflict(format!("agent {} already exists", value.id)));
            }
            v.insert(value.id, value.clone());
            Ok(())
        })?
    }

    async fn get_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent> {
        let found = self.read_lock(&self.agents, |v| v.get(&id).cloned())?;
        self.access(ctx).read(found, |a| ActionType::GetAgent(a), "agent", id.to_string()).await
    }

    async fn find_agent_by_name(&self, ctx: &SecurityContext, name: &str, scope: &Scope) -> StoreResult<Agent> {
        let access = self.access(ctx);
        access.allow(ActionType::FindAgent).await?;
        let all = self.read_lock(&self.agents, |v| v.values().cloned().collect())?;
        require_entity(
            access.filter(all, |a| ActionType::GetAgent(a)).await
                .into_iter()
                .find(|t| t.metadata.name == name && scope.contains(&t.metadata.scope)),
            "agent",
            name.to_string(),
        )
    }

    async fn update_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
        let stored = self.read_lock(&self.agents, |v| v.get(&value.id).cloned())?;
        self.access(ctx).change(stored.as_ref(), |a| ActionType::GetAgent(a), |a| ActionType::UpdateAgent(a), "agent", value.id.to_string()).await?;
        self.write_lock(&self.agents, |v| {
            v.insert(value.id, value.clone());
        })
    }

    // Sessions (durable definitions)
    async fn create_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()> {
        self.access(ctx).create(value, |a| ActionType::CreateSession(a)).await?;
        self.write_lock(&self.sessions, |v| {
            if v.contains_key(&value.id) {
                return Err(StoreError::Conflict(format!("session {} already exists", value.id)));
            }
            v.insert(value.id, value.clone());
            Ok(())
        })?
    }

    async fn get_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session> {
        let found = self.read_lock(&self.sessions, |v| v.get(&id).cloned())?;
        self.access(ctx).read(found, |a| ActionType::GetSession(a), "session", id.to_string()).await
    }

    async fn update_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()> {
        let stored = self.read_lock(&self.sessions, |v| v.get(&value.id).cloned())?;
        self.access(ctx).change(stored.as_ref(), |a| ActionType::GetSession(a), |a| ActionType::UpdateSession(a), "session", value.id.to_string()).await?;
        self.write_lock(&self.sessions, |v| {
            v.insert(value.id, value.clone());
        })
    }

    async fn delete_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<bool> {
        let stored = self.read_lock(&self.sessions, |v| v.get(&id).cloned())?;
        if !self.access(ctx).delete(stored.as_ref(), |a| ActionType::GetSession(a), |a| ActionType::DeleteSession(a)).await? {
            return Ok(false);
        }
        // The state is the `StateStore`'s, which may be another store.
        let removed = self.write_lock(&self.sessions, |v| v.remove(&id))?;
        Ok(removed.is_some())
    }

    async fn list_sessions(&self, ctx: &SecurityContext) -> StoreResult<Vec<Session>> {
        let access = self.access(ctx);
        access.allow(ActionType::ListSessions).await?;
        let all = self.read_lock(&self.sessions, |v| v.values().cloned().collect())?;
        Ok(access.filter(all, |a| ActionType::GetSession(a)).await)
    }

    // Users
    async fn create_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()> {
        let _ = ctx; // Placeholder for security check
        self.write_lock(&self.users, |v| {
            if v.contains_key(&value.id) {
                return Err(StoreError::Conflict(format!("user {} already exists", value.id)));
            }
            v.insert(value.id, value.clone());
            Ok(())
        })?
    }

    async fn get_user(&self, ctx: &SecurityContext, id: UserId) -> StoreResult<User> {
        let _ = ctx; // Placeholder for security check
        let values = self.read_lock(&self.users, |v| v.clone())?;
        require_entity(values.get(&id).cloned(), "user", id.to_string())
    }

    async fn find_user_by_name(&self, _ctx: &SecurityContext, name: &str) -> StoreResult<User> {
        let _ = _ctx;
        let values = self.read_lock(&self.users, |v| v.clone())?;
        require_entity(
            values
                .values()
                .find(|t| t.username == name)
                .cloned(),
            "user",
            name.to_string(),
        )
    }

    async fn update_user(&self, ctx: &SecurityContext, value: &User) -> StoreResult<()> {
        let _ = ctx; // Placeholder for security check
        self.write_lock(&self.users, |v| {
            if !v.contains_key(&value.id) {
                return Err(StoreError::NotFound { entity: "user", id: value.id.to_string() });
            }
            v.insert(value.id, value.clone());
            Ok(())
        })?
    }

    async fn get_component(&self, ctx: &SecurityContext, id: &str) -> StoreResult<ComponentDescriptor> {
        let found = self.read_lock(&self.components, |v| v.get(id).cloned())?;
        self.access(ctx).read(found, |a| ActionType::GetComponent(a), "component", id.to_string()).await
    }

    async fn list_components(&self, ctx: &SecurityContext) -> StoreResult<Vec<ComponentDescriptor>> {
        let access = self.access(ctx);
        access.allow(ActionType::ListComponents).await?;
        let all = self.read_lock(&self.components, |v| v.values().cloned().collect())?;
        Ok(access.filter(all, |a| ActionType::GetComponent(a)).await)
    }

    async fn put_component(&self, ctx: &SecurityContext, value: &ComponentDescriptor) -> StoreResult<()> {
        let stored = self.read_lock(&self.components, |v| v.get(&value.id).cloned())?;
        self.access(ctx).put(
            stored.as_ref(), value,
            |a| ActionType::GetComponent(a), |a| ActionType::CreateComponent(a), |a| ActionType::UpdateComponent(a),
            "component", value.id.clone(),
        ).await?;
        self.write_lock(&self.components, |v| {
            v.insert(value.id.clone(), value.clone());
        })
    }

    async fn delete_component(&self, ctx: &SecurityContext, id: &str) -> StoreResult<bool> {
        let stored = self.read_lock(&self.components, |v| v.get(id).cloned())?;
        if !self.access(ctx).delete(stored.as_ref(), |a| ActionType::GetComponent(a), |a| ActionType::DeleteComponent(a)).await? {
            return Ok(false);
        }
        self.write_lock(&self.components, |v| v.remove(id).is_some())
    }
}

#[async_trait]
impl StateStore for MemoryStore {
    async fn get_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId) -> StoreResult<AgentChannelState> {
        let values = self.read_lock(&self.agent_channel_states, |v| v.clone())?;
        Ok(values.get(&(agent_id, channel_id)).cloned().unwrap_or_default())
    }

    async fn put_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId, state: &AgentChannelState) -> StoreResult<()> {
        self.write_lock(&self.agent_channel_states, |v| {
            v.insert((agent_id, channel_id), state.clone());
        })
    }

    async fn get_agent_state(&self, agent_id: AgentId) -> StoreResult<AgentState> {
        let values = self.read_lock(&self.agent_states, |v| v.clone())?;
        Ok(values.get(&agent_id).cloned().unwrap_or_default())
    }

    async fn put_agent_state(&self, agent_id: AgentId, state: &AgentState) -> StoreResult<()> {
        self.write_lock(&self.agent_states, |v| {
            v.insert(agent_id, state.clone());
        })
    }

    async fn get_session_state(&self, session_id: SessionId) -> StoreResult<SessionState> {
        let values = self.read_lock(&self.session_states, |v| v.clone())?;
        require_entity(
            values.get(&session_id).cloned(),
            "session_state",
            session_id.to_string(),
        )
    }

    async fn put_session_state(&self, session_id: SessionId, state: &SessionState) -> StoreResult<()> {
        self.write_lock(&self.session_states, |v| {
            v.insert(session_id, state.clone());
        })
    }

    async fn get_channel_state(&self, channel_id: ChannelId) -> StoreResult<ChannelState> {
        let values = self.read_lock(&self.channel_states, |v| v.clone())?;
        require_entity(values.get(&channel_id).cloned(), "channel_state", channel_id.to_string())
    }

    async fn put_channel_state(&self, channel_id: ChannelId, state: &ChannelState) -> StoreResult<()> {
        self.write_lock(&self.channel_states, |v| {
            v.insert(channel_id, state.clone());
        })
    }

    async fn delete_channel_state(&self, channel_id: ChannelId) -> StoreResult<()> {
        self.write_lock(&self.channel_states, |v| {
            v.remove(&channel_id);
        })?;
        self.write_lock(&self.agent_channel_states, |v| {
            v.retain(|(_, cid), _| *cid != channel_id);
        })
    }

    async fn delete_session_state(&self, session_id: SessionId) -> StoreResult<()> {
        self.write_lock(&self.session_states, |v| {
            v.remove(&session_id);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::Metadata;
    use crate::template::{ChannelTemplate, ParticipantTemplate, ScopeTemplate};
    use crate::user::UserId;
    use uuid::Uuid;

    fn make_personality(name: &str, scope: Scope) -> Personality {
        Personality {
            reasoning_confidentiality: crate::confidentiality::Confidentiality::Public,
            security_attributes: Default::default(),
            id: PersonalityId(Uuid::new_v4()),
            metadata: Metadata {
                name: name.to_string(),
                scope,
                ..Metadata::default()
            },
            components: crate::component::ComponentConfig::default(),
        }
    }

    fn make_session_template(name: &str, scope: Scope) -> SessionTemplate {
        SessionTemplate {
            security_attributes: Default::default(),
            commands: Default::default(),
            id: SessionTemplateId(Uuid::new_v4()),
            metadata: Metadata {
                name: name.to_string(),
                scope,
                ..Metadata::default()
            },
            scope: ScopeTemplate::GlobalScope,
            participants: vec![ParticipantTemplate::User{ name: "user".to_string() }],
            channels: vec![ChannelTemplate {
                name: "general".to_string(),
                moderator: "broadcast".to_string(),
                confidentiality: crate::confidentiality::Confidentiality::CHANNEL_DEFAULT,
                participants: vec![],
            }],
        }
    }

    #[tokio::test]
    async fn personality_lookup_honors_scope() {
        let store = MemoryStore::new();
        let ctx = SecurityContext::default();
        let user_id = UserId(Uuid::new_v4());
        let user_scope = Scope::UserScope { user_id };

        store
            .put_personality(&ctx, &make_personality("assistant", Scope::GlobalScope))
            .await
            .unwrap();
        store
            .put_personality(
                &ctx,
                &make_personality(
                    "assistant",
                    user_scope.clone(),
                ),
            )
            .await
            .unwrap();

        // Search with GlobalScope should find an entity (either GlobalScope or UserScope)
        let found = store
            .find_personality_by_name(&ctx, "assistant", &Scope::GlobalScope)
            .await
            .unwrap();
        assert!(Scope::GlobalScope.contains(&found.metadata.scope));

        // Search with UserScope should find the UserScope entity (it's contained in UserScope, while GlobalScope is not)
        let found = store
            .find_personality_by_name(&ctx, "assistant", &user_scope)
            .await
            .unwrap();
        assert_eq!(found.metadata.scope, user_scope);
    }

    #[tokio::test]
    async fn session_template_lookup_honors_scope() {
        let store = MemoryStore::new();
        let ctx = SecurityContext::default();
        let user_scope = Scope::UserScope {
            user_id: UserId(Uuid::new_v4()),
        };

        store
            .put_session_template(&ctx, &make_session_template("pairing", Scope::GlobalScope))
            .await
            .unwrap();
        store
            .put_session_template(&ctx, &make_session_template("pairing", user_scope.clone()))
            .await
            .unwrap();

        // UserScope search should find UserScope entity (GlobalScope is not contained in UserScope)
        let loaded = store
            .find_session_template_by_name(&ctx, "pairing", &user_scope)
            .await
            .unwrap();
        assert_eq!(loaded.metadata.scope, user_scope);

        // GlobalScope search should find an entity (GlobalScope or UserScope)
        let loaded = store
            .find_session_template_by_name(&ctx, "pairing", &Scope::GlobalScope)
            .await
            .unwrap();
        assert!(Scope::GlobalScope.contains(&loaded.metadata.scope));
    }

    #[tokio::test]
    async fn create_agent_conflict_is_reported() {
        let store = MemoryStore::new();
        let ctx = SecurityContext::default();
        let personality = make_personality("assistant", Scope::GlobalScope);
        let agent = Agent::new("assistant-1".to_string(), Scope::GlobalScope, &personality, SecurityContext::default());

        store.create_agent(&ctx, &agent).await.unwrap();
        let conflict = store.create_agent(&ctx, &agent).await.unwrap_err();
        assert!(matches!(conflict, StoreError::Conflict(_)));
    }
}