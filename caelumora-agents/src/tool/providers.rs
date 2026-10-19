//! `ToolProviders`: the tools of every `ToolProvider` of an agent, found by
//! their names (issues 100, 280). It only finds a tool and hands a call to
//! it: the `ToolExecutor` that holds it hands it only calls the policy
//! permits, and makes what the model is told of the result.

use crate::turn::Turn;
use crate::situation::Situation;
use caelumora_runtime::component::{ComponentError, ComponentImpl, ComponentResult};
use crate::tool::{ToolError, ToolOutput, ToolProvider, ToolResult};
use caelumora_model::approval::ToolMatcher;
use caelumora_model::question::{Answers, Questions};
use caelumora_model::tool::{ToolCall, ToolSet};
use tracing::warn;

/// The tool providers of an agent's components. It keeps the providers,
/// not their tools: those are asked for each time, since a provider's tools
/// may change while the agent runs (issue 100). Each provider's tools are a
/// tool set named after its component (issue 194).
#[derive(Debug)]
pub(super) struct ToolProviders {
    providers: Vec<ComponentImpl<dyn ToolProvider>>,
}

impl ToolProviders {
    /// The tools `providers` contribute.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if two of them provide a tool of the
    /// same name: that is a mistake in the configuration, and resolving it
    /// by order would make one of them silently unreachable (issue 37) - the
    /// same rule as for user commands.
    pub(super) fn new(providers: Vec<ComponentImpl<dyn ToolProvider>>) -> ComponentResult<Self> {
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

    /// Runs `call` with the tool that provides it: what the tool gave, or
    /// what went wrong (`ToolError::UnknownTool` if no tool has the name),
    /// with the `answers` to the questions the tool asked for the call.
    pub(super) async fn execute(&self, call: &ToolCall, answers: Option<&Answers>, turn: &mut Turn<'_>) -> ToolResult<ToolOutput> {
        self.provider_of(&call.function_name)?.execute(&call.function_name, &call.arguments, answers, turn).await
    }

    /// What `call` does, as its tool describes it to the person asked.
    pub(super) fn describe(&self, call: &ToolCall) -> ToolResult<String> {
        self.provider_of(&call.function_name)?.describe(&call.function_name, &call.arguments)
    }

    /// What `call` asks a person before it runs, as its tool says (issue
    /// 165).
    pub(super) fn questions(&self, call: &ToolCall) -> ToolResult<Option<Questions>> {
        self.provider_of(&call.function_name)?.questions(&call.function_name, &call.arguments)
    }

    /// What other calls are like `call`, as its tool says (issue 13); the
    /// calls exactly like it if no tool has its name. `None` where that is
    /// no rule a person could keep (`ToolMatcher::check`, issue 400): a
    /// rule is offered only where it can be added.
    pub(super) fn matcher(&self, call: &ToolCall) -> Option<ToolMatcher> {
        let matcher = match self.provider_of(&call.function_name) {
            Ok(provider) => provider.matcher(&call.function_name, &call.arguments),
            Err(_) => ToolMatcher::exact(call),
        };
        matcher.check().is_ok().then_some(matcher)
    }

    /// The tools offered in `situation`, in the sets of the providers they come
    /// from (issue 194), in the order of the configuration: each as its
    /// provider offers them there (`ToolProvider::offered`, issue 290), or
    /// unavailable without tools, saying why. A tool of the name of one
    /// offered before is withheld, with a warning: a provider's tools may
    /// change after `new` checked them (issue 126), and a call of that name
    /// reaches the first (`provider_of`).
    pub(super) async fn tool_sets(&self, situation: &Situation<'_>) -> Vec<ToolSet> {
        let mut sets: Vec<ToolSet> = Vec::with_capacity(self.providers.len());
        for provider in &self.providers {
            let (mut tools, unavailable) = match provider.offered(situation).await {
                Ok(tools) => (tools, None),
                Err(why) => (Vec::new(), Some(why)),
            };
            tools.retain(|tool| match sets.iter().find(|set| set.tools.iter().any(|t| t.name == tool.name)) {
                Some(set) => {
                    warn!("Tool '{}' of component '{}' is not offered: component '{}' offers a tool of that name", tool.name, provider.id, set.name);
                    false
                }
                None => true,
            });
            sets.push(ToolSet { unavailable, destination: provider.destination(), ..ToolSet::new(provider.id.clone(), provider.description(), tools) });
        }
        sets
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::security::SecurityContext;
    use caelumora_model::tool::{Tool, ToolParameters};
    use serde_json::{json, Value};
    use caelumora_model::question::{Addressee, Question};
    use caelumora_model::turn::TurnState;
    use caelumora_model::work::Work;

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
                parameters: ToolParameters::object(Vec::<(String, ToolParameters)>::new(), Vec::<String>::new()),
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
        AgentChannelState { turn: Some(TurnState { work: Work { security_context: SecurityContext { caller_id: ActorId::Agent(AgentId::new()), ..SecurityContext::default() }, ..Default::default() }, ..Default::default() }), ..Default::default() }
    }

    #[tokio::test]
    async fn a_call_runs_with_the_tool_that_provides_it_and_an_unknown_one_is_reported() {
        let providers = ToolProviders::new(vec![component("tools")]).unwrap();
        let mut agent_channel_state = agent_state();
        let mut context = Turn::under_way(AgentId::new(), &mut agent_channel_state);

        let output = providers.execute(&call("test_tool"), None, &mut context).await.unwrap();
        assert_eq!(output.value, json!({"result": "ok"}));
        let unknown = providers.execute(&call("does_not_exist"), None, &mut context).await;
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
                questions: vec![Question { header: "Name".to_string(), question: "Who?".to_string(), choices: Vec::new(), multi_select: false }],
                ask: vec![Addressee::Owner],
                sensitive: false,
            }))
        }
    }

    #[tokio::test]
    async fn the_questions_of_a_call_are_its_tool_s_and_the_answers_go_to_it() {
        let asking = ComponentImpl { id: "asking".to_string(), fail_policy: Default::default(), facet: Arc::new(Asking) as Arc<dyn ToolProvider> };
        let providers = ToolProviders::new(vec![component("tools"), asking]).unwrap();
        assert!(providers.questions(&call("test_tool")).unwrap().is_none(), "a tool asks nothing by default");
        assert_eq!(providers.questions(&call("asking_tool")).unwrap().map(|q| q.questions.len()), Some(1));
        assert!(matches!(providers.questions(&call("does_not_exist")), Err(ToolError::UnknownTool(_))));

        let mut agent_channel_state = agent_state();
        let mut context = Turn::under_way(AgentId::new(), &mut agent_channel_state);
        let questions = providers.questions(&call("asking_tool")).unwrap().expect("it asks");
        let answers = Answers::declined(questions, ActorId::None, "alice".to_string());
        let output = providers.execute(&call("asking_tool"), Some(&answers), &mut context).await.unwrap();
        assert_eq!(output.value, json!({"answered_by": "alice"}));
    }

    /// A provider whose source cannot be reached now: it has a tool, and
    /// offers none.
    #[derive(Debug)]
    struct Unreachable;

    #[async_trait]
    impl ToolProvider for Unreachable {
        fn description(&self) -> String {
            "remote tools".to_string()
        }
        fn tools(&self) -> Vec<Tool> {
            vec![Tool { name: "remote_tool".to_string(), ..MockToolProvider.tools().remove(0) }]
        }
        async fn offered(&self, _situation: &Situation<'_>) -> Result<Vec<Tool>, String> {
            Err("its server cannot be reached".to_string())
        }
        async fn execute(&self, name: &str, _params: &Value, _answers: Option<&Answers>, _turn: &mut Turn) -> ToolResult<ToolOutput> {
            Err(ToolError::UnknownTool(name.to_string()))
        }
    }

    #[tokio::test]
    async fn each_provider_s_tools_are_a_set_named_after_its_component_as_it_offers_them() {
        let remote = ComponentImpl { id: "remote".to_string(), fail_policy: Default::default(), facet: Arc::new(Unreachable) as Arc<dyn ToolProvider> };
        let providers = ToolProviders::new(vec![component("local"), remote]).unwrap();
        let mut agent_channel_state = agent_state();
        let turn = Turn::under_way(AgentId::new(), &mut agent_channel_state);
        let sets: Vec<_> = providers.tool_sets(&turn.situation()).await.into_iter()
            .map(|set| (set.name, set.description, set.tools.into_iter().map(|t| t.name).collect::<Vec<_>>(), set.unavailable))
            .collect();
        assert_eq!(sets, [
            ("local".to_string(), "mock tools".to_string(), vec!["test_tool".to_string()], None),
            ("remote".to_string(), "remote tools".to_string(), Vec::new(), Some("its server cannot be reached".to_string())),
        ], "in the order of the configuration, an unavailable one included, without the tools it has");
    }

    /// A provider whose tools are as it is told, and may change.
    #[derive(Debug)]
    struct Changing(Mutex<Vec<Tool>>);

    #[async_trait]
    impl ToolProvider for Changing {
        fn description(&self) -> String {
            "changing tools".to_string()
        }
        fn tools(&self) -> Vec<Tool> {
            self.0.lock().unwrap().clone()
        }
        async fn execute(&self, name: &str, _params: &Value, _answers: Option<&Answers>, _turn: &mut Turn) -> ToolResult<ToolOutput> {
            Err(ToolError::UnknownTool(name.to_string()))
        }
    }

    #[tokio::test]
    async fn a_tool_whose_name_another_provider_took_since_is_not_offered() {
        let tool = |name: &str| Tool { name: name.to_string(), ..MockToolProvider.tools().remove(0) };
        let changing = Arc::new(Changing(Mutex::new(vec![tool("other_tool")])));
        let providers = ToolProviders::new(vec![component("local"), ComponentImpl { id: "changing".to_string(), fail_policy: Default::default(), facet: Arc::clone(&changing) as Arc<dyn ToolProvider> }]).unwrap();
        *changing.0.lock().unwrap() = vec![tool("test_tool"), tool("other_tool")];

        let mut agent_channel_state = agent_state();
        let turn = Turn::under_way(AgentId::new(), &mut agent_channel_state);
        let offered: Vec<_> = providers.tool_sets(&turn.situation()).await.into_iter()
            .map(|set| (set.name, set.tools.into_iter().map(|t| t.name).collect::<Vec<_>>()))
            .collect();
        assert_eq!(offered, [
            ("local".to_string(), vec!["test_tool".to_string()]),
            ("changing".to_string(), vec!["other_tool".to_string()]),
        ], "the first keeps the name, as a call of it reaches the first");
    }

    #[test]
    fn two_components_with_a_tool_of_the_same_name_are_a_configuration_error() {
        let error = ToolProviders::new(vec![component("local"), component("remote")])
            .expect_err("the second test_tool would be unreachable");
        let message = error.to_string();
        assert!(message.contains("'test_tool'") && message.contains("'local'") && message.contains("'remote'"), "{message}");

        ToolProviders::new(vec![component("local")]).expect("one provider is fine");
    }
}
