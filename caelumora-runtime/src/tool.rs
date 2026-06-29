use crate::message::TurnContext;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::agent::AgentId;
use caelumora_model::message::{ToolCall, ToolResult};
use caelumora_model::security::{ActionType, SecurityManager};
use caelumora_model::session::SessionId;
use caelumora_model::tool::Tool;
use chrono::{DateTime, Utc};
use log::info;
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
    async fn execute(&self, name: &str, params: Value, context: &TurnContext) -> Result<Value>;
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecution {
    pub id: Uuid,
    pub session_id: SessionId,
    pub agent_id: AgentId,
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
pub trait ToolExecutor: Send + Sync + Debug {
    async fn execute(&self, call: &ToolCall, security: &TurnContext) -> Result<ToolResult>;
    fn providers(&self) -> Vec<Arc<dyn ToolProvider>>;
    fn tools(&self) -> Vec<Tool>;
}

#[derive(Debug)]
pub struct DefaultToolExecutor {
    security_manager: Arc<dyn SecurityManager>,
    tool_providers: Vec<Arc<dyn ToolProvider>>,
}

impl DefaultToolExecutor {
    pub fn new(
        security_manager: Arc<dyn SecurityManager>,
        tool_providers: Vec<Arc<dyn ToolProvider>>,
    ) -> Self {
        Self {
            security_manager,
            tool_providers,
        }
    }
}

#[async_trait]
impl ToolExecutor for DefaultToolExecutor {
    async fn execute(&self, call: &ToolCall, context: &TurnContext) -> Result<ToolResult> {
        let security = &context.security_context;
        info!(
            "Executing tool '{}' (initiator: {:?}, sender: {:?}, session: {}, channel: {})",
            call.function_name,
            security.initiator_id,
            security.sender_id,
            security.session_id,
            security.channel_id
        );

        let mut _execution = ToolExecution {
            id: Uuid::new_v4(),
            session_id: security.session_id,
            agent_id: security.sender_id.as_agent_id().unwrap(),
            status: ExecutionStatus::PendingApproval,
            started_at: Utc::now(),
            finished_at: None,
        };

        if !self.security_manager.check(security, ActionType::CallTool(call)).await {
            return Err(anyhow::anyhow!("Security check failed: Access denied to tool '{}'", call.function_name));
        }

        let tool_name = &call.function_name;
        for provider in &self.tool_providers {
            let tools = provider.tools();
            if tools.iter().any(|t| t.name == *tool_name) {
                _execution.status = ExecutionStatus::Running;
                let params = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
                return match provider.execute(tool_name, params, context).await {
                    Ok(value) => {
                        _execution.status = ExecutionStatus::Completed;
                        _execution.finished_at = Some(Utc::now());
                        Ok(ToolResult {
                            id: call.id.clone(),
                            tool_name: call.function_name.clone(),
                            content: value.to_string(),
                        })
                    }
                    Err(e) => {
                        _execution.status = ExecutionStatus::Failed;
                        _execution.finished_at = Some(Utc::now());
                        // TODO: Log error in execution
                        Err(e)
                    }
                }
            }
        }
        Err(anyhow::anyhow!("Tool not found: {}", tool_name))
    }

    fn providers(&self) -> Vec<Arc<dyn ToolProvider>> {
        self.tool_providers.clone()
    }

    fn tools(&self) -> Vec<Tool> {
        self.tool_providers
            .iter()
            .flat_map(|p| p.tools())
            .collect()
    }
}
