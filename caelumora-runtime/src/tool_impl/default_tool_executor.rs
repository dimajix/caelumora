//! The `default_tool_executor` component: runs an agent's tool calls with
//! the tools of every `ToolProvider` of the agent (issues 100, 127) - the
//! default `ToolExecutor`. It only finds the tool and runs it: the runtime's
//! `GuardedToolExecutor` hands it only calls the policy permits, and makes
//! what the model is told of the result.

use crate::agent_turn::Turn;
use crate::component::{ComponentError, ComponentFacets, ComponentImpl, ComponentKind, ComponentRegistry, ComponentResult};
use crate::tool::{ToolError, ToolExecutor, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::tool::{ToolCall, ToolSet};
use std::sync::Arc;

pub const DEFAULT_TOOL_EXECUTOR_KIND: &str = "default_tool_executor";

/// Runs the tools of an agent's components. It keeps the providers, not
/// their tools: those are asked for each time, since a provider's tools may
/// change while the agent runs (issue 100). Each provider's tools are a
/// tool set named after its component (issue 194).
#[derive(Debug)]
pub struct DefaultToolExecutor {
    providers: Vec<ComponentImpl<dyn ToolProvider>>,
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
            for tool in provider.tools() {
                if let Some((_, component)) = seen.iter().find(|(name, _)| *name == tool.name) {
                    return Err(ComponentError::InvalidConfig(format!(
                        "tool '{}' is provided by both component '{}' and component '{}'",
                        tool.name, component, provider.id
                    )));
                }
                seen.push((tool.name, provider.id.clone()));
            }
        }
        Ok(Self { providers })
    }

    /// The provider of the tool `name`, as the providers' tools are now.
    fn provider_of(&self, name: &str) -> ToolResult<&ComponentImpl<dyn ToolProvider>> {
        self.providers.iter().find(|p| p.tools().iter().any(|t| t.name == name))
            .ok_or_else(|| ToolError::UnknownTool(name.to_string()))
    }
}

#[async_trait]
impl ToolExecutor for DefaultToolExecutor {
    async fn execute(&self, call: &ToolCall, turn: &mut Turn) -> ToolResult<ToolOutput> {
        self.provider_of(&call.function_name)?.execute(&call.function_name, &call.arguments, turn).await
    }

    fn describe(&self, call: &ToolCall) -> ToolResult<String> {
        self.provider_of(&call.function_name)?.describe(&call.function_name, &call.arguments)
    }

    fn tool_sets(&self) -> Vec<ToolSet> {
        self.providers.iter()
            .map(|p| ToolSet { unavailable: p.unavailable(), ..ToolSet::new(p.id.clone(), p.description(), p.tools()) })
            .collect()
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
    use caelumora_model::tool::Tool;
    use serde_json::{json, Value};

    #[derive(Debug)]
    struct MockToolProvider;

    #[async_trait]
    impl ToolProvider for MockToolProvider {
        fn description(&self) -> String {
            "mock tools".to_string()
        }
        fn tools(&self) -> Vec<Tool> {
            vec![Tool {
                name: "test_tool".to_string(),
                description: "desc".to_string(),
                parameters: caelumora_model::tool::ToolParameters::object(Vec::<(String, caelumora_model::tool::ToolParameters)>::new(), Vec::<String>::new()),
                requires_approval: false,
                visibility: Default::default(),
                classification: None,
            }]
        }
        async fn execute(&self, _name: &str, _params: &Value, _turn: &mut Turn) -> ToolResult<ToolOutput> {
            Ok(json!({"result": "ok"}).into())
        }
    }

    fn component(id: &str) -> ComponentImpl<dyn ToolProvider> {
        ComponentImpl { id: id.to_string(), fail_policy: Default::default(), facet: Arc::new(MockToolProvider) }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall { id: Some("call_1".to_string()), function_name: name.to_string(), arguments: json!({}), visibility: Default::default() }
    }

    fn agent_state() -> AgentChannelState {
        let mut agent_channel_state = AgentChannelState::default();
        agent_channel_state.turn = Some(caelumora_model::turn::TurnState { work: caelumora_model::work::Work { security_context: SecurityContext { caller_id: ActorId::Agent(AgentId::new()), ..SecurityContext::default() }, ..Default::default() }, ..Default::default() });
        agent_channel_state
    }

    #[tokio::test]
    async fn a_call_runs_with_the_tool_that_provides_it_and_an_unknown_one_is_reported() {
        let executor = DefaultToolExecutor::new(vec![component("tools")]).unwrap();
        let mut agent_channel_state = agent_state();
        let mut context = Turn::of(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent_state::AgentStateHandle::detached()).expect("a turn is under way");

        let output = executor.execute(&call("test_tool"), &mut context).await.unwrap();
        assert_eq!(output.value, json!({"result": "ok"}));
        let unknown = executor.execute(&call("does_not_exist"), &mut context).await;
        assert!(matches!(unknown, Err(ToolError::UnknownTool(name)) if name == "does_not_exist"));
    }

    /// A provider whose source cannot be reached.
    #[derive(Debug)]
    struct Unreachable;

    #[async_trait]
    impl ToolProvider for Unreachable {
        fn description(&self) -> String {
            "remote tools".to_string()
        }
        fn tools(&self) -> Vec<Tool> {
            Vec::new()
        }
        fn unavailable(&self) -> Option<String> {
            Some("its server cannot be reached".to_string())
        }
        async fn execute(&self, name: &str, _params: &Value, _turn: &mut Turn) -> ToolResult<ToolOutput> {
            Err(ToolError::UnknownTool(name.to_string()))
        }
    }

    #[test]
    fn each_provider_s_tools_are_a_set_named_after_its_component() {
        let remote = ComponentImpl { id: "remote".to_string(), fail_policy: Default::default(), facet: Arc::new(Unreachable) as Arc<dyn ToolProvider> };
        let executor = DefaultToolExecutor::new(vec![component("local"), remote]).unwrap();
        let sets: Vec<_> = executor.tool_sets().into_iter()
            .map(|set| (set.name, set.description, set.tools.into_iter().map(|t| t.name).collect::<Vec<_>>(), set.unavailable))
            .collect();
        assert_eq!(sets, [
            ("local".to_string(), "mock tools".to_string(), vec!["test_tool".to_string()], None),
            ("remote".to_string(), "remote tools".to_string(), Vec::new(), Some("its server cannot be reached".to_string())),
        ], "in the order of the configuration, an unavailable one included");
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
