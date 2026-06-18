use async_trait::async_trait;
use std::fmt::Debug;

use crate::agent::{Agent, AgentId, AgentState, Personality, PersonalityId};
use crate::meta::Scope;
use crate::session::{Participant, Session, SessionId, SessionState};
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
    #[error("permission denied")]
    PermissionDenied,
}

pub type StoreResult<T> = Result<T, StoreError>;


use crate::security::SecurityContext;

#[async_trait]
pub trait TemplateStore : Debug + Sync + Send {
    async fn get_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<Personality>;
    async fn find_personality_by_name(&self, ctx: &SecurityContext, name: &str, scope: &Scope) -> StoreResult<Personality>;
    async fn list_personalities(&self, ctx: &SecurityContext) -> StoreResult<Vec<Personality>>;
    async fn put_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()>;
    async fn delete_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<bool>;

    async fn find_session_template_by_name(&self, ctx: &SecurityContext, name: &str, scope: &Scope) -> StoreResult<SessionTemplate>;
    async fn list_session_templates(&self, ctx: &SecurityContext) -> StoreResult<Vec<SessionTemplate>>;
    async fn put_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()>;
    async fn delete_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<bool>;
}


#[async_trait]
pub trait StateStore : Debug + Sync + Send {
    async fn create_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()>;
    async fn get_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent>;
    async fn find_agent_by_name(&self, ctx: &SecurityContext, name: &str, scope: &Scope) -> StoreResult<Option<Agent>>;
    async fn update_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()>;

    async fn create_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()>;
    async fn get_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session>;
    async fn update_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()>;

    async fn get_agent_state(&self, agent_id: AgentId) -> StoreResult<AgentState>;
    async fn put_agent_state(&self, agent_id: AgentId, state: &AgentState) -> StoreResult<()>;

    async fn get_session_state(&self, session_id: SessionId) -> StoreResult<SessionState>;
    async fn put_session_state(&self, session_id: SessionId, state: &SessionState) -> StoreResult<()>;
}


#[cfg(test)]
pub mod tests {
    use std::sync::Arc;
    use std::sync::RwLock;
    use uuid::Uuid;
    use super::*;
    use crate::meta::Metadata;
    use crate::security::SecurityContext;
    use crate::template::{ScopeTemplate, SessionTemplateId};
    use crate::user::UserId;

    #[derive(Debug)]
    pub struct MockTemplateStore {
        personality: Personality,
    }

    #[async_trait]
    impl TemplateStore for MockTemplateStore {
        async fn get_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<Personality> {
            let _ = ctx;
            if self.personality.id == id {
                Ok(self.personality.clone())
            } else {
                Err(StoreError::NotFound {
                    entity: "agent_template",
                    id: id.to_string(),
                })
            }
        }

        async fn find_personality_by_name(&self, ctx: &SecurityContext, _name: &str, _scope: &Scope) -> StoreResult<Personality> {
            let _ = ctx;
            Ok(self.personality.clone())
        }

        async fn list_personalities(&self, ctx: &SecurityContext) -> StoreResult<Vec<Personality>> {
            let _ = ctx;
            unimplemented!()
        }

        async fn put_personality(&self, ctx: &SecurityContext, _value: &Personality) -> StoreResult<()> {
            let _ = ctx;
            unimplemented!()
        }

        async fn delete_personality(&self, ctx: &SecurityContext, _id: PersonalityId) -> StoreResult<bool> {
            let _ = ctx;
            unimplemented!()
        }

        async fn find_session_template_by_name(&self, ctx: &SecurityContext, _name: &str, _scope: &Scope) -> StoreResult<SessionTemplate> {
            let _ = ctx;
            Ok(SessionTemplate {
                id: SessionTemplateId(Uuid::new_v4()),
                metadata: Metadata::default(),
                scope: ScopeTemplate::GlobalScope,
                channels: Vec::new(),
            })
        }

        async fn list_session_templates(&self, ctx: &SecurityContext) -> StoreResult<Vec<SessionTemplate>> {
            let _ = ctx;
            unimplemented!()
        }

        async fn put_session_template(&self, ctx: &SecurityContext, _value: &SessionTemplate) -> StoreResult<()>{
            let _ = ctx;
            unimplemented!()
        }

        async fn delete_session_template(&self, ctx: &SecurityContext, _id: SessionTemplateId) -> StoreResult<bool> {
            let _ = ctx;
            unimplemented!()
        }
    }

    #[derive(Debug, Default)]
    pub struct MockStateStore {
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

    #[async_trait]
    impl StateStore for MockStateStore {
        async fn create_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
            let _ = ctx;
            self.write_agents(|v| v.push(value.clone()));
            Ok(())
        }

        async fn get_agent(&self, ctx: &SecurityContext, _id: AgentId) -> StoreResult<Agent> {
            let _ = ctx;
            unimplemented!()
        }

        async fn find_agent_by_name(&self, ctx: &SecurityContext, name: &str, scope: &Scope) -> StoreResult<Option<Agent>> {
            let _ = ctx;
            Ok(self
                .read_agents()
                .iter()
                .find(|a| a.metadata.name == name && a.metadata.scope == *scope)
                .cloned())
        }

        async fn update_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
            let _ = ctx;
            self.write_agents(|v| {
                if let Some(existing) = v.iter_mut().find(|a| a.id == value.id) {
                    *existing = value.clone();
                }
            });
            Ok(())
        }

        async fn create_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()> {
            let _ = ctx;
            self.write_sessions(|v| v.push(value.clone()));
            Ok(())
        }

        async fn get_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session> {
            let _ = ctx;
            self.read_sessions()
                .into_iter()
                .find(|s| s.id == id)
                .ok_or(StoreError::NotFound {
                    entity: "session",
                    id: id.to_string(),
                })
        }

        async fn update_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()> {
            let _ = ctx;
            self.write_sessions(|v| {
                if let Some(existing) = v.iter_mut().find(|s| s.id == value.id) {
                    *existing = value.clone();
                }
            });
            Ok(())
        }

        async fn get_agent_state(&self, _agent_id: AgentId) -> StoreResult<AgentState> {
            unimplemented!()
        }

        async fn put_agent_state(&self, _agent_id: AgentId, _state: &AgentState) -> StoreResult<()> {
            unimplemented!()
        }

        async fn get_session_state(&self, _session_id: SessionId) -> StoreResult<SessionState> {
            unimplemented!()
        }

        async fn put_session_state(&self, _session_id: SessionId, _state: &SessionState) -> StoreResult<()> {
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

    #[tokio::test]
    async fn get_or_create_agent_returns_existing_agent() {
        let personality = personality_template();
        let existing = Agent::new("assistant".to_string(), Scope::GlobalScope, &personality);

        let template_store = Arc::new(MockTemplateStore {
            personality: personality.clone(),
        });
        let state_store = Arc::new(MockStateStore {
            agents: RwLock::new(vec![existing.clone()]),
            ..MockStateStore::default()
        });

        let context = crate::session::SessionContext {
            id: SessionId(Uuid::new_v4()),
            owner: UserId::default(),
            security: SecurityContext::default(),
            template_store: template_store.clone(),
            state_store: state_store.clone(),
        };

        let pt = crate::template::ParticipantTemplate::Agent {
            name: "assistant".to_string(),
            scope: crate::template::ScopeTemplate::GlobalScope,
            personality_id: personality.id,
        };

        let resolved = pt.instantiate(&context).await.unwrap();

        if let Participant::Agent(agent) = resolved {
            assert_eq!(agent.id, existing.id);
        } else {
            panic!("Expected agent participant");
        }
    }

    #[tokio::test]
    async fn get_or_create_agent_creates_agent_when_missing() {
        let personality = personality_template();
        let template_store = Arc::new(MockTemplateStore {
            personality: personality.clone(),
        });
        let state_store = Arc::new(MockStateStore {
            agents: RwLock::new(Vec::new()),
            ..MockStateStore::default()
        });

        let context = crate::session::SessionContext {
            id: SessionId(Uuid::new_v4()),
            owner: UserId::default(),
            security: SecurityContext::default(),
            template_store: template_store.clone(),
            state_store: state_store.clone(),
        };

        let pt = crate::template::ParticipantTemplate::Agent {
            name: "assistant".to_string(),
            scope: crate::template::ScopeTemplate::GlobalScope,
            personality_id: personality.id,
        };

        let created_participant = pt.instantiate(&context).await.unwrap();
        let created = if let Participant::Agent(agent) = created_participant {
            agent
        } else {
            panic!("Expected agent participant");
        };

        let fetched_participant = pt.instantiate(&context).await.unwrap();
        let fetched = if let Participant::Agent(agent) = fetched_participant {
            agent
        } else {
            panic!("Expected agent participant");
        };

        assert_eq!(created.id, fetched.id);
        assert_eq!(created.personality_id, personality.id);
    }

    #[tokio::test]
    async fn agent_controller_persists_changes() {
        let personality = personality_template();
        let agent = Agent::new("assistant".to_string(), Scope::GlobalScope, &personality);
        let state_store = Arc::new(MockStateStore::default());
        let ctx = SecurityContext::default();
        state_store.create_agent(&ctx, &agent).await.unwrap();

        let event_sender = Arc::new(crate::event::FnEventSender::new(|_| {}));
        let controller = crate::agent::AgentController::new(agent.clone(), event_sender, state_store.clone());

        controller.update_name("new_name".to_string()).await;

        let persisted = state_store.find_agent_by_name(&ctx, "new_name", &Scope::GlobalScope).await.unwrap().unwrap();
        assert_eq!(persisted.id, agent.id);
        assert_eq!(persisted.metadata.name, "new_name");
    }

    #[tokio::test]
    async fn session_controller_persists_new_session() {
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

        let event_sender = Arc::new(crate::event::FnEventSender::new(|_| {}));
        let _controller = crate::session::SessionController::new(session.clone(), event_sender, state_store.clone()).await.unwrap();

        let persisted = state_store.get_session(&SecurityContext::default(), session.id).await.unwrap();
        assert_eq!(persisted.id, session.id);
    }
}