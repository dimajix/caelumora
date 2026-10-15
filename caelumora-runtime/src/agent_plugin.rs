//! The agents as a plugin (issue 370): what makes the core more than a
//! chat between users - its kinds of agent, whose actors run a model here
//! (`AgentActor`) or reach an agent that runs elsewhere
//! (`ExternalAgentActor`), and the kinds of their components. An
//! essential plugin: a program without it runs
//! no agent. It lies in the runtime until the agents' logic is a crate of
//! its own.

use crate::actor::agent::AgentActor;
use crate::actor::external_agent::ExternalAgentActor;
use crate::actor::Actor;
use crate::component::ComponentRegistry;
use crate::plugin::Plugin;
use crate::{compaction, component_impl, external, llm, prompt, tool};
use crate::runtime::RuntimeBuilder;
use caelumora_model::agent::PersonalityKind;

/// Registers the kinds of an agent's components with `registry` (issue
/// 370): its context (`prompt`), its model (`llm`), its tools (`tool`), its
/// memory and notes (`component_impl`), its compaction (`compaction`) and
/// the facet an agent that runs elsewhere is reached through (`external`).
/// What `AgentsPlugin` registers, and a test that builds components without
/// a runtime.
pub fn register_components(registry: &mut ComponentRegistry) {
    component_impl::register_all(registry);
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

    fn register(&self, runtime: &mut RuntimeBuilder) -> anyhow::Result<()> {
        register_components(runtime.components());
        let kinds = runtime.agent_kinds();
        kinds.register(PersonalityKind::Internal, AgentActor::COMPONENT_FACETS, |channel, agent, personality, runtime| async move {
            Ok(Box::new(AgentActor::new(channel, agent, personality, runtime).await?) as Box<dyn Actor>)
        });
        kinds.register(PersonalityKind::External, ExternalAgentActor::COMPONENT_FACETS, |channel, agent, personality, runtime| async move {
            Ok(Box::new(ExternalAgentActor::new(channel, agent, personality, runtime).await?) as Box<dyn Actor>)
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::actor::agent::AgentActor;
    use crate::actor::external_agent::ExternalAgentActor;
    use super::AgentsPlugin;
    use crate::channel::ChannelLoop;
    use crate::component::testing::config;
    use crate::component::{ComponentContainer, ComponentContext};
    use crate::moderator::policy_moderator::POLICY_MODERATOR_KIND;
    use crate::moderator::round_robin_policy::ROUND_ROBIN_POLICY_KIND;
    use crate::runtime::{RuntimeManager, Services};
    use crate::agent_testing::test_registry;
    use crate::tool::echo_tool::ECHO_TOOL_KIND;
    use caelumora_model::component::ComponentConfig;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::agent::PersonalityKind;
    use caelumora_model::config::AppConfig;
    use caelumora_model::confidentiality::ConfidentialityRules;
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::AllowAllPolicy;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::type_map::TypeKey;
    use std::sync::Arc;

    /// The names of `facets`, to compare them by.
    fn names(facets: Option<&[TypeKey]>) -> Option<Vec<&'static str>> {
        facets.map(|facets| facets.iter().map(TypeKey::name).collect())
    }

    /// The plugin brings both kinds of agent, their components asked for
    /// what their actors ask for (issue 337).
    #[test]
    fn the_plugin_brings_both_kinds_of_agent() {
        let store = Arc::new(MemoryStore::new());
        let secrets = Arc::new(SecretManager::new(vec![], Arc::new(AllowAllPolicy::new())));
        let runtime = RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default()), &AppConfig::default(), test_registry(Arc::<MemoryStore>::clone(&store)), &[&AgentsPlugin]).unwrap();
        assert_eq!(names(runtime.agent_kinds().facets(PersonalityKind::Internal)), names(Some(AgentActor::COMPONENT_FACETS)));
        assert_eq!(names(runtime.agent_kinds().facets(PersonalityKind::External)), names(Some(ExternalAgentActor::COMPONENT_FACETS)));
    }

    /// The owners' own declarations: a channel takes no tool, an agent no
    /// moderator; each takes what it asks for and what that needs.
    #[tokio::test]
    async fn a_channel_takes_no_tool_and_an_agent_no_moderator() {
        let registry = Arc::new(test_registry(Arc::new(MemoryStore::new())));
        let context = ComponentContext::new(SecurityContext::default(), Services::empty());

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
        let manager = |defaults: serde_json::Value| {
            let store = Arc::new(MemoryStore::new());
            let secrets = Arc::new(SecretManager::new(vec![], Arc::new(AllowAllPolicy::new())));
            let defaults: ComponentConfig = serde_json::from_value(defaults).unwrap();
            RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default()), &AppConfig { component_defaults: defaults, ..Default::default() }, test_registry(Arc::<MemoryStore>::clone(&store)), &[&AgentsPlugin])
        };
        // As in `examples/config.yaml`.
        assert!(manager(serde_json::json!([{"id": "context", "kind": "turn_context_provider"}])).is_ok());
        let refused = manager(serde_json::json!([{"id": "context", "kind": "calculator_tool"}])).expect_err("provides no context");
        assert!(refused.to_string().contains("exactly one facet"), "{refused}");
    }
}
