//! The `default_tool_executor` component: runs an agent's tool calls with
//! the tools of every `ToolProvider` of the agent (issues 100, 127) - the
//! default `ToolExecutor`. It only finds the tool and runs it: the runtime's
//! `GuardedToolExecutor` hands it only calls the policy permits, and makes
//! what the model is told of the result.

use crate::agent::turn::Turn;
use crate::component::{ComponentError, ComponentFacets, ComponentImpl, ComponentKind, ComponentRegistry, ComponentResult};
use crate::tool::{ToolError, ToolExecutor, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::approval::ToolMatcher;
use caelumora_model::question::{Answers, Questions};
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
    async fn execute(&self, call: &ToolCall, answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput> {
        self.provider_of(&call.function_name)?.execute(&call.function_name, &call.arguments, answers, turn).await
    }

    fn describe(&self, call: &ToolCall) -> ToolResult<String> {
        self.provider_of(&call.function_name)?.describe(&call.function_name, &call.arguments)
    }

    fn questions(&self, call: &ToolCall) -> ToolResult<Option<Questions>> {
        self.provider_of(&call.function_name)?.questions(&call.function_name, &call.arguments)
    }

    fn matcher(&self, call: &ToolCall) -> ToolMatcher {
        match self.provider_of(&call.function_name) {
            Ok(provider) => provider.matcher(&call.function_name, &call.arguments),
            Err(_) => ToolMatcher::exact(call),
        }
    }

    fn tool_sets(&self) -> Vec<ToolSet> {
        self.providers.iter()
            .map(|p| ToolSet { unavailable: p.unavailable(), destination: p.destination(), ..ToolSet::new(p.id.clone(), p.description(), p.tools()) })
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
                presentation: Default::default(),
            }]
        }
        async fn execute(&self, _name: &str, _params: &Value, _answers: Option<&Answers>, _turn: &mut Turn) -> ToolResult<ToolOutput> {
            Ok(json!({"result": "ok"}).into())
        }
    }

    fn component(id: &str) -> ComponentImpl<dyn ToolProvider> {
        ComponentImpl { id: id.to_string(), fail_policy: Default::default(), facet: Arc::new(MockToolProvider) }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall { id: Some("call_1".to_string()), function_name: name.to_string(), arguments: json!({}), visibility: Default::default(), presentation: Default::default() }
    }

    fn agent_state() -> AgentChannelState {
        AgentChannelState { turn: Some(caelumora_model::turn::TurnState { work: caelumora_model::work::Work { security_context: SecurityContext { caller_id: ActorId::Agent(AgentId::new()), ..SecurityContext::default() }, ..Default::default() }, ..Default::default() }), ..Default::default() }
    }

    #[tokio::test]
    async fn a_call_runs_with_the_tool_that_provides_it_and_an_unknown_one_is_reported() {
        let executor = DefaultToolExecutor::new(vec![component("tools")]).unwrap();
        let mut agent_channel_state = agent_state();
        let mut context = Turn::of(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached()).expect("a turn is under way");

        let output = executor.execute(&call("test_tool"), None, &mut context).await.unwrap();
        assert_eq!(output.value, json!({"result": "ok"}));
        let unknown = executor.execute(&call("does_not_exist"), None, &mut context).await;
        assert!(matches!(unknown, Err(ToolError::UnknownTool(name)) if name == "does_not_exist"));
    }

    /// A provider whose tool asks a question and says what it was answered.
    #[derive(Debug)]
    struct Asking;

    #[async_trait]
    impl ToolProvider for Asking {
        fn description(&self) -> String {
            "asking tools".to_string()
        }
        fn tools(&self) -> Vec<Tool> {
            vec![Tool { name: "asking_tool".to_string(), ..MockToolProvider.tools().remove(0) }]
        }
        async fn execute(&self, _name: &str, _params: &Value, answers: Option<&Answers>, _turn: &mut Turn) -> ToolResult<ToolOutput> {
            Ok(json!({"answered_by": answers.map(|a| a.answered_by_name.clone())}).into())
        }
        fn questions(&self, _name: &str, _params: &Value) -> ToolResult<Option<Questions>> {
            Ok(Some(Questions {
                questions: vec![caelumora_model::question::Question { header: "Name".to_string(), question: "Who?".to_string(), choices: Vec::new(), multi_select: false }],
                ask: vec![caelumora_model::question::Addressee::Owner],
                sensitive: false,
            }))
        }
    }

    #[tokio::test]
    async fn the_questions_of_a_call_are_its_tool_s_and_the_answers_go_to_it() {
        let asking = ComponentImpl { id: "asking".to_string(), fail_policy: Default::default(), facet: Arc::new(Asking) as Arc<dyn ToolProvider> };
        let executor = DefaultToolExecutor::new(vec![component("tools"), asking]).unwrap();
        assert!(executor.questions(&call("test_tool")).unwrap().is_none(), "a tool asks nothing by default");
        assert_eq!(executor.questions(&call("asking_tool")).unwrap().map(|q| q.questions.len()), Some(1));
        assert!(matches!(executor.questions(&call("does_not_exist")), Err(ToolError::UnknownTool(_))));

        let mut agent_channel_state = agent_state();
        let mut context = Turn::of(AgentId::new(), "agent".to_string(), &mut agent_channel_state, crate::agent::state::AgentStateHandle::detached()).expect("a turn is under way");
        let questions = executor.questions(&call("asking_tool")).unwrap().expect("it asks");
        let answers = Answers::declined(questions, ActorId::None, "alice".to_string());
        let output = executor.execute(&call("asking_tool"), Some(&answers), &mut context).await.unwrap();
        assert_eq!(output.value, json!({"answered_by": "alice"}));
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
        async fn execute(&self, name: &str, _params: &Value, _answers: Option<&Answers>, _turn: &mut Turn) -> ToolResult<ToolOutput> {
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
