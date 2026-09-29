//! The `default_tool_executor` component: runs an agent's tool calls with
//! the tools of every `ToolProvider` of the agent (issues 100, 127) - the
//! default `ToolExecutor`. It checks no permissions: the runtime's
//! `GuardedToolExecutor` hands it only calls the policy permits.

use crate::agent_turn::Turn;
use crate::component::{ComponentError, ComponentFacets, ComponentImpl, ComponentKind, ComponentRegistry, ComponentResult};
use crate::tool::{successful_result, ToolError, ToolExecutor, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::tool::{Tool, ToolCall, ToolCallResult, ToolResultStatus};
use chrono::Utc;
use std::sync::Arc;
use tracing::{debug, warn};

pub const DEFAULT_TOOL_EXECUTOR_KIND: &str = "default_tool_executor";

/// Runs the tools of an agent's components. It keeps the providers, not
/// their tools: those are asked for each time, since a provider's tools may
/// change while the agent runs (issue 100).
#[derive(Debug)]
pub struct DefaultToolExecutor {
    providers: Vec<Arc<dyn ToolProvider>>,
}

impl DefaultToolExecutor {
    /// An executor for the tools `providers` contribute.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if two of them provide a tool of the
    /// same name: that is a mistake in the configuration, and resolving it
    /// by order would make one of them silently unreachable (issue 37) - the
    /// same rule as for user commands.
    pub fn new(providers: Vec<ComponentImpl<dyn ToolProvider>>) -> ComponentResult<Self> {
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
        Ok(Self { providers: providers.into_iter().map(|p| p.facet).collect() })
    }

    /// The provider of the tool `name`, as the providers' tools are now.
    fn provider_of(&self, name: &str) -> ToolResult<&Arc<dyn ToolProvider>> {
        self.providers.iter().find(|p| p.tools().iter().any(|t| t.name == name))
            .ok_or_else(|| ToolError::UnknownTool(name.to_string()))
    }
}

#[async_trait]
impl ToolExecutor for DefaultToolExecutor {
    async fn execute(&self, call: &ToolCall, turn: &mut Turn) -> ToolCallResult {
        // Only the tool's own run is timed, not the lookup before it: a call
        // that never ran has no timing.
        let (outcome, ran) = match self.provider_of(&call.function_name) {
            Err(e) => (Err(e), None),
            Ok(provider) => {
                let started_at = Utc::now();
                let outcome = provider.execute(&call.function_name, &call.arguments, turn).await;
                let finished_at = Utc::now();
                // Session, channel and agent are on the turn's span.
                debug!(tool = %call.function_name, duration_ms = (finished_at - started_at).num_milliseconds(), ok = outcome.is_ok(), "Tool ran");
                (outcome, Some((started_at, finished_at)))
            }
        };
        let mut result = match outcome {
            Ok(value) => ToolCallResult::of(call, ToolResultStatus::Ok, successful_result(value, call.confidentiality, turn.publication())),
            Err(e) => {
                warn!("Tool call '{}' failed: {}", call.function_name, e);
                let available: Vec<String> = self.tools().into_iter().map(|t| t.name).collect();
                let (status, content) = e.to_result(&available);
                ToolCallResult::of(call, status, content)
            }
        };
        result.started_at = ran.map(|(started_at, _)| started_at);
        result.finished_at = ran.map(|(_, finished_at)| finished_at);
        result
    }

    fn describe(&self, call: &ToolCall) -> ToolResult<String> {
        self.provider_of(&call.function_name)?.describe(&call.function_name, &call.arguments)
    }

    fn tools(&self) -> Vec<Tool> {
        self.providers.iter().flat_map(|p| p.tools()).collect()
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        ComponentKind::new(DEFAULT_TOOL_EXECUTOR_KIND).provides::<dyn ToolExecutor>(),
        |_descriptor, context| async move {
            // Every tool of the agent, in the order of its configuration.
            let providers = context.all::<dyn ToolProvider>().await?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolExecutor>(Arc::new(DefaultToolExecutor::new(providers)?));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::security::SecurityContext;
    use serde_json::{json, Value};

    #[derive(Debug)]
    struct MockToolProvider;

    #[async_trait]
    impl ToolProvider for MockToolProvider {
        fn tools(&self) -> Vec<Tool> {
            vec![Tool {
                name: "test_tool".to_string(),
                description: "desc".to_string(),
                parameters: caelumora_model::tool::ToolParameters::object(Vec::<(String, caelumora_model::tool::ToolParameters)>::new(), Vec::<String>::new()),
                requires_approval: false,
                confidentiality: Default::default(),
            }]
        }
        async fn execute(&self, _name: &str, _params: &Value, _context: &mut Turn) -> ToolResult<Value> {
            Ok(json!({"result": "ok"}))
        }
    }

    fn component(id: &str) -> ComponentImpl<dyn ToolProvider> {
        ComponentImpl { id: id.to_string(), fail_policy: Default::default(), facet: Arc::new(MockToolProvider) }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall { id: Some("call_1".to_string()), function_name: name.to_string(), arguments: json!({}), confidentiality: Default::default() }
    }

    fn agent_state() -> AgentChannelState {
        let mut agent_channel_state = AgentChannelState::default();
        agent_channel_state.turn = Some(caelumora_model::agent::TurnState { security_context: SecurityContext { caller_id: ActorId::Agent(AgentId::new()), ..SecurityContext::default() }, ..Default::default() });
        agent_channel_state
    }

    #[tokio::test]
    async fn a_call_runs_and_records_when() {
        let executor = DefaultToolExecutor::new(vec![component("tools")]).unwrap();
        let mut agent_channel_state = agent_state();
        let mut context = Turn::new(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached());

        let result = executor.execute(&call("test_tool"), &mut context).await;
        assert_eq!(result.status, ToolResultStatus::Ok);
        assert_eq!(result.content, "{\"result\":\"ok\"}");
        let (started_at, finished_at) = (result.started_at.expect("a call that ran has its start"), result.finished_at.expect("and its end"));
        assert!(finished_at >= started_at);
        assert!(serde_json::to_string(&result).unwrap().contains("\"started_at\""), "and it is stored with the history");
    }

    #[tokio::test]
    async fn an_unknown_tool_is_reported_as_such() {
        let executor = DefaultToolExecutor::new(vec![component("tools")]).unwrap();
        let mut agent_channel_state = agent_state();
        let mut context = Turn::new(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached());

        let result = executor.execute(&call("does_not_exist"), &mut context).await;
        assert_eq!(result.status, ToolResultStatus::UnknownTool);
        assert!(result.content.contains("Available tools: test_tool"), "{}", result.content);
        assert_eq!((result.id.as_deref(), result.tool_name.as_str()), (Some("call_1"), "does_not_exist"), "the result is the call's");
        assert!(result.started_at.is_none(), "it never ran");
    }

    #[test]
    fn two_components_with_a_tool_of_the_same_name_are_a_configuration_error() {
        let error = DefaultToolExecutor::new(vec![component("local"), component("remote")])
            .expect_err("the second test_tool would be unreachable");
        let message = error.to_string();
        assert!(message.contains("'test_tool'") && message.contains("'local'") && message.contains("'remote'"), "{message}");

        DefaultToolExecutor::new(vec![component("local")]).expect("one provider is fine");
    }
}
