//! The agents as a plugin (issue 370): what makes the core more than a
//! chat between users - its kinds of agent, whose actors run a model here
//! (`AgentActor`) or reach an agent that runs elsewhere
//! (`ExternalAgentActor`), and the kinds of their components. An
//! essential plugin: a program without it runs no agent.

use crate::actor::agent::AgentActor;
use crate::actor::external_agent::ExternalAgentActor;
use caelumora_runtime::actor::Actor;
use caelumora_runtime::component::ComponentRegistry;
use caelumora_runtime::plugin::Plugin;
use crate::{compaction, components, external, llm, prompt, tool};
use caelumora_runtime::runtime::RuntimeBuilder;
use caelumora_model::agent::PersonalityKind;
use anyhow::Result;

/// Registers the kinds of an agent's components with `registry` (issue
/// 370): its context (`prompt`), its model (`llm`), its tools (`tool`), its
/// memory and notes (`components`), its compaction (`compaction`) and
/// the facet an agent that runs elsewhere is reached through (`external`).
/// What `AgentsPlugin` registers, and a test that builds components without
/// a runtime.
pub fn register_components(registry: &mut ComponentRegistry) {
    components::register_all(registry);
    prompt::register_all(registry);
    tool::register_all(registry);
    llm::register_all(registry);
    external::register_all(registry);
    compaction::register_all(registry);
}

/// The agents, as a plugin.
#[derive(Debug, Default)]
pub struct AgentsPlugin;

impl Plugin for AgentsPlugin {
    fn name(&self) -> &'static str {
        "agents"
    }

    fn register(&self, runtime: &mut RuntimeBuilder) -> Result<()> {
        register_components(runtime.components());
        runtime.prompts().add(prompt::NAMESPACE, prompt::PROMPTS)?;
        let kinds = runtime.agent_kinds();
        kinds.register(PersonalityKind::Internal, AgentActor::COMPONENT_FACETS, |agent, personality, runtime| async move {
            Ok(Box::new(AgentActor::new(agent, personality, runtime).await?) as Box<dyn Actor>)
        });
        kinds.register(PersonalityKind::External, ExternalAgentActor::COMPONENT_FACETS, |agent, personality, runtime| async move {
            Ok(Box::new(ExternalAgentActor::new(agent, personality, runtime).await?) as Box<dyn Actor>)
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::actor::agent::AgentActor;
    use crate::actor::external_agent::ExternalAgentActor;
    use caelumora_runtime::channel::ChannelLoop;
    use caelumora_runtime::component::testing::config;
    use caelumora_runtime::component::{ComponentContainer, ComponentContext};
    use caelumora_runtime::moderator::policy_moderator::POLICY_MODERATOR_KIND;
    use caelumora_runtime::moderator::round_robin_policy::ROUND_ROBIN_POLICY_KIND;
    use caelumora_runtime::runtime::Services;
    use super::AgentsPlugin;
    use crate::testing::{self, test_registry};
    use caelumora_runtime::testing::TestRuntime;
    use crate::tool::echo_tool::ECHO_TOOL_KIND;
    use caelumora_model::component::ComponentConfig;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::agent::PersonalityKind;
    use caelumora_model::config::AppConfig;
    use caelumora_model::store::StoreBackend;
    use caelumora_model::type_map::TypeKey;
    use std::sync::Arc;
    use serde_json::Value;

    /// The names of `facets`, to compare them by.
    fn names(facets: Option<&[TypeKey]>) -> Option<Vec<&'static str>> {
        facets.map(|facets| facets.iter().map(TypeKey::name).collect())
    }

    /// The plugin brings both kinds of agent, their components asked for
    /// what their actors ask for (issue 337).
    #[test]
    fn the_plugin_brings_both_kinds_of_agent() {
        let runtime = TestRuntime { plugins: vec![&AgentsPlugin], ..Default::default() }.build();
        assert_eq!(names(runtime.agent_kinds().facets(PersonalityKind::Internal)), names(Some(AgentActor::COMPONENT_FACETS)));
        assert_eq!(names(runtime.agent_kinds().facets(PersonalityKind::External)), names(Some(ExternalAgentActor::COMPONENT_FACETS)));
    }

    /// The owners' own declarations: a channel takes no tool, an agent no
    /// moderator; each takes what it asks for and what that needs.
    #[tokio::test]
    async fn a_channel_takes_no_tool_and_an_agent_no_moderator() {
        let registry = Arc::new(test_registry(StoreBackend::in_memory().component_store().unwrap()));
        // The texts its tools are made with.
        let context = ComponentContext::new(SecurityContext::default(), Services::empty().with(testing::catalogue()));

        let channel = config(&[("moderator", POLICY_MODERATOR_KIND), ("policy", ROUND_ROBIN_POLICY_KIND)]);
        ComponentContainer::from_config(&channel, ChannelLoop::COMPONENT_FACETS, &registry, &context).await.expect("its moderator and what it needs");
        let with_tool = config(&[("policy", ROUND_ROBIN_POLICY_KIND), ("echo", ECHO_TOOL_KIND)]);
        let error = ComponentContainer::from_config(&with_tool, ChannelLoop::COMPONENT_FACETS, &registry, &context).await.expect_err("a channel calls no tool");
        assert!(error.to_string().contains("'echo'") && error.to_string().contains("ToolProvider"), "{error}");

        ComponentContainer::from_config(&config(&[("echo", ECHO_TOOL_KIND)]), AgentActor::COMPONENT_FACETS, &registry, &context).await.expect("an agent's tool");
        let with_moderator = config(&[("echo", ECHO_TOOL_KIND), ("moderator", POLICY_MODERATOR_KIND)]);
        let error = ComponentContainer::from_config(&with_moderator, AgentActor::COMPONENT_FACETS, &registry, &context).await.expect_err("an agent moderates nothing");
        assert!(error.to_string().contains("'moderator'") && error.to_string().contains("Moderator"), "{error}");
    }

    #[test]
    fn the_configuration_may_replace_the_context_provider_but_only_with_one() {
        let manager = |defaults: Value| {
            let defaults: ComponentConfig = serde_json::from_value(defaults).unwrap();
            TestRuntime { config: AppConfig { component_defaults: defaults, ..Default::default() }, plugins: vec![&AgentsPlugin], ..Default::default() }.try_build()
        };
        // As in `examples/config.yaml`.
        assert!(manager(serde_json::json!([{"id": "context", "kind": "turn_context_provider"}])).is_ok());
        let refused = manager(serde_json::json!([{"id": "context", "kind": "calculator_tool"}])).expect_err("provides no context");
        assert!(refused.to_string().contains("provides no facet of which there is one"), "{refused}");
    }
}
