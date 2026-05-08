use uuid::Uuid;

use crate::agent::{Agent, AgentState};
use crate::session::{Session, SessionState};
use crate::template::{AgentTemplate, SessionTemplate};

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
    fn get_session_template(&self, name: &str) -> StoreResult<SessionTemplate>;
    fn get_agent_template(&self, id: Uuid) -> StoreResult<AgentTemplate>;
    fn get_agent(&self, id: Uuid) -> StoreResult<Agent>;
}


pub trait TemplateStore {
    fn get_agent_template(&self, id: Uuid) -> StoreResult<AgentTemplate>;
    fn find_agent_template_by_name(&self, name: &str) -> StoreResult<AgentTemplate>;
    fn list_agent_templates(&self) -> StoreResult<Vec<AgentTemplate>>;
    fn put_agent_template(&self, value: AgentTemplate) -> StoreResult<()>;
    fn delete_agent_template(&self, id: Uuid) -> StoreResult<bool>;

    fn get_session_template(&self, name: &str) -> StoreResult<SessionTemplate>;
    fn find_session_template_by_name(&self, name: &str) -> StoreResult<SessionTemplate>;
    fn list_session_templates(&self) -> StoreResult<Vec<SessionTemplate>>;
    fn put_session_template(&self, value: SessionTemplate) -> StoreResult<()>;
    fn delete_session_template(&self, name: &str) -> StoreResult<bool>;
}


pub trait StateStore {
    fn create_agent(&self, value: Agent) -> StoreResult<()>;
    fn get_agent(&self, id: Uuid) -> StoreResult<Agent>;
    fn update_agent(&self, value: Agent) -> StoreResult<()>;

    fn create_session(&self, value: Session) -> StoreResult<()>;
    fn get_session(&self, id: Uuid) -> StoreResult<Session>;
    fn update_session(&self, value: Session) -> StoreResult<()>;

    fn get_agent_state(&self, agent_id: Uuid) -> StoreResult<AgentState>;
    fn put_agent_state(&self, agent_id: Uuid, state: AgentState) -> StoreResult<()>;

    fn get_session_state(&self, session_id: Uuid) -> StoreResult<SessionState>;
    fn put_session_state(&self, session_id: Uuid, state: SessionState) -> StoreResult<()>;
}
