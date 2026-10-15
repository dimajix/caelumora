//! The body of an agent in one channel (issue 355): what both kinds of
//! agent are there alike - the `AgentActor`, whose mind is a model run
//! here, and the `ExternalAgentActor`, whose mind runs elsewhere. Who the
//! agent is in the channel and in whose name it acts, what it holds of the
//! channel (its `AgentChannelState`), how it takes in what it hears there
//! (`intake`), the commands it carries out itself (`commands`), and where
//! its actor stands and what `cancel` and `stop` asked of it (`RunControl`,
//! issue 363). What the two minds do with it is their actors'.

use super::intake::TakenIn;
use super::run_control::RunControl;
use super::Answering;
use crate::agent::state::AgentStateHandle;
use crate::agent::turn::Turn;
use crate::agent::work::AgentWork;
use crate::command::{AgentCommandProvider, CommandExecutor};
use crate::component::{ComponentContainer, ComponentContext, ComponentImpl};
use crate::prompt::ContextProvider;
use crate::controller::AgentController;
use crate::message::PerceiveInProcessor;
use crate::runtime::ActorRuntime;
use crate::transport::{EventReceiver, EventSender};
use crate::view::ViewsFactory;
use anyhow::{Context, Result};
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope};
use caelumora_model::agent::{AgentChannelState, AgentId, Personality};
use caelumora_model::channel::Channel;
use caelumora_model::component::ComponentConfig;
use caelumora_model::context::{KnowledgeRef, ModelProfile, PromptPart};
use caelumora_model::security::SecurityContext;
use caelumora_model::transcript::TranscriptEntry;
use caelumora_model::type_map::TypeKey;
use caelumora_model::value::name::Name;
use caelumora_model::work::Work;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::warn;


/// What an agent's actor does with a change of a message its transcript
/// did not take (issue 236; `AgentBody::take_in`).
pub(super) type HandOver = fn(&mut AgentChannelState, TranscriptEntry);

/// The body of one agent in one channel (issue 355): the agent, in whose
/// name it acts there, its components that hear and frame what it is
/// given, its commands, and its state in the channel.
#[derive(Debug)]
pub(super) struct AgentBody {
    /// The session's controller of the agent, shared with its actors in
    /// the session's other channels (issue 120).
    pub(super) agent: Arc<AgentController>,
    pub(super) runtime: ActorRuntime,
    /// Where it acts and what it is limited to, as it was built; its roles
    /// are taken anew by `security_context` (issue 304).
    context: SecurityContext,
    /// What filters what reaches it (issue 196).
    pub(super) perceive_in_processors: Vec<ComponentImpl<dyn PerceiveInProcessor>>,
    /// What frames what its mind is given (issues 98, 204, 214): exactly one.
    context_provider: ComponentImpl<dyn ContextProvider>,
    /// What it does with a change its transcript did not take: nothing for
    /// an agent run here, which gives its transcript nowhere else; a notice
    /// for an external one (issue 236).
    pub(super) hand_over: HandOver,
    /// The commands it carries out itself (issue 269), from its
    /// personality's components, behind the executor that checks who may
    /// see and ask for them.
    pub(super) commands: CommandExecutor<dyn AgentCommandProvider>,
    /// How its model is given its context, kept with every turn of it; the
    /// default for an agent that runs elsewhere.
    pub(super) profile: ModelProfile,
    /// The agent's own state, shared with its actors in other channels and
    /// sessions (issue 62).
    agent_state: Arc<AgentStateHandle>,
    /// What its turns and its prompt providers navigate (issue 163), as the
    /// agent.
    views: ViewsFactory,
    /// What it was built from, its preconfigured components resolved
    /// (issue 270).
    built_from: ComponentConfig,
    /// Its state in this channel, held by its actor's run loop for as long
    /// as it runs.
    pub(super) state: Mutex<AgentChannelState>,
    /// Where its actor's run loop stands, and what `cancel`, `stop` and
    /// `wakeup` asked of it (issue 363).
    pub(super) control: RunControl,
}

impl AgentBody {
    /// The body of the agent `agent` in `channel`, as its `personality`
    /// makes it, and the components it was built with - those `facets` asks
    /// for (issue 337), with the components of `defaults` as the defaults of
    /// the singles they provide - for its actor to take its own from. Its
    /// state in the channel is made fit to run from by `resume`, as its
    /// actor resumes, and its run control stands where that state says. Its
    /// commands' turns have the default profile, and it hands nothing over
    /// (`with_profile`, `handing_over`).
    ///
    /// # Errors
    /// If its components cannot be built, its commands collide
    /// (`command_executor`), or its state cannot be loaded.
    pub(super) async fn build(
        channel: &Channel,
        agent: Arc<AgentController>,
        personality: &Personality,
        facets: &[TypeKey],
        defaults: &ComponentConfig,
        runtime: ActorRuntime,
        resume: impl FnOnce(&mut AgentChannelState),
    ) -> Result<(Self, ComponentContainer)> {
        let attributes = agent.load().security_attributes.clone();
        // It acts on behalf of its owner, in this channel (issue 87), with
        // its owner's roles as they are when it is built, limited by its
        // personality (issue 304); `security_context` takes them anew.
        let context = agent.security_context(channel.session_id, channel.id, personality.limiting_roles());
        let components = ComponentContainer::with_defaults(
            &personality.components,
            defaults,
            facets,
            runtime.component_registry(),
            // Its components get nothing of its runtime (issue 128); what they
            // read of the session they read through their work (issue 163).
            &ComponentContext::new(context.clone(), runtime.services().clone()),
        )
        .await?;
        let perceive_in_processors = components.get_all::<dyn PerceiveInProcessor>(None)?;
        let context_provider = components.one::<dyn ContextProvider>().await?;
        let commands = Self::command_executor(&components, &runtime)?;
        let mut state = runtime.load_channel_state().await?;
        resume(&mut state);
        let control = RunControl::new(state.processing_state, state.turn.as_ref().map(|t| t.work.turn_id));
        let agent_state = runtime.agent_state(attributes).await?;
        let views = runtime.views();
        let body = Self {
            agent,
            runtime,
            context,
            perceive_in_processors,
            context_provider,
            hand_over: |_, _| {},
            commands,
            profile: ModelProfile::default(),
            agent_state,
            views,
            built_from: components.components(),
            state: Mutex::new(state),
            control,
        };
        Ok((body, components))
    }

    /// This body, whose commands' turns are given their context as `profile`
    /// says: its model's.
    pub(super) fn with_profile(self, profile: ModelProfile) -> Self {
        Self { profile, ..self }
    }

    /// This body, handing a change its transcript did not take to
    /// `hand_over` (issue 236).
    pub(super) fn handing_over(self, hand_over: HandOver) -> Self {
        Self { hand_over, ..self }
    }

    /// The agent's id.
    pub(super) fn agent_id(&self) -> AgentId {
        self.agent.id()
    }

    /// The agent as an actor.
    pub(super) fn id(&self) -> ActorId {
        ActorId::Agent(self.agent.id())
    }

    /// The agent's name as it is now.
    pub(super) fn name(&self) -> Name {
        self.agent.load().name.name.clone()
    }

    /// The context it acts in, here: its owner's delegate, with its owner's
    /// roles as they are now and limited as it was built (issue 304).
    pub(super) fn security_context(&self) -> SecurityContext {
        self.agent.security_context(self.context.session_id, self.context.channel_id, self.context.limiting_roles.as_deref())
    }

    /// What it was built from, its preconfigured components resolved (issue
    /// 270).
    pub(super) fn components(&self) -> ComponentConfig {
        self.built_from.clone()
    }

    /// The turn under way in `state`, as its steps work with it; `None`
    /// between turns.
    pub(super) fn turn<'s>(&self, state: &'s mut AgentChannelState) -> Option<Turn<'s>> {
        Turn::of(self.agent_id(), self.agent.name(), state, Arc::clone(&self.agent_state), self.views.clone())
    }

    /// `work` as the agent's components read it, with its `state` in the
    /// channel - for work that is no turn of `state`, as an external agent's
    /// answer is.
    pub(super) fn work<'a>(&'a self, work: &'a Work, state: &'a AgentChannelState) -> AgentWork<'a> {
        AgentWork::new(self.agent_id(), work, state, &self.agent_state, &self.views)
    }

    /// The prompt parts and the knowledge they hold that its context
    /// provider assembles for `work`, whatever its fail policy: without a
    /// context there is nothing to give the mind. Parts that may be left
    /// out are the prompt providers', each by its own policy.
    ///
    /// # Errors
    /// If the context provider cannot assemble them.
    pub(super) async fn current_context(&self, work: &AgentWork<'_>) -> Result<(Vec<PromptPart>, Vec<KnowledgeRef>)> {
        self.context_provider.current_context(work).await
            .with_context(|| format!("component '{}' could not assemble the context", self.context_provider.id))
    }

    /// Stores its `state` in the channel, with where its actor stands now.
    /// A failure is logged: the actor carries on with the state in memory.
    pub(super) async fn store(&self, state: &mut AgentChannelState) {
        state.processing_state = self.control.state();
        if let Err(e) = self.runtime.store_channel_state(state).await {
            warn!("Agent {} cannot store its state in channel {}: {:#}", self.agent_id(), self.context.channel_id, e);
        }
    }

    /// Tells the users where its actor stands, if it moved since it last
    /// told them (issue 243).
    pub(super) async fn announce_state(&self, output: &dyn EventSender) {
        if let Some(now) = self.control.unannounced() {
            self.announce(now, output).await;
        }
    }

    /// Takes in `received` and everything queued behind it (`take_in`),
    /// and carries out every command where it came in it (issue 269), then
    /// takes in the rest. An awake agent is shown carrying it out meanwhile,
    /// and is idle again after - told with its actor's next step; a sleeping
    /// one carries it out asleep (issue 299): moved, it would be neither
    /// woken by `/wakeup` nor left out by the moderator. Returns what was
    /// taken in, without the commands.
    pub(super) async fn hear(&self, state: &mut AgentChannelState, received: Envelope, rx: &mut Box<dyn EventReceiver>, output: &dyn EventSender) -> TakenIn {
        let mut taken = self.take_in(state, received, rx).await;
        while let Some(command) = taken.command.take() {
            let awake = self.control.state() == ActorState::Idle;
            if awake {
                self.control.go_to(ActorState::CarryingOutCommand);
                self.announce_state(output).await;
            }
            self.carry_out(state, &command, output).await;
            if awake {
                self.control.go_to(ActorState::Idle);
            }
            let Some(next) = rx.try_recv() else { break };
            taken = taken.then(self.take_in(state, next, rx).await);
        }
        taken
    }

    /// Tells the users that its actor stands at `state` (issue 243) - in
    /// its own contexts: the state is the agent's, not a turn's. Not sent
    /// is only logged.
    pub(super) async fn announce(&self, state: ActorState, output: &dyn EventSender) {
        let own = Answering::own(self.id(), self.name(), &self.security_context());
        if let Err(e) = own.send(ActorEvent::StateChanged(state), output).await {
            warn!("{:#}", e);
        }
    }
}


#[cfg(test)]
mod testing {
    use super::*;
    use crate::prompt::external_context_provider::ExternalContextProvider;
    use crate::runtime::ChannelRuntime;
    use crate::session::testing::test_runtime;
    use caelumora_model::agent::{Agent, PersonalityId};
    use caelumora_model::scope::Scope;
    use caelumora_model::session::{ChannelId, SessionId};
    use caelumora_model::value::name::ScopedName;

    impl AgentBody {
        /// The body of an agent in a channel outside a session, perceiving
        /// through `processors`: for tests of what it takes in.
        pub(in crate::actor) fn detached(processors: Vec<ComponentImpl<dyn PerceiveInProcessor>>) -> Self {
            let runtime = test_runtime();
            let agent = Agent::new(ScopedName::new("agent", Scope::GlobalScope).unwrap_or_else(|e| panic!("{e}")), PersonalityId::new(), ActorId::None);
            let agent = Arc::new(AgentController::new(agent, None, Arc::new(()), runtime.entity_stores()));
            let commands = CommandExecutor::empty(Arc::clone(runtime.access_policy()));
            let (session_id, channel_id) = (SessionId::new(), ChannelId::new());
            let channel = ChannelRuntime::outside_a_session(Arc::clone(&runtime), Arc::new(CommandExecutor::empty(Arc::clone(runtime.access_policy()))), session_id);
            let actor_runtime = channel.new_actor_runtime(ActorId::Agent(agent.id()), session_id, channel_id);
            Self {
                context: agent.security_context(session_id, channel_id, None),
                agent,
                views: ViewsFactory::of_actor(actor_runtime.clone()),
                runtime: actor_runtime,
                perceive_in_processors: processors,
                context_provider: ComponentImpl { id: "context".to_string(), fail_policy: Default::default(), facet: Arc::new(ExternalContextProvider { prompt_providers: Vec::new() }) },
                hand_over: |_, _| {},
                commands,
                profile: ModelProfile::default(),
                agent_state: AgentStateHandle::detached(),
                built_from: ComponentConfig::default(),
                state: Mutex::new(AgentChannelState::default()),
                control: RunControl::new(ActorState::Idle, None),
            }
        }
    }
}
