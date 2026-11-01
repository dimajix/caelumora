//! The agents as a library (issues 370, 516): what makes the kernel more than a chat between
//! users - its kinds of agent, whose actors run a model here (`AgentActor`) or reach an agent that
//! runs elsewhere (`ExternalAgentActor`), and the kinds of their components. An essential library:
//! a program without it runs no agent.

use crate::actor::agent::AgentActor;
use crate::actor::external_agent::ExternalAgentActor;
use crate::{components, external, llm, prompt, tool};
use anyhow::Result;
use caelumora_kernel::api::actor::{agent_factory, Actor};
use caelumora_kernel::api::component::LibraryComponents;
use caelumora_kernel::api::library::{Library, LibraryBuilder};
use caelumora_model::agent::PersonalityKind;

/// Registers the kinds of an agent's components with `registry` (issue 370): its context
/// (`prompt`), its model (`llm`), its tools (`tool`), its memory and notes (`components`) and the
/// facet an agent that runs elsewhere is reached through (`external`). What `AgentsLibrary`
/// registers, and a test that builds components without a kernel.
pub fn register_components(registry: &mut LibraryComponents<'_>) {
    components::register_all(registry);
    prompt::register_all(registry);
    tool::register_all(registry);
    llm::register_all(registry);
    external::register_all(registry);
}

/// The agents, as a library.
#[derive(Debug, Default)]
pub struct AgentsLibrary;

impl Library for AgentsLibrary {
    fn name(&self) -> &'static str {
        "agents"
    }

    fn register(&self, library: &mut dyn LibraryBuilder) -> Result<()> {
        register_components(&mut library.components());
        library.add_prompts(prompt::NAMESPACE, prompt::PROMPTS)?;
        library.register_agent_kind(
            PersonalityKind::Internal,
            AgentActor::COMPONENT_FACETS,
            agent_factory(|runtime| async move {
                Ok(Box::new(AgentActor::new(runtime).await?) as Box<dyn Actor>)
            }),
        );
        library.register_agent_kind(
            PersonalityKind::External,
            ExternalAgentActor::COMPONENT_FACETS,
            agent_factory(|runtime| async move {
                Ok(Box::new(ExternalAgentActor::new(runtime).await?) as Box<dyn Actor>)
            }),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::AgentsLibrary;
    use crate::actor::agent::AgentActor;
    use crate::actor::external_agent::ExternalAgentActor;
    use crate::testing::{self, test_registry};
    use crate::tool::echo_tool::ECHO_TOOL_KIND;
    use caelumora_kernel::api::component::Services;
    use caelumora_kernel::channel::ChannelLoop;
    use caelumora_kernel::component::testing::config;
    use caelumora_kernel::component::BuildContext;
    use caelumora_kernel::moderator::policy_moderator::POLICY_MODERATOR_KIND;
    use caelumora_kernel::moderator::round_robin_policy::ROUND_ROBIN_POLICY_KIND;
    use caelumora_kernel::testing::TestKernel;
    use caelumora_model::agent::PersonalityKind;
    use caelumora_model::component::ComponentConfig;
    use caelumora_model::config::AppConfig;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::store::Persistence;
    use caelumora_model::type_map::TypeKey;
    use serde_json::Value;
    use std::sync::Arc;

    /// The names of `facets`, to compare them by.
    fn names(facets: Option<&[TypeKey]>) -> Option<Vec<&'static str>> {
        facets.map(|facets| facets.iter().map(TypeKey::name).collect())
    }

    /// The library brings both kinds of agent, their components asked for
    /// what their actors ask for (issue 337).
    #[test]
    fn the_library_brings_both_kinds_of_agent() {
        let kernel = TestKernel { libraries: vec![&AgentsLibrary], ..Default::default() }.build();
        assert_eq!(
            names(kernel.agent_kinds().facets(PersonalityKind::Internal)),
            names(Some(AgentActor::COMPONENT_FACETS))
        );
        assert_eq!(
            names(kernel.agent_kinds().facets(PersonalityKind::External)),
            names(Some(ExternalAgentActor::COMPONENT_FACETS))
        );
    }

    /// The owners' own declarations: a channel takes no tool, an agent no
    /// moderator; each takes what it asks for and what that needs.
    #[tokio::test]
    async fn a_channel_takes_no_tool_and_an_agent_no_moderator() {
        let registry = Arc::new(test_registry(&Persistence::in_memory()));
        // The texts its tools are made with.
        let context = BuildContext::new(
            SecurityContext::default(),
            Services::empty().with(testing::catalogue()),
        );

        let channel =
            config(&[("moderator", POLICY_MODERATOR_KIND), ("policy", ROUND_ROBIN_POLICY_KIND)]);
        registry
            .build(&channel, ChannelLoop::COMPONENT_FACETS, &context)
            .await
            .expect("its moderator and what it needs");
        let with_tool = config(&[("policy", ROUND_ROBIN_POLICY_KIND), ("echo", ECHO_TOOL_KIND)]);
        let error = registry
            .build(&with_tool, ChannelLoop::COMPONENT_FACETS, &context)
            .await
            .expect_err("a channel calls no tool");
        assert!(
            error.to_string().contains("'echo'") && error.to_string().contains("ToolProvider"),
            "{error}"
        );

        registry
            .build(&config(&[("echo", ECHO_TOOL_KIND)]), AgentActor::COMPONENT_FACETS, &context)
            .await
            .expect("an agent's tool");
        let with_moderator =
            config(&[("echo", ECHO_TOOL_KIND), ("moderator", POLICY_MODERATOR_KIND)]);
        let error = registry
            .build(&with_moderator, AgentActor::COMPONENT_FACETS, &context)
            .await
            .expect_err("an agent moderates nothing");
        assert!(
            error.to_string().contains("'moderator'") && error.to_string().contains("Moderator"),
            "{error}"
        );
    }

    #[test]
    fn the_configuration_may_replace_the_context_provider_but_only_with_one() {
        let manager = |defaults: Value| {
            let defaults: ComponentConfig = serde_json::from_value(defaults).unwrap();
            TestKernel {
                config: AppConfig { component_defaults: defaults, ..Default::default() },
                libraries: vec![&AgentsLibrary],
                ..Default::default()
            }
            .try_build()
        };
        // As in `examples/config.yaml`.
        assert!(manager(serde_json::json!([{"id": "context", "kind": "turn_context_provider"}]))
            .is_ok());
        let refused = manager(serde_json::json!([{"id": "context", "kind": "calculator_tool"}]))
            .expect_err("provides no context");
        assert!(
            refused.to_string().contains("provides no facet of which there is one"),
            "{refused}"
        );
    }
}
