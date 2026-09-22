use crate::actor::TurnContext;
use async_trait::async_trait;
use caelumora_model::agent::AgentId;
use caelumora_model::security::{ActionType, SecurityManager};
use caelumora_model::session::SessionId;
use caelumora_model::tool::{Tool, ToolCall, ToolCallResult};
use chrono::{DateTime, Utc};
use tracing::debug;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Debug;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("invalid parameter: {0}")]
    InvalidParameter(String),
    #[error("tool execution failed: {0}")]
    ExecutionFailed(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
}

pub type ToolResult<T> = Result<T, ToolError>;

#[async_trait]
pub trait ToolProvider: Send + Sync + Debug {
    /// Returns a list of tools provided by this component.
    fn tools(&self) -> Vec<Tool>;

    /// Executes a specific tool by name.
    async fn execute(&self, name: &str, params: &Value, context: &mut TurnContext) -> ToolResult<Value>;
    /// Provides a textual description for a specific tool call
    fn describe(&self,  name: &str, _params: &Value) -> ToolResult<String> {
        let tools = self.tools();
        if let Some(tool) = tools.iter().find(|t| t.name == name) {
            Ok(tool.description.to_string())
        }
        else {
            Err(ToolError::UnknownTool(name.to_string()))
        }
    }
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
    async fn execute(&self, call: &ToolCall, context: &mut TurnContext) -> ToolResult<ToolCallResult>;
    fn describe(&self, call: &ToolCall) -> ToolResult<String>;
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
    async fn execute(&self, call: &ToolCall, context: &mut TurnContext) -> ToolResult<ToolCallResult> {
        let security = &context.security_context();
        debug!(
            "Executing tool '{}' (initiator: {:?}, sender: {:?}, session: {}, channel: {})",
            call.function_name,
            security.initiator_id,
            security.caller_id,
            security.session_id,
            security.channel_id
        );

        let agent_id = security.caller_id.as_agent_id()
            .ok_or_else(|| ToolError::PermissionDenied(format!("tools can only be called by agents, not by {}", security.caller_id)))?;
        let mut _execution = ToolExecution {
            id: Uuid::new_v4(),
            session_id: security.session_id,
            agent_id,
            status: ExecutionStatus::PendingApproval,
            started_at: Utc::now(),
            finished_at: None,
        };

        if !self.security_manager.check(security, ActionType::CallTool(call)).await {
            return Err(ToolError::PermissionDenied(call.function_name.clone()));
        }

        let tool_name = &call.function_name;
        for provider in &self.tool_providers {
            let tools = provider.tools();
            if let Some(_tool) = tools.iter().find(|t| t.name == *tool_name) {
                _execution.status = ExecutionStatus::Running;
                return match provider.execute(tool_name, &call.arguments, context).await {
                    Ok(value) => {
                        _execution.status = ExecutionStatus::Completed;
                        _execution.finished_at = Some(Utc::now());
                        Ok(ToolCallResult {
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
        Err(ToolError::UnknownTool(tool_name.clone()))
    }

    fn describe(&self, call: &ToolCall) -> ToolResult<String> {
        let tool_name = &call.function_name;
        for provider in &self.tool_providers {
            let tools = provider.tools();
            if let Some(_tool) = tools.iter().find(|t| t.name == *tool_name) {
                return provider.describe(tool_name, &call.arguments)
            }
        }

        Err(ToolError::UnknownTool(tool_name.clone()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::TurnContext;
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::agent::AgentState;
    use caelumora_model::security::DummySecurityManager;
    use serde_json::json;

    #[derive(Debug)]
    struct MockToolProvider {
        requires_approval: bool,
    }

    #[async_trait]
    impl ToolProvider for MockToolProvider {
        fn tools(&self) -> Vec<Tool> {
            vec![Tool {
                name: "test_tool".to_string(),
                description: "desc".to_string(),
                parameters: caelumora_model::tool::ToolParameters::object(Vec::<(String, caelumora_model::tool::ToolParameters)>::new(), Vec::<String>::new()),
                requires_approval: self.requires_approval,
            }]
        }
        async fn execute(&self, _name: &str, _params: &Value, _context: &mut TurnContext) -> ToolResult<Value> {
            Ok(json!({"result": "ok"}))
        }
    }

    #[tokio::test]
    async fn test_tool_executor() {
        let security_manager = Arc::new(DummySecurityManager::new());
        let provider = Arc::new(MockToolProvider { requires_approval: false });
        let executor = DefaultToolExecutor::new(security_manager, vec![provider]);

        let call = ToolCall {
            id: Some("call_1".to_string()),
            function_name: "test_tool".to_string(),
            arguments: json!("{}".to_string()),
        };

        let mut agent_state = AgentState::default();
        let mut agent_channel_state = AgentChannelState::default();
        let agent_id = AgentId::new();
        agent_channel_state.turn_security_context.caller_id = ActorId::Agent(agent_id);
        let mut context = TurnContext::new(agent_id, "agent".to_string(), &mut agent_state, &mut agent_channel_state);

        let result = executor.execute(&call, &mut context).await.unwrap();
        assert_eq!(result.content, "{\"result\":\"ok\"}");
    }
}
