use uuid::Uuid;

use crate::agent::{Agent, AgentState, Personality};
use crate::meta::Scope;
use crate::session::{Session, SessionContext, SessionState};
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
    fn update_agent(&self, value: Agent) -> StoreResult<()>;

    fn create_session(&self, value: Session) -> StoreResult<()>;
    fn get_session(&self, id: Uuid) -> StoreResult<Session>;
    fn update_session(&self, value: Session) -> StoreResult<()>;

    fn get_agent_state(&self, agent_id: Uuid) -> StoreResult<AgentState>;
    fn put_agent_state(&self, agent_id: Uuid, state: AgentState) -> StoreResult<()>;

    fn get_session_state(&self, session_id: Uuid) -> StoreResult<SessionState>;
    fn put_session_state(&self, session_id: Uuid, state: SessionState) -> StoreResult<()>;
}
