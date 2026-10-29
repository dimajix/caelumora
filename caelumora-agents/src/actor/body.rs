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
use crate::command::AgentCommandProvider;
use crate::message::PerceiveInProcessor;
use crate::prompt::{ContextProvider, ModelProfileProvider};
use crate::situation::Situation;
use crate::turn::Turn;
use anyhow::{Context, Result};
use caelumora_model::actor::{
    ActorDescription, ActorEvent, ActorId, ActorState, Envelope, RetryTarget,
};
use caelumora_model::agent::{AgentChannelState, AgentId, HearsDuringTurn, RetryRefused};
use caelumora_model::component::ComponentConfig;
use caelumora_model::confidentiality::Visibility;
use caelumora_model::context::{KnowledgeRef, ModelProfile, PromptPart};
use caelumora_model::security::SecurityContext;
use caelumora_model::transcript::TranscriptEntry;
use caelumora_model::type_map::TypeKey;
use caelumora_model::value::name::Name;
use caelumora_model::work::Work;
use caelumora_runtime::actor::Answering;
use caelumora_runtime::agent::state::AgentStateHandle;
use caelumora_runtime::command::CommandExecutor;
use caelumora_runtime::component::{ComponentContainer, ComponentContext, ComponentImpl};
use caelumora_runtime::controller::{AgentController, PersonalityController};
use caelumora_runtime::runtime::ActorRuntime;
use caelumora_runtime::transport::{EventReceiver, EventSender};
use futures::future::BoxFuture;
use std::future::ready;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::warn;

/// What an agent's actor does with a change of a message its transcript
/// did not take (issue 236; `AgentBody::take_in`), given its runtime to read
/// what lies beyond what its state holds (issue 380).
pub(super) type HandOver =
    for<'a> fn(&'a ActorRuntime, &'a mut AgentChannelState, TranscriptEntry) -> BoxFuture<'a, ()>;

/// Hands over nothing: what an agent run here does, which gives its
/// transcript nowhere else.
fn hand_over_nothing<'a>(
    _: &'a ActorRuntime,
    _: &'a mut AgentChannelState,
    _: TranscriptEntry,
) -> BoxFuture<'a, ()> {
    Box::pin(ready(()))
}

/// The body of one agent in one channel (issue 355): the agent, in whose
/// name it acts there, its components that hear and frame what it is
/// given, its commands, and its state in the channel.
#[derive(Debug)]
pub(super) struct AgentBody {
    /// The session's controller of the agent, shared with its actors in
    /// the session's other channels (issue 120).
    pub(super) agent: Arc<AgentController>,
    /// The instance's controller of its personality (issue 406), what it
    /// answers by as it runs - what it is limited to, who sees its
    /// reasoning. Its components stay as they were built from it.
    personality: Arc<PersonalityController>,
    pub(super) runtime: ActorRuntime,
    /// Where it acts, as it was built; its roles and what it is limited to
    /// are taken anew by `security_context` (issues 304, 406).
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
    /// How its model is given its context, kept with every turn of it - its
    /// model's, or its remote agent's (`ModelProfileProvider`).
    pub(super) profile: ModelProfile,
    /// The agent's own state, shared with its actors in other channels and
    /// sessions (issue 62).
    agent_state: Arc<AgentStateHandle>,
    /// What it was built from, the component bundles it includes resolved
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
    /// The body of the agent `agent` in `channel`, as the personality
    /// `personality` holds makes it, and the components it was built with - those `facets` asks
    /// for (issue 337), with the components of `defaults` as the defaults of
    /// the singles they provide - for its actor to take its own from. Its
    /// state in the channel is made fit to run from by `resume`, as its
    /// actor resumes, and its run control stands where that state says. It
    /// hands nothing over (`handing_over`).
    ///
    /// # Errors
    /// If its components cannot be built, its commands collide
    /// (`command_executor`), or its state cannot be loaded.
    pub(super) async fn build(
        agent: Arc<AgentController>,
        personality: Arc<PersonalityController>,
        facets: &[TypeKey],
        defaults: &ComponentConfig,
        runtime: ActorRuntime,
        resume: impl FnOnce(&mut AgentChannelState),
    ) -> Result<(Self, ComponentContainer)> {
        let attributes = agent.load().security_attributes.clone();
        // It acts on behalf of its owner, in this channel (issue 87), with
        // its owner's roles as they are when it is built, limited by its
        // personality (issue 304); `security_context` takes them anew.
        let channel = runtime.channel_controller().load();
        let context = agent.security_context(
            channel.session_id,
            channel.id,
            personality.load().limiting_roles(),
        );
        let components = ComponentContainer::with_defaults(
            &personality.load().components,
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
        let profile = components.one::<dyn ModelProfileProvider>().await?.profile();
        let commands = Self::command_executor(&components, &runtime)?;
        let mut state = runtime.load_channel_state().await?;
        resume(&mut state);
        let control =
            RunControl::new(state.processing_state, state.turn.as_ref().map(|t| t.work.turn_id));
        let agent_state = runtime.agent_state(attributes).await?;
        let body = Self {
            agent,
            personality,
            runtime,
            context,
            perceive_in_processors,
            context_provider,
            hand_over: hand_over_nothing,
            commands,
            profile,
            agent_state,
            built_from: components.components(),
            state: Mutex::new(state),
            control,
        };
        Ok((body, components))
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
    /// roles as they are now, limited as its personality is now (issues 304,
    /// 406).
    pub(super) fn security_context(&self) -> SecurityContext {
        self.agent.security_context(
            self.context.session_id,
            self.context.channel_id,
            self.personality.load().limiting_roles(),
        )
    }

    /// What of it is public in its channel (issue 403): its name as it is
    /// now, introduced as its personality says now.
    pub(super) fn describe(&self) -> ActorDescription {
        ActorDescription {
            id: self.id(),
            name: self.name(),
            introduction: self.personality.load().introduction.clone(),
        }
    }

    /// Who sees its reasoning at least (issues 61, 133), as its
    /// personality says now (issue 406).
    pub(super) fn reasoning_visibility(&self) -> Visibility {
        self.personality.load().reasoning_visibility
    }

    /// Whose messages it takes in between the steps of a turn (issue 431),
    /// as its personality says now.
    pub(super) fn hears_during_turn(&self) -> HearsDuringTurn {
        self.personality.load().hears_during_turn
    }

    /// What it was built from, the component bundles it includes resolved
    /// (issue 270).
    pub(super) fn components(&self) -> ComponentConfig {
        self.built_from.clone()
    }

    /// The turn under way in `state`, as its steps work with it; `None`
    /// between turns.
    pub(super) fn turn<'s>(&self, state: &'s mut AgentChannelState) -> Option<Turn<'s>> {
        Turn::of(
            self.agent_id(),
            self.agent.name(),
            state,
            Arc::clone(&self.agent_state),
            self.runtime.clone(),
        )
    }

    /// The contexts `request` is answered in, continued in the agent's name
    /// and with its rights (`Answering::to`), and the work that answers it:
    /// asked for as the request says (issue 439), bound by the channel as it
    /// is now - its classification (issue 212), and its session's time zone
    /// the times of messages are given in (issue 424). How its model is
    /// given its context is the answerer's to add (`Work::given_as`).
    pub(super) fn work_for(&self, request: &Envelope) -> (Answering, Work) {
        let answering = Answering::to(request, self.id(), self.name(), &self.security_context());
        let work = Work::new(
            answering.event_context.clone(),
            request.event.request_reason(),
            answering.security_context.clone(),
            self.runtime.channel_classification(),
            self.runtime.session_time_zone(),
        );
        (answering, work)
    }

    /// The situation of `work`, as the agent's components read it, with its
    /// `state` in the channel - for work that is no turn of `state`, as an
    /// external agent's answer is.
    pub(super) fn situation<'a>(
        &'a self,
        work: &'a Work,
        state: &'a AgentChannelState,
    ) -> Situation<'a> {
        Situation::new(self.agent_id(), work, state, &self.agent_state, &self.runtime)
    }

    /// The prompt parts and the knowledge they hold that its context
    /// provider assembles for `situation`, whatever its fail policy: without a
    /// context there is nothing to give the mind. Parts that may be left
    /// out are the prompt providers', each by its own policy.
    ///
    /// # Errors
    /// If the context provider cannot assemble them.
    pub(super) async fn current_context(
        &self,
        situation: &Situation<'_>,
    ) -> Result<(Vec<PromptPart>, Vec<KnowledgeRef>)> {
        self.context_provider.current_context(situation).await.with_context(|| {
            format!("component '{}' could not assemble the context", self.context_provider.id)
        })
    }

    /// Stores its `state` in the channel, with where its actor stands now.
    /// A failure is logged: the actor carries on with the state in memory.
    pub(super) async fn store(&self, state: &mut AgentChannelState) {
        state.processing_state = self.control.state();
        if let Err(e) = self.runtime.store_channel_state(state).await {
            warn!(
                "Agent {} cannot store its state in channel {}: {:#}",
                self.agent_id(),
                self.context.channel_id,
                e
            );
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
    pub(super) async fn hear(
        &self,
        state: &mut AgentChannelState,
        received: Envelope,
        rx: &mut Box<dyn EventReceiver>,
        output: &dyn EventSender,
    ) -> TakenIn {
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
            let Some(next) = rx.try_recv().await else { break };
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

    /// Tells its channel what the agent recorded since it last did - what
    /// it was given to read, a compaction of its context (issue 417) -, in
    /// its own contexts, for the channel's history to hold what it saw
    /// where it saw it. Not sent is only logged: the transcript holds it.
    pub(super) async fn tell_recorded(
        &self,
        state: &mut AgentChannelState,
        output: &dyn EventSender,
    ) {
        let told = state.take_told();
        if told.is_empty() {
            return;
        }
        Answering::own(self.id(), self.name(), &self.security_context())
            .send_all(told, output)
            .await;
    }
}

impl AgentBody {
    /// Tells whoever asked each of `retries` that the agent does not answer
    /// again, for `reason` (issue 234) - in the contexts of each request,
    /// as an answer to it, in the agent's name and with its rights. One that
    /// cannot be sent is logged.
    pub(super) async fn reject_retries(
        &self,
        retries: Vec<Envelope>,
        reason: RetryRefused,
        output: &dyn EventSender,
    ) {
        let own = self.id();
        let name = self.name();
        let acting = self.security_context();
        for retry in retries {
            let Some(target) = retry_target(&retry) else { continue };
            let rejected = ActorEvent::RetryRejected { target, reason };
            if let Err(e) =
                Answering::to(&retry, own, name.to_string(), &acting).send(rejected, output).await
            {
                warn!("Agent {} cannot reject a request to answer again: {:#}", own, e);
            }
        }
    }
}

/// What the request to answer again in `retry` is about (issue 235);
/// `None` for any other event.
pub(super) fn retry_target(retry: &Envelope) -> Option<RetryTarget> {
    match retry.event {
        ActorEvent::RetryRequested { target, .. } => Some(target),
        _ => None,
    }
}

#[cfg(test)]
mod testing {
    use super::*;
    use crate::prompt::external_context_provider::ExternalContextProvider;
    use caelumora_model::agent::testing::make_personality;
    use caelumora_model::agent::{Agent, Personality};
    use caelumora_model::channel::testing::bare_channel;
    use caelumora_model::scope::Scope;
    use caelumora_model::session::testing::bare_session;
    use caelumora_model::session::SessionId;
    use caelumora_model::user::UserId;
    use caelumora_model::value::name::ScopedName;
    use caelumora_runtime::runtime::ChannelRuntime;
    use caelumora_runtime::session::testing::test_runtime;

    impl AgentBody {
        /// The body of an agent in a channel outside a session, perceiving
        /// through `processors`: for tests of what it takes in.
        pub(in crate::actor) fn detached(
            processors: Vec<ComponentImpl<dyn PerceiveInProcessor>>,
        ) -> Self {
            let runtime = test_runtime();
            let personality = make_personality("plain");
            let agent = Agent::new(
                ScopedName::new("agent", Scope::GlobalScope).unwrap_or_else(|e| panic!("{e}")),
                personality.id,
                ActorId::User(UserId::new()),
            );
            let agent =
                Arc::new(AgentController::new(agent, None, Arc::new(()), runtime.entity_stores()));
            let commands = CommandExecutor::empty(Arc::clone(runtime.access_policy()));
            let session = bare_session(SessionId::new());
            let channel = bare_channel(session.id);
            let (session_id, channel_id) = (session.id, channel.id);
            let channel = ChannelRuntime::outside_a_session(Arc::clone(&runtime), session, channel);
            let actor_runtime = channel.new_actor_runtime(ActorId::Agent(agent.id()));
            Self {
                context: agent.security_context(session_id, channel_id, None),
                agent,
                personality: Arc::new(PersonalityController::new(personality)),
                runtime: actor_runtime,
                perceive_in_processors: processors,
                context_provider: ComponentImpl {
                    id: "context".to_string(),
                    fail_policy: Default::default(),
                    facet: Arc::new(ExternalContextProvider { prompt_providers: Vec::new() }),
                },
                hand_over: hand_over_nothing,
                commands,
                profile: ModelProfile::default(),
                agent_state: AgentStateHandle::detached(),
                built_from: ComponentConfig::default(),
                state: Mutex::new(AgentChannelState::default()),
                control: RunControl::new(ActorState::Idle, None),
            }
        }

        /// This body, of a personality that hears between the steps of a
        /// turn as `hears` says (issue 431).
        pub(in crate::actor) fn hearing(self, hears: HearsDuringTurn) -> Self {
            let personality = Personality {
                hears_during_turn: hears,
                ..Personality::clone(&self.personality.load())
            };
            Self { personality: Arc::new(PersonalityController::new(personality)), ..self }
        }
    }
}
