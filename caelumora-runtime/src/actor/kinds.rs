//! The kinds of agent a program runs (issue 370): for each kind of
//! personality, how the actor of an agent of that kind is built and which
//! facets its components are asked for. Built after the pattern of
//! `ComponentRegistry` and `Backends`: empty to begin with, filled by the
//! plugins - the agents' (`caelumora_agents::plugin::AgentsPlugin`). The channel builds an agent's
//! actor through them and knows no actor of an agent by name; the users'
//! and the approvers' actors are the runtime's own.

use super::Actor;
use crate::channel::ChannelError;
use crate::controller::{AgentController, PersonalityController};
use crate::runtime::ActorRuntime;
use anyhow::Result;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::PersonalityKind;
use caelumora_model::type_map::TypeKey;
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::Arc;

/// What builds the actor of an agent of one kind: from its controller, its
/// personality's and its runtime, which holds the channel it takes part in.
type AgentFactory = Box<
    dyn Fn(
            Arc<AgentController>,
            Arc<PersonalityController>,
            ActorRuntime,
        ) -> BoxFuture<'static, Result<Box<dyn Actor>>>
        + Send
        + Sync,
>;

/// A registered kind of agent.
struct Registration {
    facets: &'static [TypeKey],
    factory: AgentFactory,
}

/// The kinds of agent a program runs, by the kind of their personality: what
/// the `RuntimeBuilder` is given them by, and what the channel builds an
/// agent's actor with.
#[derive(Default)]
pub struct AgentKinds {
    kinds: HashMap<PersonalityKind, Registration>,
}

impl fmt::Debug for AgentKinds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentKinds").field("kinds", &self.kinds.keys().collect::<Vec<_>>()).finish()
    }
}

impl AgentKinds {
    /// Registers the agents of personalities of `kind`: their actor is built
    /// by `factory`, their components are asked for `facets` (issue 337). A
    /// kind registered before is replaced.
    pub fn register<F, Fut>(
        &mut self,
        kind: PersonalityKind,
        facets: &'static [TypeKey],
        factory: F,
    ) where
        F: Fn(Arc<AgentController>, Arc<PersonalityController>, ActorRuntime) -> Fut
            + Send
            + Sync
            + 'static,
        Fut: Future<Output = Result<Box<dyn Actor>>> + Send + 'static,
    {
        let factory: AgentFactory = Box::new(move |agent, personality, runtime| {
            Box::pin(factory(agent, personality, runtime))
        });
        self.kinds.insert(kind, Registration { facets, factory });
    }

    /// The facets the components of an agent of `kind` are asked for;
    /// `None` for a kind the program runs no agent of.
    pub fn facets(&self, kind: PersonalityKind) -> Option<&'static [TypeKey]> {
        self.kinds.get(&kind).map(|registration| registration.facets)
    }

    /// The actor of the agent `agent` holds, built as the kind of the
    /// personality `personality` holds builds it, running with `runtime` in its channel.
    ///
    /// # Errors
    /// `ChannelError::UnknownAgentKind` for a kind the program runs no agent
    /// of; whatever building the actor reports.
    pub(crate) async fn build(
        &self,
        agent: Arc<AgentController>,
        personality: Arc<PersonalityController>,
        runtime: ActorRuntime,
    ) -> Result<Box<dyn Actor>> {
        let Some(registration) = self.kinds.get(&personality.load().kind) else {
            return Err(ChannelError::UnknownAgentKind {
                participant: ActorId::Agent(agent.id()),
                channel: runtime.channel_controller().id(),
            }
            .into());
        };
        (registration.factory)(agent, personality, runtime).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::testing::detached_actor_runtime;
    use anyhow::anyhow;
    use caelumora_model::agent::{Agent, Personality, PersonalityId};
    use caelumora_model::component::ComponentConfig;
    use caelumora_model::scope::Scope;
    use caelumora_model::security::SecurityAttributes;
    use caelumora_model::store::Persistence;
    use caelumora_model::user::UserId;
    use caelumora_model::value::name::ScopedName;

    /// The names of `facets`, to compare them by.
    fn names(facets: Option<&[TypeKey]>) -> Option<Vec<&'static str>> {
        facets.map(|facets| facets.iter().map(TypeKey::name).collect())
    }

    fn personality(kind: PersonalityKind) -> Personality {
        Personality {
            id: PersonalityId::new(),
            name: ScopedName::new("plain", Default::default()).unwrap(),
            kind,
            roles: None,
            description: None,
            introduction: None,
            components: ComponentConfig::default(),
            security_attributes: SecurityAttributes::systems(),
            reasoning_visibility: Default::default(),
            hears_during_turn: Default::default(),
            deleted: None,
        }
    }

    /// Builds the actor of an agent of `personality` with `kinds`, in a
    /// channel of no running session.
    async fn built(kinds: &AgentKinds, personality: Personality) -> Result<Box<dyn Actor>> {
        let stores = Persistence::in_memory().entity_stores().unwrap();
        let agent = Agent::new(
            ScopedName::new("agent", Scope::GlobalScope).unwrap(),
            personality.id,
            ActorId::User(UserId::new()),
        );
        let agent = Arc::new(AgentController::new(agent, None, Arc::new(()), &stores));
        kinds
            .build(
                agent,
                Arc::new(PersonalityController::new(personality)),
                detached_actor_runtime(),
            )
            .await
    }

    /// An agent of a kind the program runs none of cannot take part: its
    /// actor is refused, saying why (issue 370).
    #[tokio::test]
    async fn an_agent_of_a_kind_nobody_registered_is_refused() {
        let error = built(&AgentKinds::default(), personality(PersonalityKind::External))
            .await
            .expect_err("no kind registered");
        assert!(
            matches!(
                error.downcast_ref::<ChannelError>(),
                Some(ChannelError::UnknownAgentKind { .. })
            ),
            "{error:#}"
        );
    }

    /// The actor of an agent is built by its personality's kind, as it
    /// was registered last.
    #[tokio::test]
    async fn an_agent_is_built_by_the_kind_of_its_personality() {
        let mut kinds = AgentKinds::default();
        assert!(kinds.facets(PersonalityKind::Internal).is_none(), "none unless registered");
        kinds.register(PersonalityKind::Internal, &[], |_, _, _| async {
            Err(anyhow!("built as first registered"))
        });
        kinds.register(PersonalityKind::External, &[], |_, _, _| async {
            Err(anyhow!("built as first registered"))
        });
        kinds.register(PersonalityKind::External, &[], |_, _, _| async {
            Err(anyhow!("built as registered again"))
        });

        let error = built(&kinds, personality(PersonalityKind::External))
            .await
            .expect_err("the stand-in builds nothing");
        assert_eq!(error.to_string(), "built as registered again", "replaced");
        let error = built(&kinds, personality(PersonalityKind::Internal))
            .await
            .expect_err("the stand-in builds nothing");
        assert_eq!(error.to_string(), "built as first registered", "by its own kind");
        assert_eq!(names(kinds.facets(PersonalityKind::External)), Some(Vec::new()));
    }
}
