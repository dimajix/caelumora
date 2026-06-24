use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::agent::AgentId;
use caelumora_model::message::{ToolCall, ToolResult};
use caelumora_model::security::SecurityContext;
use caelumora_model::session::SessionId;
use caelumora_model::tool::Tool;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Debug;
use std::sync::Arc;
use uuid::Uuid;

#[async_trait]
pub trait ToolProvider: Send + Sync + Debug {
    /// Returns a list of tools provided by this component.
    fn tools(&self) -> Vec<Tool>;

    /// Executes a specific tool by name.
    async fn execute(&self, name: &str, params: Value, security: &SecurityContext) -> Result<Value>;
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecution {
    pub id: Uuid,
    pub session_id: SessionId,
    pub agent_id: AgentId,
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


#[derive(Debug)]
pub struct ToolExecutor {
    tool_providers: Vec<Arc<dyn ToolProvider>>,
}

impl ToolExecutor {
    pub fn new(
        tool_providers: Vec<Arc<dyn ToolProvider>>,
    ) -> Self {
        Self {
            tool_providers,
        }
    }

    pub async fn execute(&self, call: &ToolCall, security: &SecurityContext) -> Result<ToolResult> {
        let tool_name = &call.function_name;
        for provider in &self.tool_providers {
            let tools = provider.tools();
            if tools.iter().any(|t| t.name == *tool_name) {
                let mut execution = ToolExecution {
                    id: uuid::Uuid::new_v4(),
                    session_id: security.session_id,
                    agent_id: security.sender_id.as_agent_id().unwrap(),
                    input: serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null),
                    output: None,
                    status: ExecutionStatus::Running,
                    started_at: chrono::Utc::now(),
                    finished_at: None,
                };

                let params = serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
                return match provider.execute(tool_name, params, security).await {
                    Ok(value) => {
                        execution.output = Some(value.clone());
                        execution.status = ExecutionStatus::Completed;
                        execution.finished_at = Some(chrono::Utc::now());
                        Ok(ToolResult {
                            id: call.id.clone(),
                            tool_name: call.function_name.clone(),
                            content: value.to_string(),
                        })
                    }
                    Err(e) => {
                        execution.status = ExecutionStatus::Failed;
                        execution.finished_at = Some(chrono::Utc::now());
                        // TODO: Log error in execution
                        Err(e)
                    }
                }
            }
        }
        Err(anyhow::anyhow!("Tool not found: {}", tool_name))
    }

    pub fn tool_providers(&self) -> Vec<Arc<dyn ToolProvider>> {
        self.tool_providers.clone()
    }

    pub fn tools(&self) -> Vec<Tool> {
        self.tool_providers
            .iter()
            .flat_map(|p| p.tools())
            .collect()
    }
}
