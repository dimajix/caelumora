//! The agents as a plugin (issue 370): what makes the core more than a
//! chat between users - its kinds of agent, whose actors run a model here
//! (`AgentActor`) or reach an agent that runs elsewhere
//! (`ExternalAgentActor`). An essential plugin: a program without it runs
//! no agent. It lies in the runtime until the agents' logic is a crate of
//! its own.

use crate::actor::agent::AgentActor;
use crate::actor::external_agent::ExternalAgentActor;
use crate::actor::Actor;
use crate::plugin::Plugin;
use crate::runtime::RuntimeBuilder;
use caelumora_model::agent::PersonalityKind;

/// The agents, as a plugin.
#[derive(Debug, Default)]
pub struct AgentsPlugin;

impl Plugin for AgentsPlugin {
    fn name(&self) -> &'static str {
        "agents"
    }

    fn register(&self, runtime: &mut RuntimeBuilder) -> anyhow::Result<()> {
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
    use crate::session::testing::test_runtime;
    use caelumora_model::agent::PersonalityKind;
    use caelumora_model::type_map::TypeKey;

    /// The names of `facets`, to compare them by.
    fn names(facets: Option<&[TypeKey]>) -> Option<Vec<&'static str>> {
        facets.map(|facets| facets.iter().map(TypeKey::name).collect())
    }

    /// The plugin brings both kinds of agent, their components asked for
    /// what their actors ask for (issue 337).
    #[test]
    fn the_plugin_brings_both_kinds_of_agent() {
        let runtime = test_runtime();
        assert_eq!(names(runtime.agent_kinds().facets(PersonalityKind::Internal)), names(Some(AgentActor::COMPONENT_FACETS)));
        assert_eq!(names(runtime.agent_kinds().facets(PersonalityKind::External)), names(Some(ExternalAgentActor::COMPONENT_FACETS)));
    }
}
