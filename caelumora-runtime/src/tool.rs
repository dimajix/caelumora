use crate::actor::TurnContext;
use crate::component::{ComponentError, ComponentImpl, ComponentResult};
use async_trait::async_trait;
use caelumora_model::agent::AgentId;
use caelumora_model::security::{ActionType, SecurityManager};
use caelumora_model::session::SessionId;
use caelumora_model::tool::{Tool, ToolCall, ToolCallResult, ToolResultStatus};
use chrono::{DateTime, Utc};
use tracing::{debug, info};
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

/// What the model is given as the result of a tool call that produced none
/// (issue 15): a small JSON object with the `status` and a `message` that
/// says what happened and what to do next, e.g.
/// `{"status":"denied","message":"The user declined ..."}`. Successful
/// results are passed on unchanged.
pub fn unsuccessful_result(status: ToolResultStatus, message: &str) -> String {
    serde_json::json!({ "status": status, "message": message }).to_string()
}

/// The result for a call a user declined.
pub fn denied_result() -> String {
    unsuccessful_result(
        ToolResultStatus::Denied,
        "The user declined this tool call. Do not repeat it; continue without it or ask the user how to proceed.",
    )
}

/// The result for a call nobody approved in time.
pub fn timed_out_result() -> String {
    unsuccessful_result(
        ToolResultStatus::TimedOut,
        "No one approved this tool call in time; it was not executed. You may ask the user whether to try again.",
    )
}

/// The result for a call the policy does not permit. Deliberately without the
/// reason: telling the model why describes the policy to something that may
/// try to route around it. The reason is in the log.
pub fn forbidden_result() -> String {
    unsuccessful_result(
        ToolResultStatus::Forbidden,
        "This tool call is not permitted. Do not retry it; continue without it or tell the user that you cannot do this.",
    )
}

impl ToolError {
    /// The status and the model-facing result for a call that failed with
    /// this error. `available` names the tools that do exist, offered when
    /// the model called one that does not.
    pub fn to_result(&self, available: &[String]) -> (ToolResultStatus, String) {
        let (status, message) = match self {
            ToolError::PermissionDenied(_) => return (ToolResultStatus::Forbidden, forbidden_result()),
            ToolError::UnknownTool(name) => (
                ToolResultStatus::UnknownTool,
                format!("There is no tool named '{}'. Available tools: {}.", name, if available.is_empty() { "none".to_string() } else { available.join(", ") }),
            ),
            ToolError::InvalidParameter(reason) => (
                ToolResultStatus::InvalidArguments,
                format!("The arguments were invalid: {}. Correct them and call the tool again.", reason),
            ),
            ToolError::ExecutionFailed(reason) => (
                ToolResultStatus::Failed,
                format!("The tool failed: {}", reason),
            ),
        };
        (status, unsuccessful_result(status, &message))
    }
}

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
    /// Whether the policy permits `call` in `context`. Checked before asking
    /// anyone for approval, so that users are not asked about calls that
    /// would be refused anyway; `execute` checks again.
    async fn is_permitted(&self, call: &ToolCall, context: &TurnContext) -> bool;
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
    /// An executor for the tools `providers` contribute.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if two of them provide a tool of the
    /// same name: that is a mistake in the configuration, and resolving it
    /// by order would make one of them silently unreachable (issue 37) - the
    /// same rule as for user commands.
    pub fn new(
        security_manager: Arc<dyn SecurityManager>,
        providers: Vec<ComponentImpl<dyn ToolProvider>>,
    ) -> ComponentResult<Self> {
        let mut seen: Vec<(String, String)> = Vec::new();
        for provider in &providers {
            for tool in provider.facet.tools() {
                if let Some((_, component)) = seen.iter().find(|(name, _)| *name == tool.name) {
                    return Err(ComponentError::InvalidConfig(format!(
                        "tool '{}' is provided by both component '{}' and component '{}'",
                        tool.name, component, provider.id
                    )));
                }
                seen.push((tool.name, provider.id.clone()));
            }
        }
        Ok(Self {
            security_manager,
            tool_providers: providers.into_iter().map(|p| p.facet).collect(),
        })
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
            info!("Tool call '{}' by agent {} forbidden by the security manager", call.function_name, agent_id);
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
                            status: ToolResultStatus::Ok,
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

    async fn is_permitted(&self, call: &ToolCall, context: &TurnContext) -> bool {
        self.security_manager.check(context.security_context(), ActionType::CallTool(call)).await
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

    #[derive(Debug)]
    struct DenyAllSecurityManager;

    #[async_trait]
    impl SecurityManager for DenyAllSecurityManager {
        async fn check(&self, _context: &caelumora_model::security::SecurityContext, _action: ActionType<'_>) -> bool {
            false
        }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall { id: Some("call_1".to_string()), function_name: name.to_string(), arguments: json!({}) }
    }

    fn parse(result: &str) -> Value {
        serde_json::from_str(result).expect("an unsuccessful result is a JSON object")
    }

    #[test]
    fn each_error_maps_to_its_own_status_with_a_next_step() {
        let available = vec!["calculator".to_string(), "echo".to_string()];
        let cases = [
            (ToolError::UnknownTool("shell".to_string()), ToolResultStatus::UnknownTool, "Available tools: calculator, echo"),
            (ToolError::InvalidParameter("'x' is missing".to_string()), ToolResultStatus::InvalidArguments, "call the tool again"),
            (ToolError::ExecutionFailed("disk full".to_string()), ToolResultStatus::Failed, "disk full"),
            (ToolError::PermissionDenied("rule 7: no shell after 18:00".to_string()), ToolResultStatus::Forbidden, "not permitted"),
        ];
        for (error, expected, fragment) in cases {
            let (status, result) = error.to_result(&available);
            assert_eq!(status, expected);
            let json = parse(&result);
            assert_eq!(json["status"], serde_json::to_value(expected).unwrap());
            let message = json["message"].as_str().unwrap();
            assert!(message.contains(fragment), "{:?}: {}", expected, message);
        }
    }

    #[test]
    fn a_policy_refusal_does_not_disclose_the_policy() {
        let (_, result) = ToolError::PermissionDenied("rule 7: no shell after 18:00".to_string()).to_result(&[]);
        assert!(!result.contains("rule 7"), "{}", result);
        assert_eq!(result, forbidden_result());
    }

    #[test]
    fn denial_and_timeout_are_distinguishable() {
        assert_eq!(parse(&denied_result())["status"], "denied");
        assert_eq!(parse(&timed_out_result())["status"], "timed_out");
        assert!(denied_result().contains("Do not repeat it"));
        assert!(timed_out_result().contains("ask the user whether to try again"));
    }

    #[tokio::test]
    async fn the_policy_is_checked_before_approval_and_again_on_execution() {
        let provider = Arc::new(MockToolProvider { requires_approval: true });
        let executor = DefaultToolExecutor::new(Arc::new(DenyAllSecurityManager), vec![component("tools", provider)]).unwrap();

        let mut agent_state = AgentState::default();
        let mut agent_channel_state = AgentChannelState::default();
        let agent_id = AgentId::new();
        agent_channel_state.turn_security_context.caller_id = ActorId::Agent(agent_id);
        let mut context = TurnContext::new(agent_id, "agent".to_string(), &mut agent_state, &mut agent_channel_state);

        assert!(!executor.is_permitted(&call("test_tool"), &context).await);
        let error = executor.execute(&call("test_tool"), &mut context).await.unwrap_err();
        assert!(matches!(error, ToolError::PermissionDenied(_)));
    }

    #[tokio::test]
    async fn an_unknown_tool_is_reported_as_such() {
        let executor = DefaultToolExecutor::new(Arc::new(DummySecurityManager::new()), vec![component("tools", Arc::new(MockToolProvider { requires_approval: false }))]).unwrap();
        let mut agent_state = AgentState::default();
        let mut agent_channel_state = AgentChannelState::default();
        agent_channel_state.turn_security_context.caller_id = ActorId::Agent(AgentId::new());
        let mut context = TurnContext::new(AgentId::new(), "agent".to_string(), &mut agent_state, &mut agent_channel_state);

        let error = executor.execute(&call("does_not_exist"), &mut context).await.unwrap_err();
        assert!(matches!(error, ToolError::UnknownTool(ref name) if name == "does_not_exist"));
    }

    fn component(id: &str, facet: Arc<dyn ToolProvider>) -> ComponentImpl<dyn ToolProvider> {
        ComponentImpl { id: id.to_string(), fail_policy: Default::default(), facet }
    }

    #[test]
    fn two_components_with_a_tool_of_the_same_name_are_a_configuration_error() {
        let security = Arc::new(DummySecurityManager::new());
        let same = || Arc::new(MockToolProvider { requires_approval: false });
        let error = DefaultToolExecutor::new(security.clone(), vec![component("local", same()), component("remote", same())])
            .expect_err("the second test_tool would be unreachable");
        let message = error.to_string();
        assert!(message.contains("'test_tool'") && message.contains("'local'") && message.contains("'remote'"), "{message}");

        DefaultToolExecutor::new(security, vec![component("local", same())]).expect("one provider is fine");
    }

    #[test]
    fn stored_results_without_a_status_read_as_ok() {
        let result: ToolCallResult = serde_json::from_value(json!({"id": "1", "tool_name": "echo", "content": "hi"})).unwrap();
        assert_eq!(result.status, ToolResultStatus::Ok);
        assert!(!serde_json::to_string(&result).unwrap().contains("status"), "Ok is not written");

        let denied = ToolCallResult { status: ToolResultStatus::Denied, ..result };
        assert!(serde_json::to_string(&denied).unwrap().contains("\"status\":\"denied\""));
    }

    #[tokio::test]
    async fn test_tool_executor() {
        let security_manager = Arc::new(DummySecurityManager::new());
        let provider = Arc::new(MockToolProvider { requires_approval: false });
        let executor = DefaultToolExecutor::new(security_manager, vec![component("tools", provider)]).unwrap();

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
