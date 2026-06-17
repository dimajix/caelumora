use std::fmt::Debug;
use std::sync::Arc;

use crate::agent::{Agent, AgentId, AgentState, Personality, PersonalityId};
use crate::meta::Scope;
use crate::session::{Session, SessionId, SessionState};
use crate::template::{SessionTemplate, SessionTemplateId};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("not found: {entity} {id}")]
    NotFound { entity: &'static str, id: String },
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("validation: {0}")]
    Validation(String),
    #[error("backend: {0}")]
    Backend(String),
}

pub type StoreResult<T> = Result<T, StoreError>;


pub trait EntityResolver : Debug {
    fn get_session_template(&self, name: &str, scope: &Scope) -> StoreResult<SessionTemplate>;
    fn get_personality(&self, name: &str, scope: &Scope) -> StoreResult<Personality>;
    fn get_or_create_agent(&self, name: &str, scope: &Scope, personality_id: PersonalityId) -> StoreResult<Agent>;
}

#[derive(Debug)]
pub struct StoreBackedEntityResolver<TS: ?Sized, SS: ?Sized> {
    template_store: Arc<TS>,
    state_store: Arc<SS>,
}

impl<TS: ?Sized, SS: ?Sized> StoreBackedEntityResolver<TS, SS> {
    pub fn new(template_store: Arc<TS>, state_store: Arc<SS>) -> Self {
        Self {
            template_store,
            state_store,
        }
    }
}

impl<TS, SS> EntityResolver for StoreBackedEntityResolver<TS, SS>
where
    TS: TemplateStore + ?Sized,
    SS: StateStore + ?Sized,
{
    fn get_session_template(&self, name: &str, scope: &Scope) -> StoreResult<SessionTemplate> {
        self.template_store.find_session_template_by_name(name, scope)
    }

    fn get_personality(&self, name: &str, scope: &Scope) -> StoreResult<Personality> {
        self.template_store.find_personality_by_name(name, scope)
    }

    fn get_or_create_agent(&self, name: &str, scope: &Scope, personality_id: PersonalityId) -> StoreResult<Agent> {
        if let Some(agent) = self.state_store.find_agent_by_name(name, scope)? {
            return Ok(agent);
        }

        let personality = self.template_store.get_personality(personality_id)?;
        let agent = Agent::new(name.to_string(), scope.clone(), &personality);
        self.state_store.create_agent(&agent)?;
        Ok(agent)
    }
}


pub trait TemplateStore : Debug + Sync + Send {
    fn get_personality(&self, id: PersonalityId) -> StoreResult<Personality>;
    fn find_personality_by_name(&self, name: &str, scope: &Scope) -> StoreResult<Personality>;
    fn list_personalities(&self) -> StoreResult<Vec<Personality>>;
    fn put_personality(&self, value: &Personality) -> StoreResult<()>;
    fn delete_personality(&self, id: PersonalityId) -> StoreResult<bool>;

    fn find_session_template_by_name(&self, name: &str, scope: &Scope) -> StoreResult<SessionTemplate>;
    fn list_session_templates(&self) -> StoreResult<Vec<SessionTemplate>>;
    fn put_session_template(&self, value: &SessionTemplate) -> StoreResult<()>;
    fn delete_session_template(&self, id: SessionTemplateId) -> StoreResult<bool>;
}


pub trait StateStore : Debug + Sync + Send {
    fn create_agent(&self, value: &Agent) -> StoreResult<()>;
    fn get_agent(&self, id: AgentId) -> StoreResult<Agent>;
    fn find_agent_by_name(&self, name: &str, scope: &Scope) -> StoreResult<Option<Agent>>;
    fn update_agent(&self, value: &Agent) -> StoreResult<()>;

    fn create_session(&self, value: &Session) -> StoreResult<()>;
    fn get_session(&self, id: SessionId) -> StoreResult<Session>;
    fn update_session(&self, value: &Session) -> StoreResult<()>;

    fn get_agent_state(&self, agent_id: AgentId) -> StoreResult<AgentState>;
    fn put_agent_state(&self, agent_id: AgentId, state: &AgentState) -> StoreResult<()>;

    fn get_session_state(&self, session_id: SessionId) -> StoreResult<SessionState>;
    fn put_session_state(&self, session_id: SessionId, state: &SessionState) -> StoreResult<()>;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::RwLock;
    use uuid::Uuid;
    use super::*;
    use crate::meta::Metadata;
    use crate::template::{ScopeTemplate, SessionTemplateId};
    use crate::user::UserId;

    #[derive(Debug)]
    struct MockTemplateStore {
        personality: Personality,
    }

    impl TemplateStore for MockTemplateStore {
        fn get_personality(&self, id: PersonalityId) -> StoreResult<Personality> {
            if self.personality.id == id {
                Ok(self.personality.clone())
            } else {
                Err(StoreError::NotFound {
                    entity: "agent_template",
                    id: id.to_string(),
                })
            }
        }

        fn find_personality_by_name(&self, _name: &str, _scope: &Scope) -> StoreResult<Personality> {
            Ok(self.personality.clone())
        }

        fn list_personalities(&self) -> StoreResult<Vec<Personality>> {
            unimplemented!()
        }

        fn put_personality(&self, _value: &Personality) -> StoreResult<()> {
            unimplemented!()
        }

        fn delete_personality(&self, _id: PersonalityId) -> StoreResult<bool> {
            unimplemented!()
        }

        fn find_session_template_by_name(&self, _name: &str, _scope: &Scope) -> StoreResult<SessionTemplate> {
            Ok(SessionTemplate {
                id: SessionTemplateId(Uuid::new_v4()),
                metadata: Metadata::default(),
                scope: ScopeTemplate::GlobalScope,
                channels: Vec::new(),
            })
        }

        fn list_session_templates(&self) -> StoreResult<Vec<SessionTemplate>> {
            unimplemented!()
        }

        fn put_session_template(&self, _value: &SessionTemplate) -> StoreResult<()> {
            unimplemented!()
        }

        fn delete_session_template(&self, _id: SessionTemplateId) -> StoreResult<bool> {
            unimplemented!()
        }
    }

    #[derive(Debug, Default)]
    struct MockStateStore {
        agents: RwLock<Vec<Agent>>,
        sessions: RwLock<Vec<Session>>,
    }

    impl MockStateStore {
        fn read_agents(&self) -> Vec<Agent> {
            match self.agents.read() {
                Ok(guard) => guard.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        }
        fn write_agents<F, R>(&self, f: F) -> R
        where
            F: FnOnce(&mut Vec<Agent>) -> R,
        {
            match self.agents.write() {
                Ok(mut guard) => f(&mut guard),
                Err(mut poisoned) => f(poisoned.get_mut()),
            }
        }
        fn read_sessions(&self) -> Vec<Session> {
            match self.sessions.read() {
                Ok(guard) => guard.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        }
        fn write_sessions<F, R>(&self, f: F) -> R
        where
            F: FnOnce(&mut Vec<Session>) -> R,
        {
            match self.sessions.write() {
                Ok(mut guard) => f(&mut guard),
                Err(mut poisoned) => f(poisoned.get_mut()),
            }
        }
    }

    impl StateStore for MockStateStore {
        fn create_agent(&self, value: &Agent) -> StoreResult<()> {
            self.write_agents(|v| v.push(value.clone()));
            Ok(())
        }

        fn get_agent(&self, _id: AgentId) -> StoreResult<Agent> {
            unimplemented!()
        }

        fn find_agent_by_name(&self, name: &str, scope: &Scope) -> StoreResult<Option<Agent>> {
            Ok(self
                .read_agents()
                .iter()
                .find(|a| a.metadata.name == name && a.metadata.scope == *scope)
                .cloned())
        }

        fn update_agent(&self, value: &Agent) -> StoreResult<()> {
            self.write_agents(|v| {
                if let Some(existing) = v.iter_mut().find(|a| a.id == value.id) {
                    *existing = value.clone();
                }
            });
            Ok(())
        }

        fn create_session(&self, value: &Session) -> StoreResult<()> {
            self.write_sessions(|v| v.push(value.clone()));
            Ok(())
        }

        fn get_session(&self, _id: SessionId) -> StoreResult<Session> {
            unimplemented!()
        }

        fn update_session(&self, value: &Session) -> StoreResult<()> {
            self.write_sessions(|v| {
                if let Some(existing) = v.iter_mut().find(|s| s.id == value.id) {
                    *existing = value.clone();
                }
            });
            Ok(())
        }

        fn get_agent_state(&self, _agent_id: AgentId) -> StoreResult<AgentState> {
            unimplemented!()
        }

        fn put_agent_state(&self, _agent_id: AgentId, _state: &AgentState) -> StoreResult<()> {
            unimplemented!()
        }

        fn get_session_state(&self, _session_id: SessionId) -> StoreResult<SessionState> {
            unimplemented!()
        }

        fn put_session_state(&self, _session_id: SessionId, _state: &SessionState) -> StoreResult<()> {
            unimplemented!()
        }
    }

    fn personality_template() -> Personality {
        Personality {
            id: PersonalityId(Uuid::new_v4()),
            metadata: Metadata {
                name: "helper".to_string(),
                scope: Scope::GlobalScope,
                owner: UserId(Uuid::new_v4()),
                description: "Helpful assistant".to_string(),
            },
            components: crate::component::ComponentConfig::default(),
        }
    }

    #[test]
    fn get_or_create_agent_returns_existing_agent() {
        let personality = personality_template();
        let existing = Agent::new("assistant".to_string(), Scope::GlobalScope, &personality);

        let resolver = StoreBackedEntityResolver::new(
            Arc::new(MockTemplateStore {
                personality: personality.clone(),
            }),
            Arc::new(MockStateStore {
                agents: RwLock::new(vec![existing.clone()]),
                ..MockStateStore::default()
            }),
        );

        let resolved = resolver
            .get_or_create_agent("assistant", &Scope::GlobalScope, personality.id)
            .unwrap();

        assert_eq!(resolved.id, existing.id);
    }

    #[test]
    fn get_or_create_agent_creates_agent_when_missing() {
        let personality = personality_template();
        let state_store = Arc::new(MockStateStore {
            agents: RwLock::new(Vec::new()),
            ..MockStateStore::default()
        });

        let resolver = StoreBackedEntityResolver::new(
            Arc::new(MockTemplateStore {
                personality: personality.clone(),
            }),
            state_store,
        );

        let created = resolver
            .get_or_create_agent("assistant", &Scope::GlobalScope, personality.id)
            .unwrap();
        let fetched = resolver
            .get_or_create_agent("assistant", &Scope::GlobalScope, personality.id)
            .unwrap();

        assert_eq!(created.id, fetched.id);
        assert_eq!(created.personality_id, personality.id);
    }

    #[test]
    fn agent_controller_persists_changes() {
        let personality = personality_template();
        let agent = Agent::new("assistant".to_string(), Scope::GlobalScope, &personality);
        let state_store = Arc::new(MockStateStore::default());
        state_store.create_agent(&agent).unwrap();

        let event_sender = Arc::new(crate::event::FnEventSender::new(|_| {}));
        let controller = crate::agent::AgentController::new(agent.clone(), event_sender, state_store.clone());

        controller.update_name("new_name".to_string());

        let persisted = state_store.find_agent_by_name("new_name", &Scope::GlobalScope).unwrap().unwrap();
        assert_eq!(persisted.id, agent.id);
        assert_eq!(persisted.metadata.name, "new_name");
    }

    #[test]
    fn session_controller_persists_changes() {
        let session = Session {
            id: SessionId(Uuid::new_v4()),
            metadata: Metadata {
                name: "session".to_string(),
                scope: Scope::GlobalScope,
                ..Metadata::default()
            },
            channels: Vec::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let state_store = Arc::new(MockStateStore::default());
        state_store.create_session(&session).unwrap();

        let event_sender = Arc::new(crate::event::FnEventSender::new(|_| {}));
        let controller = crate::session::SessionController::new(session.clone(), event_sender, state_store.clone());

        controller.update_name("new_session_name".to_string());

        let persisted = state_store.write_sessions(|v| v.iter().find(|s| s.metadata.name == "new_session_name").cloned().unwrap());
        assert_eq!(persisted.id, session.id);
        assert_eq!(persisted.metadata.name, "new_session_name");
    }
}
