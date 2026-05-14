use std::collections::HashMap;
use std::sync::RwLock;

use uuid::Uuid;

use crate::agent::{Agent, AgentState, Personality};
use crate::meta::Scope;
use crate::session::{Session, SessionState};
use crate::store::{StateStore, StoreError, StoreResult, TemplateStore};
use crate::template::SessionTemplate;

#[derive(Debug, Default)]
pub struct MemoryStore {
    personalities: RwLock<HashMap<Uuid, Personality>>,
    session_templates: RwLock<HashMap<String, SessionTemplate>>,
    agents: RwLock<HashMap<Uuid, Agent>>,
    sessions: RwLock<HashMap<Uuid, Session>>,
    agent_states: RwLock<HashMap<Uuid, AgentState>>,
    session_states: RwLock<HashMap<Uuid, SessionState>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

fn require_entity<T>(value: Option<T>, entity: &'static str, id: String) -> StoreResult<T> {
    value.ok_or(StoreError::NotFound { entity, id })
}

impl TemplateStore for MemoryStore {
    fn get_personality(&self, id: Uuid) -> StoreResult<Personality> {
        let values = self
            .personalities
            .read()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        require_entity(values.get(&id).cloned(), "agent_template", id.to_string())
    }

    fn find_personality_by_name(&self, name: &str, scope: &Scope) -> StoreResult<Personality> {
        let values = self
            .personalities
            .read()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
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
        let values = self
            .personalities
            .read()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        Ok(values.values().cloned().collect())
    }

    fn put_personality(&self, value: Personality) -> StoreResult<()> {
        let mut values = self
            .personalities
            .write()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        values.insert(value.id, value);
        Ok(())
    }

    fn delete_personality(&self, id: Uuid) -> StoreResult<bool> {
        let mut values = self
            .personalities
            .write()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        Ok(values.remove(&id).is_some())
    }

    fn find_session_template_by_name(&self, name: &str, scope: &Scope) -> StoreResult<SessionTemplate> {
        let values = self
            .session_templates
            .read()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
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
        let values = self
            .session_templates
            .read()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        Ok(values.values().cloned().collect())
    }

    fn put_session_template(&self, value: SessionTemplate) -> StoreResult<()> {
        let mut values = self
            .session_templates
            .write()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        values.insert(value.metadata.name.clone(), value);
        Ok(())
    }

    fn delete_session_template(&self, id: Uuid) -> StoreResult<bool> {
        let mut values = self
            .session_templates
            .write()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        let key = values
            .values()
            .find(|t| t.id == id)
            .map(|t| t.metadata.name.clone());
        match key {
            Some(name) => Ok(values.remove(&name).is_some()),
            None => Ok(false),
        }
    }
}

impl StateStore for MemoryStore {
    fn create_agent(&self, value: Agent) -> StoreResult<()> {
        let mut values = self
            .agents
            .write()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        if values.contains_key(&value.id) {
            return Err(StoreError::Conflict(format!("agent {} already exists", value.id)));
        }
        values.insert(value.id, value);
        Ok(())
    }

    fn get_agent(&self, id: Uuid) -> StoreResult<Agent> {
        let values = self
            .agents
            .read()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        require_entity(values.get(&id).cloned(), "agent", id.to_string())
    }

    fn find_agent_by_name(&self, name: &str, scope: &Scope) -> StoreResult<Option<Agent>> {
        let values = self
            .agents
            .read()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        Ok(values
            .values()
            .find(|a| a.metadata.name == name && a.metadata.scope == *scope)
            .cloned())
    }

    fn update_agent(&self, value: Agent) -> StoreResult<()> {
        let mut values = self
            .agents
            .write()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        if !values.contains_key(&value.id) {
            return Err(StoreError::NotFound {
                entity: "agent",
                id: value.id.to_string(),
            });
        }
        values.insert(value.id, value);
        Ok(())
    }

    fn create_session(&self, value: Session) -> StoreResult<()> {
        let mut values = self
            .sessions
            .write()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        if values.contains_key(&value.id) {
            return Err(StoreError::Conflict(format!("session {} already exists", value.id)));
        }
        values.insert(value.id, value);
        Ok(())
    }

    fn get_session(&self, id: Uuid) -> StoreResult<Session> {
        let values = self
            .sessions
            .read()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        require_entity(values.get(&id).cloned(), "session", id.to_string())
    }

    fn update_session(&self, value: Session) -> StoreResult<()> {
        let mut values = self
            .sessions
            .write()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        if !values.contains_key(&value.id) {
            return Err(StoreError::NotFound {
                entity: "session",
                id: value.id.to_string(),
            });
        }
        values.insert(value.id, value);
        Ok(())
    }

    fn get_agent_state(&self, agent_id: Uuid) -> StoreResult<AgentState> {
        let values = self
            .agent_states
            .read()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        require_entity(values.get(&agent_id).cloned(), "agent_state", agent_id.to_string())
    }

    fn put_agent_state(&self, agent_id: Uuid, state: AgentState) -> StoreResult<()> {
        let mut values = self
            .agent_states
            .write()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        values.insert(agent_id, state);
        Ok(())
    }

    fn get_session_state(&self, session_id: Uuid) -> StoreResult<SessionState> {
        let values = self
            .session_states
            .read()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        require_entity(
            values.get(&session_id).cloned(),
            "session_state",
            session_id.to_string(),
        )
    }

    fn put_session_state(&self, session_id: Uuid, state: SessionState) -> StoreResult<()> {
        let mut values = self
            .session_states
            .write()
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        values.insert(session_id, state);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::Metadata;
    use crate::template::{ParticipantTemplate, ScopeTemplate};

    fn make_personality(name: &str, scope: Scope) -> Personality {
        Personality {
            id: Uuid::new_v4(),
            metadata: Metadata {
                name: name.to_string(),
                scope,
                ..Metadata::default()
            },
        }
    }

    fn make_session_template(name: &str, scope: Scope) -> SessionTemplate {
        SessionTemplate {
            id: Uuid::new_v4(),
            metadata: Metadata {
                name: name.to_string(),
                scope,
                ..Metadata::default()
            },
            scope: ScopeTemplate::GlobalScope,
            participants: vec![ParticipantTemplate::User],
        }
    }

    #[test]
    fn personality_lookup_honors_scope() {
        let store = MemoryStore::new();
        store
            .put_personality(make_personality("assistant", Scope::GlobalScope))
            .unwrap();
        store
            .put_personality(make_personality(
                "assistant",
                Scope::UserScope {
                    user_id: Uuid::new_v4(),
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
            user_id: Uuid::new_v4(),
        };

        store
            .put_session_template(make_session_template("pairing", Scope::GlobalScope))
            .unwrap();
        store
            .put_session_template(make_session_template("pairing", user_scope.clone()))
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

        store.create_agent(agent.clone()).unwrap();
        let conflict = store.create_agent(agent).unwrap_err();
        assert!(matches!(conflict, StoreError::Conflict(_)));
    }
}