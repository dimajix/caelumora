use async_trait::async_trait;
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

#[async_trait]
pub trait TemplateStore: Send + Sync {
    async fn put_agent_template(&self, value: AgentTemplate) -> StoreResult<()>;
    async fn get_agent_template(&self, id: Uuid) -> StoreResult<AgentTemplate>;
    async fn find_agent_template_by_name(&self, name: &str) -> StoreResult<AgentTemplate>;
    async fn list_agent_templates(&self) -> StoreResult<Vec<AgentTemplate>>;
    async fn delete_agent_template(&self, id: Uuid) -> StoreResult<bool>;

    async fn put_session_template(&self, value: SessionTemplate) -> StoreResult<()>;
    async fn get_session_template(&self, name: &str) -> StoreResult<SessionTemplate>;
    async fn find_session_template_by_name(&self, name: &str) -> StoreResult<SessionTemplate>;
    async fn list_session_templates(&self) -> StoreResult<Vec<SessionTemplate>>;
    async fn delete_session_template(&self, name: &str) -> StoreResult<bool>;
}

#[async_trait]
pub trait StateStore: Send + Sync {
    async fn create_agent(&self, value: Agent) -> StoreResult<()>;
    async fn get_agent(&self, id: Uuid) -> StoreResult<Agent>;
    async fn update_agent(&self, value: Agent) -> StoreResult<()>;

    async fn create_session(&self, value: Session) -> StoreResult<()>;
    async fn get_session(&self, id: Uuid) -> StoreResult<Session>;
    async fn update_session(&self, value: Session) -> StoreResult<()>;

    async fn get_agent_state(&self, agent_id: Uuid) -> StoreResult<AgentState>;
    async fn put_agent_state(&self, agent_id: Uuid, state: AgentState) -> StoreResult<()>;

    async fn get_session_state(&self, session_id: Uuid) -> StoreResult<SessionState>;
    async fn put_session_state(&self, session_id: Uuid, state: SessionState) -> StoreResult<()>;
}
