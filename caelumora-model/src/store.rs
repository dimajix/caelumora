use uuid::Uuid;

use crate::agent::{Agent, AgentState, Personality};
use crate::meta::Scope;
use crate::session::{Session, SessionState};
use crate::template::{SessionTemplate};

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


pub trait EntityResolver {
    fn get_session_template(&self, name: &str, scope: &Scope) -> StoreResult<SessionTemplate>;
    fn get_personality(&self, name: &str, scope: &Scope) -> StoreResult<Personality>;
    fn get_or_create_agent(&self, name: &str, scope: &Scope, personality_id: Uuid) -> StoreResult<Agent>;
}

pub struct StoreBackedEntityResolver<TS, SS> {
    template_store: TS,
    state_store: SS,
}

impl<TS, SS> StoreBackedEntityResolver<TS, SS> {
    pub fn new(template_store: TS, state_store: SS) -> Self {
        Self {
            template_store,
            state_store,
        }
    }
}

impl<TS, SS> EntityResolver for StoreBackedEntityResolver<TS, SS>
where
    TS: TemplateStore,
    SS: StateStore,
{
    fn get_session_template(&self, name: &str, scope: &Scope) -> StoreResult<SessionTemplate> {
        self.template_store.find_session_template_by_name(name, scope)
    }

    fn get_personality(&self, name: &str, scope: &Scope) -> StoreResult<Personality> {
        self.template_store.find_personality_by_name(name, scope)
    }

    fn get_or_create_agent(&self, name: &str, scope: &Scope, personality_id: Uuid) -> StoreResult<Agent> {
        if let Some(agent) = self.state_store.find_agent_by_name(name, scope)? {
            return Ok(agent);
        }

        let personality = self.template_store.get_personality(personality_id)?;
        let agent = Agent::new(name.to_string(), scope.clone(), &personality);
        self.state_store.create_agent(agent.clone())?;
        Ok(agent)
    }
}


pub trait TemplateStore {
    fn get_personality(&self, id: Uuid) -> StoreResult<Personality>;
    fn find_personality_by_name(&self, name: &str, scope: &Scope) -> StoreResult<Personality>;
    fn list_personalities(&self) -> StoreResult<Vec<Personality>>;
    fn put_personality(&self, value: Personality) -> StoreResult<()>;
    fn delete_personality(&self, id: Uuid) -> StoreResult<bool>;

    fn find_session_template_by_name(&self, name: &str, scope: &Scope) -> StoreResult<SessionTemplate>;
    fn list_session_templates(&self) -> StoreResult<Vec<SessionTemplate>>;
    fn put_session_template(&self, value: SessionTemplate) -> StoreResult<()>;
    fn delete_session_template(&self, id: Uuid) -> StoreResult<bool>;
}


pub trait StateStore {
    fn create_agent(&self, value: Agent) -> StoreResult<()>;
    fn get_agent(&self, id: Uuid) -> StoreResult<Agent>;
    fn find_agent_by_name(&self, name: &str, scope: &Scope) -> StoreResult<Option<Agent>>;
    fn update_agent(&self, value: Agent) -> StoreResult<()>;

    fn create_session(&self, value: Session) -> StoreResult<()>;
    fn get_session(&self, id: Uuid) -> StoreResult<Session>;
    fn update_session(&self, value: Session) -> StoreResult<()>;

    fn get_agent_state(&self, agent_id: Uuid) -> StoreResult<AgentState>;
    fn put_agent_state(&self, agent_id: Uuid, state: AgentState) -> StoreResult<()>;

    fn get_session_state(&self, session_id: Uuid) -> StoreResult<SessionState>;
    fn put_session_state(&self, session_id: Uuid, state: SessionState) -> StoreResult<()>;
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::meta::Metadata;
    use crate::template::ScopeTemplate;

    struct MockTemplateStore {
        personality: Personality,
    }

    impl TemplateStore for MockTemplateStore {
        fn get_personality(&self, id: Uuid) -> StoreResult<Personality> {
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

        fn put_personality(&self, _value: Personality) -> StoreResult<()> {
            unimplemented!()
        }

        fn delete_personality(&self, _id: Uuid) -> StoreResult<bool> {
            unimplemented!()
        }

        fn find_session_template_by_name(&self, _name: &str, _scope: &Scope) -> StoreResult<SessionTemplate> {
            Ok(SessionTemplate {
                id: Uuid::new_v4(),
                metadata: Metadata::default(),
                scope: ScopeTemplate::GlobalScope,
                participants: Vec::new(),
            })
        }

        fn list_session_templates(&self) -> StoreResult<Vec<SessionTemplate>> {
            unimplemented!()
        }

        fn put_session_template(&self, _value: SessionTemplate) -> StoreResult<()> {
            unimplemented!()
        }

        fn delete_session_template(&self, _id: Uuid) -> StoreResult<bool> {
            unimplemented!()
        }
    }

    struct MockStateStore {
        agents: RefCell<Vec<Agent>>,
    }

    impl StateStore for MockStateStore {
        fn create_agent(&self, value: Agent) -> StoreResult<()> {
            self.agents.borrow_mut().push(value);
            Ok(())
        }

        fn get_agent(&self, _id: Uuid) -> StoreResult<Agent> {
            unimplemented!()
        }

        fn find_agent_by_name(&self, name: &str, scope: &Scope) -> StoreResult<Option<Agent>> {
            Ok(self
                .agents
                .borrow()
                .iter()
                .find(|a| a.metadata.name == name && a.metadata.scope == *scope)
                .cloned())
        }

        fn update_agent(&self, _value: Agent) -> StoreResult<()> {
            unimplemented!()
        }

        fn create_session(&self, _value: Session) -> StoreResult<()> {
            unimplemented!()
        }

        fn get_session(&self, _id: Uuid) -> StoreResult<Session> {
            unimplemented!()
        }

        fn update_session(&self, _value: Session) -> StoreResult<()> {
            unimplemented!()
        }

        fn get_agent_state(&self, _agent_id: Uuid) -> StoreResult<AgentState> {
            unimplemented!()
        }

        fn put_agent_state(&self, _agent_id: Uuid, _state: AgentState) -> StoreResult<()> {
            unimplemented!()
        }

        fn get_session_state(&self, _session_id: Uuid) -> StoreResult<SessionState> {
            unimplemented!()
        }

        fn put_session_state(&self, _session_id: Uuid, _state: SessionState) -> StoreResult<()> {
            unimplemented!()
        }
    }

    fn personality_template() -> Personality {
        Personality {
            id: Uuid::new_v4(),
            metadata: Metadata {
                name: "helper".to_string(),
                scope: Scope::GlobalScope,
                owner: Uuid::new_v4(),
                description: "Helpful assistant".to_string(),
            },
        }
    }

    #[test]
    fn get_or_create_agent_returns_existing_agent() {
        let personality = personality_template();
        let existing = Agent::new("assistant".to_string(), Scope::GlobalScope, &personality);

        let resolver = StoreBackedEntityResolver::new(
            MockTemplateStore {
                personality: personality.clone(),
            },
            MockStateStore {
                agents: RefCell::new(vec![existing.clone()]),
            },
        );

        let resolved = resolver
            .get_or_create_agent("assistant", &Scope::GlobalScope, personality.id)
            .unwrap();

        assert_eq!(resolved.id, existing.id);
    }

    #[test]
    fn get_or_create_agent_creates_agent_when_missing() {
        let personality = personality_template();
        let state_store = MockStateStore {
            agents: RefCell::new(Vec::new()),
        };

        let resolver = StoreBackedEntityResolver::new(
            MockTemplateStore {
                personality: personality.clone(),
            },
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
}
