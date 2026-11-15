//! `ToolProviders`: the tools of every `ToolProvider` of an agent, found by
//! their names (issues 100, 280, 584). It only finds a tool: the
//! `ToolExecutor` that holds it asks the tool found only what the policy
//! permits, and makes what the model is told of the result.

use crate::situation::Situation;
use crate::tool::{Tool, ToolProvider};
use caelumora_api::component::{ComponentError, ComponentImpl, ComponentResult};
use caelumora_domain::tool::{ToolDescription, ToolSet};
use std::sync::Arc;
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
                let name = &tool.description().name;
                if let Some((_, component)) = seen.iter().find(|(seen, _)| seen == name) {
                    return Err(ComponentError::InvalidConfig(format!(
                        "tool '{}' is provided by both component '{}' and component '{}'",
                        name, component, provider.id
                    )));
                }
                seen.push((name.clone(), provider.id.clone()));
            }
        }
        Ok(Self { providers })
    }

    /// The tool named `name`, as the providers' tools are now: of the provider configured first
    /// that has one (issue 126), `None` if none has.
    pub(super) fn find(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.providers.iter().find_map(|provider| {
            provider.tools().into_iter().find(|t| t.description().name == name)
        })
    }

    /// The tools offered in `situation`, in the sets of the providers they come
    /// from (issue 194), in the order of the configuration: each as its
    /// provider offers them there (`ToolProvider::offered`, issue 290), or
    /// unavailable without tools, saying why. A tool of the name of one
    /// offered before is withheld, with a warning: a provider's tools may
    /// change after `new` checked them (issue 126), and a call of that name
    /// reaches the first (`find`).
    pub(super) async fn tool_sets(&self, situation: &Situation<'_>) -> Vec<ToolSet> {
        let mut sets: Vec<ToolSet> = Vec::with_capacity(self.providers.len());
        for provider in &self.providers {
            let (mut tools, unavailable): (Vec<ToolDescription>, _) =
                match provider.offered(situation).await {
                    Ok(tools) => (tools.iter().map(|t| t.description().clone()).collect(), None),
                    Err(why) => (Vec::new(), Some(why)),
                };
            tools.retain(|tool| match sets.iter().find(|set| set.tools.iter().any(|t| t.name == tool.name)) {
                Some(set) => {
                    warn!("Tool '{}' of component '{}' is not offered: component '{}' offers a tool of that name", tool.name, provider.id, set.name);
                    false
                }
                None => true,
            });
            sets.push(ToolSet {
                unavailable,
                destination: provider.destination(),
                ..ToolSet::new(provider.id.clone(), provider.description(), tools)
            });
        }
        sets
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{FixedTools, ToolOutput, ToolResult};
    use crate::turn::Turn;
    use async_trait::async_trait;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use caelumora_domain::question::{Addressee, Answers, Question, Questions};
    use caelumora_domain::tool::ToolParameters;
    use caelumora_domain::turn::TurnState;
    use caelumora_domain::work::Work;
    use serde_json::{json, Value};
    use std::sync::Mutex;

    /// The description of a tool named `name`, without parameters.
    fn described(name: &str) -> ToolDescription {
        ToolDescription {
            name: name.to_string(),
            description: "desc".to_string(),
            parameters: ToolParameters::object(
                Vec::<(String, ToolParameters)>::new(),
                Vec::<String>::new(),
            ),
            requires_approval: false,
            visibility: Default::default(),
            classification: None,
            presentation: Default::default(),
        }
    }

    /// A tool that answers "ok".
    #[derive(Debug)]
    struct Answering(ToolDescription);

    #[async_trait]
    impl Tool for Answering {
        fn description(&self) -> &ToolDescription {
            &self.0
        }
        async fn execute(
            &self,
            _params: &Value,
            _answers: Option<&Answers>,
            _turn: &mut Turn<'_>,
        ) -> ToolResult<ToolOutput> {
            Ok(json!({"result": "ok"}).into())
        }
    }

    /// The tool `name`, which answers "ok".
    fn answering(name: &str) -> Arc<dyn Tool> {
        Arc::new(Answering(described(name)))
    }

    /// The component `id`, whose tool is `test_tool`.
    fn component(id: &str) -> ComponentImpl<dyn ToolProvider> {
        ComponentImpl {
            id: id.to_string(),
            fail_policy: Default::default(),
            facet: Arc::new(FixedTools::new(
                "mock tools".to_string(),
                vec![answering("test_tool")],
            )),
        }
    }

    fn agent_state() -> AgentChannelState {
        AgentChannelState {
            turn: Some(TurnState { work: Work::default(), ..Default::default() }),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_call_finds_the_tool_of_its_name_and_none_of_another() {
        let providers = ToolProviders::new(vec![component("tools")]).unwrap();
        let mut agent_channel_state = agent_state();
        let mut context = Turn::under_way(AgentId::new(), &mut agent_channel_state);

        let tool = providers.find("test_tool").expect("found");
        let output = tool.execute(&json!({}), None, &mut context).await.unwrap();
        assert_eq!(output.value, json!({"result": "ok"}));
        assert!(providers.find("does_not_exist").is_none());
    }

    /// A tool that asks a question and says what it was answered.
    #[derive(Debug)]
    struct Asking(ToolDescription);

    #[async_trait]
    impl Tool for Asking {
        fn description(&self) -> &ToolDescription {
            &self.0
        }
        async fn execute(
            &self,
            _params: &Value,
            answers: Option<&Answers>,
            _turn: &mut Turn<'_>,
        ) -> ToolResult<ToolOutput> {
            Ok(json!({"answered_by": answers.map(|a| a.answered_by_name.clone())}).into())
        }
        fn questions(&self, _params: &Value) -> ToolResult<Option<Questions>> {
            Ok(Some(Questions {
                questions: vec![Question {
                    header: "Name".to_string(),
                    question: "Who?".to_string(),
                    choices: Vec::new(),
                    multi_select: false,
                }],
                ask: vec![Addressee::Owner],
                sensitive: false,
            }))
        }
    }

    #[tokio::test]
    async fn the_questions_of_a_call_are_its_tool_s_and_the_answers_go_to_it() {
        let asking = ComponentImpl {
            id: "asking".to_string(),
            fail_policy: Default::default(),
            facet: Arc::new(FixedTools::new(
                "asking tools".to_string(),
                vec![Arc::new(Asking(described("asking_tool")))],
            )) as Arc<dyn ToolProvider>,
        };
        let providers = ToolProviders::new(vec![component("tools"), asking]).unwrap();
        let test_tool = providers.find("test_tool").unwrap();
        assert!(
            test_tool.questions(&json!({})).unwrap().is_none(),
            "a tool asks nothing by default"
        );
        let asking_tool = providers.find("asking_tool").unwrap();
        let questions = asking_tool.questions(&json!({})).unwrap().expect("it asks");
        assert_eq!(questions.questions.len(), 1);

        let mut agent_channel_state = agent_state();
        let mut context = Turn::under_way(AgentId::new(), &mut agent_channel_state);
        let answers = Answers::declined(questions, ActorId::None, "alice".to_string());
        let output = asking_tool.execute(&json!({}), Some(&answers), &mut context).await.unwrap();
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
        fn tools(&self) -> Vec<Arc<dyn Tool>> {
            vec![answering("remote_tool")]
        }
        async fn offered(&self, _situation: &Situation<'_>) -> Result<Vec<Arc<dyn Tool>>, String> {
            Err("its server cannot be reached".to_string())
        }
    }

    #[tokio::test]
    async fn each_provider_s_tools_are_a_set_named_after_its_component_as_it_offers_them() {
        let remote = ComponentImpl {
            id: "remote".to_string(),
            fail_policy: Default::default(),
            facet: Arc::new(Unreachable) as Arc<dyn ToolProvider>,
        };
        let providers = ToolProviders::new(vec![component("local"), remote]).unwrap();
        let mut agent_channel_state = agent_state();
        let turn = Turn::under_way(AgentId::new(), &mut agent_channel_state);
        let sets: Vec<_> = providers
            .tool_sets(&turn.situation())
            .await
            .into_iter()
            .map(|set| {
                (
                    set.name,
                    set.description,
                    set.tools.into_iter().map(|t| t.name).collect::<Vec<_>>(),
                    set.unavailable,
                )
            })
            .collect();
        assert_eq!(sets, [
            ("local".to_string(), "mock tools".to_string(), vec!["test_tool".to_string()], None),
            ("remote".to_string(), "remote tools".to_string(), Vec::new(), Some("its server cannot be reached".to_string())),
        ], "in the order of the configuration, an unavailable one included, without the tools it has");
    }

    /// A provider whose tools are named as it is told, and may change.
    #[derive(Debug)]
    struct Changing(Mutex<Vec<&'static str>>);

    impl ToolProvider for Changing {
        fn description(&self) -> String {
            "changing tools".to_string()
        }
        fn tools(&self) -> Vec<Arc<dyn Tool>> {
            let described =
                |name| ToolDescription { description: "changing".to_string(), ..described(name) };
            self.0
                .lock()
                .unwrap()
                .iter()
                .map(|name| Arc::new(Answering(described(name))) as _)
                .collect()
        }
    }

    #[tokio::test]
    async fn a_tool_whose_name_another_provider_took_since_is_not_offered_nor_found() {
        let changing = Arc::new(Changing(Mutex::new(vec!["other_tool"])));
        let providers = ToolProviders::new(vec![
            component("local"),
            ComponentImpl {
                id: "changing".to_string(),
                fail_policy: Default::default(),
                facet: Arc::clone(&changing) as Arc<dyn ToolProvider>,
            },
        ])
        .unwrap();
        *changing.0.lock().unwrap() = vec!["test_tool", "other_tool"];

        let mut agent_channel_state = agent_state();
        let turn = Turn::under_way(AgentId::new(), &mut agent_channel_state);
        let offered: Vec<_> = providers
            .tool_sets(&turn.situation())
            .await
            .into_iter()
            .map(|set| (set.name, set.tools.into_iter().map(|t| t.name).collect::<Vec<_>>()))
            .collect();
        assert_eq!(
            offered,
            [
                ("local".to_string(), vec!["test_tool".to_string()]),
                ("changing".to_string(), vec!["other_tool".to_string()]),
            ],
            "the first keeps the name, as a call of it reaches the first"
        );
        let found = providers.find("test_tool").unwrap();
        assert_eq!(found.description().description, "desc", "the first's");
    }

    #[test]
    fn two_components_with_a_tool_of_the_same_name_are_a_configuration_error() {
        let error = ToolProviders::new(vec![component("local"), component("remote")])
            .expect_err("the second test_tool would be unreachable");
        let message = error.to_string();
        assert!(
            message.contains("'test_tool'")
                && message.contains("'local'")
                && message.contains("'remote'"),
            "{message}"
        );

        ToolProviders::new(vec![component("local")]).expect("one provider is fine");
    }
}
