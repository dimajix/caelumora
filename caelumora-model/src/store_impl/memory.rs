use std::collections::HashMap;
use std::sync::RwLock;
use log::info;
use crate::agent::{Agent, AgentId, AgentState, Personality, PersonalityId};
use crate::meta::Scope;
use crate::session::{Session, SessionId, SessionState};
use crate::store::{StateStore, StoreError, StoreResult, TemplateStore};
use crate::template::{SessionTemplate, SessionTemplateId};

#[derive(Debug, Default)]
pub struct MemoryStore {
    personalities: RwLock<HashMap<PersonalityId, Personality>>,
    session_templates: RwLock<HashMap<String, SessionTemplate>>,
    agents: RwLock<HashMap<AgentId, Agent>>,
    sessions: RwLock<HashMap<SessionId, Session>>,
    agent_states: RwLock<HashMap<AgentId, AgentState>>,
    session_states: RwLock<HashMap<SessionId, SessionState>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        info!("Creating memory store");
        Self::default()
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

impl TemplateStore for MemoryStore {
    fn get_personality(&self, id: PersonalityId) -> StoreResult<Personality> {
        let values = self.read_lock(&self.personalities, |v| v.clone())?;
        require_entity(values.get(&id).cloned(), "agent_template", id.to_string())
    }

    fn find_personality_by_name(&self, name: &str, scope: &Scope) -> StoreResult<Personality> {
        let values = self.read_lock(&self.personalities, |v| v.clone())?;
        require_entity(
            values
                .values()
                .find(|t| t.metadata.name == name && t.metadata.scope == *scope)
                .cloned(),
            "agent_template",
            name.to_string(),
        )
    }

    fn list_personalities(&self) -> StoreResult<Vec<Personality>> {
        self.read_lock(&self.personalities, |v| v.values().cloned().collect())
    }

    fn put_personality(&self, value: &Personality) -> StoreResult<()> {
        self.write_lock(&self.personalities, |v| {
            v.insert(value.id, value.clone());
        })
    }

    fn delete_personality(&self, id: PersonalityId) -> StoreResult<bool> {
        self.write_lock(&self.personalities, |v| v.remove(&id).is_some())
    }

    fn find_session_template_by_name(&self, name: &str, scope: &Scope) -> StoreResult<SessionTemplate> {
        let values = self.read_lock(&self.session_templates, |v| v.clone())?;
        require_entity(
            values
                .values()
                .find(|t| t.metadata.name == name && t.metadata.scope == *scope)
                .cloned(),
            "session_template",
            name.to_string(),
        )
    }

    fn list_session_templates(&self) -> StoreResult<Vec<SessionTemplate>> {
        self.read_lock(&self.session_templates, |v| v.values().cloned().collect())
    }

    fn put_session_template(&self, value: &SessionTemplate) -> StoreResult<()> {
        self.write_lock(&self.session_templates, |v| {
            v.insert(value.metadata.name.clone(), value.clone());
        })
    }

    fn delete_session_template(&self, id: SessionTemplateId) -> StoreResult<bool> {
        self.write_lock(&self.session_templates, |v| {
            let key = v
                .values()
                .find(|t| t.id == id)
                .map(|t| t.metadata.name.clone());
            match key {
                Some(name) => v.remove(&name).is_some(),
                None => false,
            }
        })
    }
}

impl StateStore for MemoryStore {
    fn create_agent(&self, value: &Agent) -> StoreResult<()> {
        self.write_lock(&self.agents, |v| {
            if v.contains_key(&value.id) {
                return Err(StoreError::Conflict(format!("agent {} already exists", value.id)));
            }
            v.insert(value.id, value.clone());
            Ok(())
        })?
    }

    fn get_agent(&self, id: AgentId) -> StoreResult<Agent> {
        let values = self.read_lock(&self.agents, |v| v.clone())?;
        require_entity(values.get(&id).cloned(), "agent", id.to_string())
    }

    fn find_agent_by_name(&self, name: &str, scope: &Scope) -> StoreResult<Option<Agent>> {
        self.read_lock(&self.agents, |v| {
            v.values()
                .find(|a| a.metadata.name == name && a.metadata.scope == *scope)
                .cloned()
        })
    }

    fn update_agent(&self, value: &Agent) -> StoreResult<()> {
        self.write_lock(&self.agents, |v| {
            if !v.contains_key(&value.id) {
                return Err(StoreError::NotFound {
                    entity: "agent",
                    id: value.id.to_string(),
                });
            }
            v.insert(value.id, value.clone());
            Ok(())
        })?
    }

    fn create_session(&self, value: &Session) -> StoreResult<()> {
        self.write_lock(&self.sessions, |v| {
            if v.contains_key(&value.id) {
                return Err(StoreError::Conflict(format!("session {} already exists", value.id)));
            }
            v.insert(value.id, value.clone());
            Ok(())
        })?
    }

    fn get_session(&self, id: SessionId) -> StoreResult<Session> {
        let values = self.read_lock(&self.sessions, |v| v.clone())?;
        require_entity(values.get(&id).cloned(), "session", id.to_string())
    }

    fn update_session(&self, value: &Session) -> StoreResult<()> {
        self.write_lock(&self.sessions, |v| {
            if !v.contains_key(&value.id) {
                return Err(StoreError::NotFound {
                    entity: "session",
                    id: value.id.to_string(),
                });
            }
            v.insert(value.id, value.clone());
            Ok(())
        })?
    }

    fn get_agent_state(&self, agent_id: AgentId) -> StoreResult<AgentState> {
        let values = self.read_lock(&self.agent_states, |v| v.clone())?;
        require_entity(values.get(&agent_id).cloned(), "agent_state", agent_id.to_string())
    }

    fn put_agent_state(&self, agent_id: AgentId, state: &AgentState) -> StoreResult<()> {
        self.write_lock(&self.agent_states, |v| {
            v.insert(agent_id, state.clone());
        })
    }

    fn get_session_state(&self, session_id: SessionId) -> StoreResult<SessionState> {
        let values = self.read_lock(&self.session_states, |v| v.clone())?;
        require_entity(
            values.get(&session_id).cloned(),
            "session_state",
            session_id.to_string(),
        )
    }

    fn put_session_state(&self, session_id: SessionId, state: &SessionState) -> StoreResult<()> {
        self.write_lock(&self.session_states, |v| {
            v.insert(session_id, state.clone());
        })
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;
    use super::*;
    use crate::meta::Metadata;
    use crate::template::{ChannelTemplate, ParticipantTemplate, ScopeTemplate};
    use crate::user::UserId;

    fn make_personality(name: &str, scope: Scope) -> Personality {
        Personality {
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
            id: SessionTemplateId(Uuid::new_v4()),
            metadata: Metadata {
                name: name.to_string(),
                scope,
                ..Metadata::default()
            },
            scope: ScopeTemplate::GlobalScope,
            channels: vec![ChannelTemplate {
                name: "general".to_string(),
                moderator: "broadcast".to_string(),
                participants: vec![ParticipantTemplate::User],
            }],
        }
    }

    #[test]
    fn personality_lookup_honors_scope() {
        let store = MemoryStore::new();
        store
            .put_personality(&make_personality("assistant", Scope::GlobalScope))
            .unwrap();
        store
            .put_personality(&make_personality(
                "assistant",
                Scope::UserScope {
                    user_id: UserId(Uuid::new_v4()),
                },
            ))
            .unwrap();

        let global = store
            .find_personality_by_name("assistant", &Scope::GlobalScope)
            .unwrap();
        assert_eq!(global.metadata.scope, Scope::GlobalScope);
    }

    #[test]
    fn session_template_lookup_honors_scope() {
        let store = MemoryStore::new();
        let user_scope = Scope::UserScope {
            user_id: UserId(Uuid::new_v4()),
        };

        store
            .put_session_template(&make_session_template("pairing", Scope::GlobalScope))
            .unwrap();
        store
            .put_session_template(&make_session_template("pairing", user_scope.clone()))
            .unwrap();

        let loaded = store
            .find_session_template_by_name("pairing", &user_scope)
            .unwrap();
        assert_eq!(loaded.metadata.scope, user_scope);
    }

    #[test]
    fn create_agent_conflict_is_reported() {
        let store = MemoryStore::new();
        let personality = make_personality("assistant", Scope::GlobalScope);
        let agent = Agent::new("assistant-1".to_string(), Scope::GlobalScope, &personality);

        store.create_agent(&agent).unwrap();
        let conflict = store.create_agent(&agent).unwrap_err();
        assert!(matches!(conflict, StoreError::Conflict(_)));
    }
}