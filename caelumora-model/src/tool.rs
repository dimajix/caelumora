use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub definition: ToolDefinition,
    pub execution_env: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecution {
    pub id: Uuid,
    pub tool_id: Uuid,
    pub session_id: Uuid,
    pub agent_id: Uuid,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub status: ExecutionStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecutionStatus {
    PendingApproval,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(&self, tool: &Tool, input: serde_json::Value, execution: &mut ToolExecution) -> anyhow::Result<serde_json::Value>;
}
