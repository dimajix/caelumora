//! The kinds of agent a program runs (issue 370): for each kind of personality, how the actor of an
//! agent of that kind is built and which facets its components are asked for. Built after the
//! pattern of `ComponentRegistry` and `Backends`: empty to begin with, filled by the libraries -
//! the agents' (`caelumora_agents::library::AgentsLibrary`). The channel builds an agent's actor
//! through them and knows no actor of an agent by name; the users' and the approvers' actors are
//! the kernel's own.

use crate::channel::ChannelError;
use crate::controller::{AgentController, PersonalityController};
use crate::kernel::{AgentRuntime, ParticipantRuntime};
use anyhow::{anyhow, Result};
use caelumora_api::actor::{Actor, AgentFactory};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::PersonalityKind;
use caelumora_model::type_map::TypeKey;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// A registered kind of agent.
struct Registration {
    facets: &'static [TypeKey],
    factory: AgentFactory,
}

/// The kinds of agent a program runs, by the kind of their personality: what
/// the `KernelBuilder` is given them by, and what the channel builds an
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
    /// by `factory` from the agent's runtime (issue 516), their components are asked for `facets`
    /// (issue 337). A kind registered before is replaced.
    pub fn register(
        &mut self,
        kind: PersonalityKind,
        facets: &'static [TypeKey],
        factory: AgentFactory,
    ) {
        self.kinds.insert(kind, Registration { facets, factory });
    }

    /// The facets the components of an agent of `kind` are asked for;
    /// `None` for a kind the program runs no agent of.
    pub fn facets(&self, kind: PersonalityKind) -> Option<&'static [TypeKey]> {
        self.kinds.get(&kind).map(|registration| registration.facets)
    }

    /// The actor of the agent `agent` holds, built as the kind of the
    /// personality `personality` holds builds it, running with `runtime` in its channel - given
    /// its `AgentRuntime` (issue 516).
    ///
    /// # Errors
    /// `ChannelError::UnknownAgentKind` for a kind the program runs no agent
    /// of; an error if the agent's state cannot be loaded, if one of its commands has the name of
    /// a command of the session, which would hide it (issue 37); whatever building the actor
    /// reports.
    pub(crate) async fn build(
        &self,
        agent: Arc<AgentController>,
        personality: Arc<PersonalityController>,
        runtime: ParticipantRuntime,
    ) -> Result<Box<dyn Actor>> {
        let Some(registration) = self.kinds.get(&personality.load().kind) else {
            return Err(ChannelError::UnknownAgentKind {
                participant: ActorId::Agent(agent.id()),
                channel: runtime.channel_controller().id(),
            }
            .into());
        };
        let session_commands = Arc::clone(runtime.command_executor());
        let agent_runtime = AgentRuntime::new(runtime, agent, personality).await?;
        let actor = (registration.factory)(Arc::new(agent_runtime)).await?;
        if let Some(command) = actor.commands().into_iter().find(|c| session_commands.has(&c.name))
        {
            return Err(anyhow!(
                "command /{} of agent '{}' ({}), which a component of its personality provides, \
                 is a command of the session too, which would hide it",
                command.name,
                actor.name(),
                actor.id()
            ));
        }
        Ok(actor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::help_command::{HelpCommandProvider, HELP_COMMAND_NAME};
    use crate::command::{ChannelCommandProvider, CommandExecutor};
    use crate::kernel::ChannelRuntime;
    use crate::session::testing::test_kernel;
    use async_trait::async_trait;
    use caelumora_api::actor::agent_factory;
    use caelumora_api::component::ComponentImpl;
    use caelumora_api::transport::{EventReceiver, EventSender};
    use caelumora_model::actor::{ActorState, Envelope};
    use caelumora_model::agent::testing::make_personality;
    use caelumora_model::agent::{Agent, Personality};
    use caelumora_model::channel::testing::bare_channel;
    use caelumora_model::command::Command;
    use caelumora_model::scope::Scope;
    use caelumora_model::session::testing::bare_session;
    use caelumora_model::session::SessionId;
    use caelumora_model::user::UserId;
    use caelumora_model::value::name::{Name, ScopedName};
    use caelumora_security::AccessPolicy;
    use std::sync::Weak;

    /// The names of `facets`, to compare them by.
    fn names(facets: Option<&[TypeKey]>) -> Option<Vec<&'static str>> {
        facets.map(|facets| facets.iter().map(TypeKey::name).collect())
    }

    /// Builds the actor of an agent of `personality` with `kinds`, in a
    /// channel of no running session, whose session has no commands.
    async fn built(kinds: &AgentKinds, personality: Personality) -> Result<Box<dyn Actor>> {
        built_with(kinds, personality, CommandExecutor::empty).await
    }

    /// As `built`, the session's commands those `commands` makes of the access policy.
    async fn built_with(
        kinds: &AgentKinds,
        personality: Personality,
        commands: impl FnOnce(Arc<dyn AccessPolicy>) -> CommandExecutor,
    ) -> Result<Box<dyn Actor>> {
        let kernel = test_kernel();
        let agent = Agent::new(
            ScopedName::new("agent", Scope::GlobalScope).unwrap(),
            personality.id,
            ActorId::User(UserId::new()),
        );
        let controllers = kernel.controller_manager();
        let session = bare_session(SessionId::new());
        let channel = bare_channel(session.id);
        let channel = ChannelRuntime::new(
            Arc::clone(&kernel),
            Arc::new(commands(Arc::clone(kernel.access_policy()))),
            controllers.new_session_controller(session),
            controllers.new_channel_controller(channel),
            Weak::new(),
        );
        let participant = channel.new_participant_runtime(ActorId::Agent(agent.id));
        let agent =
            Arc::new(AgentController::new(agent, None, Arc::new(()), kernel.entity_stores()));
        kinds.build(agent, Arc::new(PersonalityController::new(personality)), participant).await
    }

    /// An actor that carries out `/help` itself.
    #[derive(Debug)]
    struct Helping(ActorId);

    #[async_trait]
    impl Actor for Helping {
        fn id(&self) -> ActorId {
            self.0
        }

        fn name(&self) -> Name {
            Name::new("helping").unwrap()
        }

        fn event_filter(&self) -> fn(&Envelope) -> bool {
            |_| true
        }

        async fn run(&self, _: Box<dyn EventReceiver>, _: Box<dyn EventSender>) -> Result<()> {
            Ok(())
        }

        fn state(&self) -> ActorState {
            ActorState::Idle
        }

        fn commands(&self) -> Vec<Command> {
            let help = HELP_COMMAND_NAME.to_string();
            vec![Command {
                name: help,
                description: String::new(),
                usage: None,
                owners_only: false,
            }]
        }

        fn stop(&self) {}

        fn leave(&self) {}
    }

    /// An agent of a kind the program runs none of cannot take part: its
    /// actor is refused, saying why (issue 370).
    #[tokio::test]
    async fn an_agent_of_a_kind_nobody_registered_is_refused() {
        let error = built(
            &AgentKinds::default(),
            Personality { kind: PersonalityKind::External, ..make_personality("plain") },
        )
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
        let failing = |why: &'static str| agent_factory(move |_| async move { Err(anyhow!(why)) });
        kinds.register(PersonalityKind::Internal, &[], failing("built as first registered"));
        kinds.register(PersonalityKind::External, &[], failing("built as first registered"));
        kinds.register(PersonalityKind::External, &[], failing("built as registered again"));

        let error = built(
            &kinds,
            Personality { kind: PersonalityKind::External, ..make_personality("plain") },
        )
        .await
        .expect_err("the stand-in builds nothing");
        assert_eq!(error.to_string(), "built as registered again", "replaced");
        let error = built(&kinds, make_personality("plain"))
            .await
            .expect_err("the stand-in builds nothing");
        assert_eq!(error.to_string(), "built as first registered", "by its own kind");
        assert_eq!(names(kinds.facets(PersonalityKind::External)), Some(Vec::new()));
    }

    /// Issue 37: an agent's command of the name of a command of its session would be hidden by it -
    /// its actor is refused, by the kernel, whatever its kind; where the session has no such
    /// command, it is built.
    #[tokio::test]
    async fn an_agent_whose_command_a_command_of_the_session_hides_is_refused() {
        let mut kinds = AgentKinds::default();
        let helping = agent_factory(|runtime| async move {
            Ok(Box::new(Helping(runtime.actor_id())) as Box<dyn Actor>)
        });
        kinds.register(PersonalityKind::Internal, &[], helping);
        let help = |policy| {
            let help = ComponentImpl {
                id: "help".to_string(),
                fail_policy: Default::default(),
                facet: Arc::new(HelpCommandProvider) as Arc<dyn ChannelCommandProvider>,
            };
            CommandExecutor::new(policy, vec![help]).unwrap()
        };

        let error = built_with(&kinds, make_personality("plain"), help)
            .await
            .expect_err("hidden by the session's /help");
        assert!(error.to_string().contains("/help"), "{error:#}");
        built(&kinds, make_personality("plain")).await.expect("no session command hides it");
    }
}
