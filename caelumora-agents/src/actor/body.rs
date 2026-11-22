//! The body of an agent in one channel (issue 355): what both kinds of agent are there alike - the
//! `AgentActor`, whose mind is a model run here, and the `ExternalAgentActor`, whose mind runs
//! elsewhere. Who the agent is in the channel and the runtime it asks the kernel through - which
//! builds in whose name it acts (issue 516) -, what it holds of the channel (its
//! `AgentChannelState`), how it takes in what it hears there (`intake`), the commands it carries
//! out itself (`commands`), and where its actor stands and what `cancel` and `stop` asked of it
//! (`RunControl`, issue 363). What the two minds do with it is their actors'.

use super::intake::TakenIn;
use super::run_control::RunControl;
use crate::command::AgentCommand;
use crate::message::PerceiveInProcessor;
use crate::prompt::ContextProvider;
use crate::situation::Situation;
use crate::turn::Turn;
use anyhow::{Context, Result};
use caelumora_api::actor::Answering;
use caelumora_api::command::CommandProvider;
use caelumora_api::command::CommandSet;
use caelumora_api::component::ComponentContainer;
use caelumora_api::component::ComponentImpl;
use caelumora_api::model::ModelProfileProvider;
use caelumora_api::runtime::ActorRuntime;
use caelumora_api::transport::{EventReceiver, EventSender};
use caelumora_domain::actor::{
    ActorDescription, ActorEvent, ActorId, ActorState, Envelope, RetryTarget,
};
use caelumora_domain::agent::{AgentChannelState, AgentId, HearsDuringTurn, RetryRefused};
use caelumora_domain::channel::ThreadId;
use caelumora_domain::component::ComponentConfig;
use caelumora_domain::confidentiality::Visibility;
use caelumora_domain::context::{KnowledgeRef, ModelProfile, PromptPart};
use caelumora_domain::transcript::TranscriptEntry;
use caelumora_domain::turn::TurnState;
use caelumora_domain::type_map::TypeKey;
use caelumora_domain::value::name::Name;
use caelumora_domain::work::{TurnId, Work};
use futures::future::BoxFuture;
use std::collections::BTreeSet;
use std::future::ready;
use std::sync::{self, Arc};
use tokio::sync::Mutex;
use tracing::warn;

/// What an agent's actor does with a change of a message its transcript
/// did not take (issue 236; `AgentBody::take_in`), given its runtime to read
/// what lies beyond what its state holds (issue 380).
pub(super) type HandOver = for<'a> fn(
    &'a dyn ActorRuntime,
    &'a mut AgentChannelState,
    TranscriptEntry,
) -> BoxFuture<'a, ()>;

/// Hands over nothing: what an agent run here does, which gives its
/// transcript nowhere else.
fn hand_over_nothing<'a>(
    _: &'a dyn ActorRuntime,
    _: &'a mut AgentChannelState,
    _: TranscriptEntry,
) -> BoxFuture<'a, ()> {
    Box::pin(ready(()))
}

/// The agent's own join or leave of a thread (issue 602), as it takes it in: what changes the
/// threads it follows (`AgentBody::follow`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OwnChange {
    Joined(ThreadId),
    Left(ThreadId),
}

/// The body of one agent in one channel (issue 355): its runtime, its
/// components that hear and frame what it is given, its commands, and its
/// state in the channel.
#[derive(Debug)]
pub(super) struct AgentBody {
    /// What it asks the kernel through (issue 516): who the agent and its personality are as they
    /// are now - how it is introduced, who sees its reasoning (issue 406) -, its state in its
    /// channel, and what it asks of its channel and of the system services bound to its work.
    /// Its components stay as they were built from its personality.
    pub(super) runtime: Arc<dyn ActorRuntime>,
    /// What filters what reaches it (issue 196).
    pub(super) perceive_in_processors: Vec<ComponentImpl<dyn PerceiveInProcessor>>,
    /// What frames what its mind is given (issues 98, 204, 214): exactly one.
    context_provider: ComponentImpl<dyn ContextProvider>,
    /// What it does with a change its transcript did not take: nothing for
    /// an agent run here, which gives its transcript nowhere else; a notice
    /// for an external one (issue 236).
    pub(super) hand_over: HandOver,
    /// The commands it carries out itself (issue 269), from its
    /// personality's components; who may see and ask for them is the
    /// channel's to decide (issue 516).
    pub(super) commands: CommandSet<dyn AgentCommand>,
    /// How its model is given its context, kept with every turn of it - its
    /// model's, or its remote agent's (`ModelProfileProvider`).
    pub(super) profile: ModelProfile,
    /// What it was built from, the component bundles it includes resolved
    /// (issue 270).
    built_from: ComponentConfig,
    /// Its state in this channel, held by its actor's run loop for as long
    /// as it runs.
    pub(super) state: Mutex<AgentChannelState>,
    /// Where its actor's run loop stands, and what `cancel`, `stop` and
    /// `wakeup` asked of it (issue 363).
    pub(super) control: RunControl,
    /// The threads it follows, as far as it has taken in (issue 602): those its runtime named and
    /// its state held as it was built, then changed as it takes in its own join or leave of a
    /// thread (`AgentBody::follow`). What reaches it of a thread goes on that thread's branch from
    /// its join on up to its leave, also what was delivered before its leave and waited to be
    /// taken in; the threads its runtime names change already as the channel announces them.
    followed: sync::Mutex<BTreeSet<ThreadId>>,
}

impl AgentBody {
    /// The body of the agent of `runtime` in its channel, as its personality makes it, and the
    /// components it was built with - those `facets` asks for (issue 337), with the components of
    /// `defaults` as the defaults of the singles they provide - for its actor to take its own from.
    /// Its state in the channel is made fit to run from by `resume`, as its actor resumes, and its
    /// run control stands where that state says. It hands nothing over (`handing_over`).
    ///
    /// # Errors
    /// If its components cannot be built, two of them provide the same command, or its state
    /// cannot be loaded.
    pub(super) async fn build(
        facets: &[TypeKey],
        defaults: &ComponentConfig,
        runtime: Arc<dyn ActorRuntime>,
        resume: impl FnOnce(&mut AgentChannelState),
    ) -> Result<(Self, ComponentContainer)> {
        // Its components get nothing of its runtime (issue 128); what they read of the session they
        // read through their work (issue 163). They are built in its name as the kernel holds it
        // (issue 516).
        let components =
            runtime.build_components(&runtime.personality().components, defaults, facets).await?;
        let perceive_in_processors = components.get_all::<dyn PerceiveInProcessor>(None)?;
        let context_provider = components.one::<dyn ContextProvider>().await?;
        let profile = components.one::<dyn ModelProfileProvider>().await?.profile();
        let commands =
            CommandSet::new(components.get_all::<dyn CommandProvider<dyn AgentCommand>>(None)?)?;
        let mut state = runtime.load_channel_state().await?;
        resume(&mut state);
        let control = RunControl::resuming(&state, runtime.joined_thread_id());
        let followed =
            sync::Mutex::new(runtime.threads().union(&state.threads_held()).copied().collect());
        let body = Self {
            runtime,
            perceive_in_processors,
            context_provider,
            hand_over: hand_over_nothing,
            commands,
            profile,
            built_from: components.components(),
            state: Mutex::new(state),
            control,
            followed,
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
        self.runtime.agent().id
    }

    /// The agent as an actor.
    pub(super) fn id(&self) -> ActorId {
        ActorId::Agent(self.agent_id())
    }

    /// The agent's name as it is now.
    pub(super) fn name(&self) -> Name {
        self.runtime.agent().name.name.clone()
    }

    /// The agent's name as it is now, with its scope, as the channel knows it.
    pub(super) fn scoped_name(&self) -> String {
        self.runtime.agent().name.to_string()
    }

    /// What of it is public in its channel (issue 403): its name as it is
    /// now, introduced as its personality says now.
    pub(super) fn describe(&self) -> ActorDescription {
        ActorDescription {
            id: self.id(),
            name: self.name(),
            introduction: self.runtime.personality().introduction.clone(),
        }
    }

    /// Who sees its reasoning at least (issues 61, 133), as its
    /// personality says now (issue 406).
    pub(super) fn reasoning_visibility(&self) -> Visibility {
        self.runtime.personality().reasoning_visibility
    }

    /// Whose messages it takes in between the steps of a turn (issue 431),
    /// as its personality says now.
    pub(super) fn hears_during_turn(&self) -> HearsDuringTurn {
        self.runtime.personality().hears_during_turn
    }

    /// What it was built from, the component bundles it includes resolved
    /// (issue 270).
    pub(super) fn components(&self) -> ComponentConfig {
        self.built_from.clone()
    }

    /// The turn under way in `thread` of `state`, as its steps work with it; `None` between turns
    /// there.
    pub(super) fn turn<'s>(
        &self,
        state: &'s mut AgentChannelState,
        thread: ThreadId,
    ) -> Option<Turn<'s>> {
        Turn::of(state, thread, Arc::clone(&self.runtime))
    }

    /// Begins a turn of `work` in `thread` of `state` (`AgentChannelState::begin_turn`), as the
    /// turn that runs (`RunControl::begin_turn`), whose part its steps work with (`under_way`).
    pub(super) fn begin_turn(
        &self,
        state: &mut AgentChannelState,
        thread: ThreadId,
        work: Work,
    ) -> TurnId {
        let turn_id = state.begin_turn(thread, work);
        self.control.begin_turn(thread, turn_id);
        turn_id
    }

    /// The working state of the turn under way in `state`, in the thread of the turn that runs or
    /// ran last (`RunControl::thread`); `None` between turns.
    pub(super) fn under_way<'s>(&self, state: &'s AgentChannelState) -> Option<&'s TurnState> {
        state.turn(self.control.thread()?)
    }

    /// The thread whose branch of the agent's transcript `envelope` is taken in on, answered on or
    /// carried out on (issue 602): the thread it happens in (`EventContext::thread_id`) if the
    /// agent follows it (`followed`), else the one it joined in
    /// (`ActorRuntime::joined_thread_id`). A nudge, a request to answer again and a command reach
    /// it from any thread, until they are routed by the thread they name; read on a branch of
    /// their own, a request to answer again would be refused and a turn would begin empty.
    pub(super) fn branch_of(&self, envelope: &Envelope) -> ThreadId {
        let happened_in = envelope.context.thread_id;
        if self.lock_followed().contains(&happened_in) {
            happened_in
        } else {
            self.runtime.joined_thread_id()
        }
    }

    /// The agent's own join or leave of a thread that `envelope` announces (issue 602): what
    /// changes the threads it follows (`follow`).
    pub(super) fn own_change(&self, envelope: &Envelope) -> Option<OwnChange> {
        let thread = envelope.context.thread_id;
        let (participant, change) = match envelope.event {
            ActorEvent::ParticipantJoined { participant, .. } => {
                (participant, OwnChange::Joined(thread))
            }
            ActorEvent::ParticipantLeft { participant, .. } => {
                (participant, OwnChange::Left(thread))
            }
            _ => return None,
        };
        (participant == self.id()).then_some(change)
    }

    /// The thread the agent leaves by `change`, if it leaves one (issue 602): one it left and no
    /// longer takes part in (`ActorRuntime::threads`). A leave of the channel ends its actor, and
    /// one read back as it joins the channel again, of a thread it takes part in again, leaves
    /// nothing.
    pub(super) fn leaves(&self, change: Option<OwnChange>) -> Option<ThreadId> {
        match change {
            Some(OwnChange::Left(thread)) => {
                (!self.runtime.threads().contains(&thread)).then_some(thread)
            }
            _ => None,
        }
    }

    /// Follows its own join or leave of a thread, `change`, in `state` and in the threads it
    /// follows (issue 602): its branch of a thread it joined forks from where it stands in the
    /// thread that one hangs at (`ActorRuntime::origin_of`, `AgentChannelState::join_thread`),
    /// and its part and its branch of a thread it leaves go (`leaves`,
    /// `AgentChannelState::leave_thread`), what it kept there for when it joins again.
    pub(super) fn follow(&self, state: &mut AgentChannelState, change: OwnChange) {
        match change {
            OwnChange::Joined(thread) => {
                let from = self.runtime.origin_of(thread).map(|origin| origin.thread);
                state.join_thread(thread, from);
                self.lock_followed().insert(thread);
            }
            OwnChange::Left(_) => {
                if let Some(thread) = self.leaves(Some(change)) {
                    self.lock_followed().remove(&thread);
                    state.leave_thread(thread);
                }
            }
        }
    }

    fn lock_followed(&self) -> sync::MutexGuard<'_, BTreeSet<ThreadId>> {
        self.followed.lock().unwrap_or_else(sync::PoisonError::into_inner)
    }

    /// The context `request` is answered in, continued in the agent's name
    /// (`Answering::to`), and the work that answers it:
    /// asked for as the request says (issue 439), bound by the channel as it
    /// is now - its classification (issue 212), and its session's time zone
    /// the times of messages are given in (issue 424). How its model is
    /// given its context is the answerer's to add (`Work::given_as`).
    pub(super) fn work_for(&self, request: &Envelope) -> (Answering, Work) {
        let answering = Answering::to(request, self.id(), self.name());
        let work = Work::new(
            answering.event_context.clone(),
            request.event.request_reason(),
            self.runtime.channel_classification(),
            self.runtime.session_time_zone(),
        );
        (answering, work)
    }

    /// The situation of `work` on the branch of `thread`, as the agent's components read it, with
    /// its `state` in the channel - for work that is no turn of `state`, as an external agent's
    /// answer is.
    pub(super) fn situation<'a>(
        &'a self,
        work: &'a Work,
        thread: ThreadId,
        state: &'a AgentChannelState,
    ) -> Situation<'a> {
        Situation::new(work, thread, state, &*self.runtime)
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

    /// Stores its `state` in the channel, with where its actor stands now, in the thread it stands
    /// in (`standing_in`). A failure is logged: the actor carries on with the state in memory.
    pub(super) async fn store(&self, state: &mut AgentChannelState) {
        state.thread_mut(self.standing_in()).processing_state = self.control.state();
        if let Err(e) = self.runtime.store_channel_state(state).await {
            warn!(
                "Agent {} cannot store its state in channel {}: {:#}",
                self.agent_id(),
                self.runtime.channel_id(),
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
    /// its own context (`own`), in the thread it stands in (`standing_in`): the state is the
    /// agent's, not a turn's. Not sent is only logged.
    pub(super) async fn announce(&self, state: ActorState, output: &dyn EventSender) {
        let changed = ActorEvent::StateChanged(state);
        if let Err(e) = self.own(self.standing_in()).send(changed, output).await {
            warn!("{:#}", e);
        }
    }

    /// The thread its actor stands in (issue 602): the one of the turn that runs or ran last
    /// (`RunControl::thread`) while it follows that thread, else - before its first turn, or once
    /// it left that thread - the one it counts as joined in.
    pub(super) fn standing_in(&self) -> ThreadId {
        let followed = |thread: &ThreadId| self.lock_followed().contains(thread);
        self.control.thread().filter(followed).unwrap_or_else(|| self.runtime.joined_thread_id())
    }

    /// Tells its channel what the agent recorded since it last did - what it was given to read,
    /// where it stopped reading a part given, a compaction of its context (issues 417, 537) -, in
    /// its own context, each in the thread whose branch it was recorded on (issue 602), for the
    /// channel's history to hold what it saw where it saw it. Not sent is only logged: the
    /// transcript holds it.
    pub(super) async fn tell_recorded(
        &self,
        state: &mut AgentChannelState,
        output: &dyn EventSender,
    ) {
        let told = state.take_told();
        // What was recorded on one branch after another shares a span, as all of it did with one
        // thread.
        for run in told.chunk_by(|(one, _), (other, _)| one == other) {
            let Some((thread, _)) = run.first() else { continue };
            let events = run.iter().map(|(_, event)| event.clone()).collect();
            self.own(*thread).send_all(events, output).await;
        }
    }

    /// The context the agent speaks in of itself in `thread`, nobody having asked: in its channel,
    /// its own initiator (`Answering::own`).
    pub(super) fn own(&self, thread: ThreadId) -> Answering {
        let runtime = &self.runtime;
        let (session, channel) = (runtime.session_id(), runtime.channel_id());
        Answering::own(session, channel, thread, self.id(), self.name())
    }
}

impl AgentBody {
    /// Tells whoever asked each of `retries` that the agent does not answer
    /// again, for `reason` (issue 234) - in the context of each request,
    /// as an answer to it, in the agent's name and with its rights. One that
    /// cannot be sent is logged.
    pub(super) async fn refuse_retries(
        &self,
        retries: Vec<Envelope>,
        reason: RetryRefused,
        output: &dyn EventSender,
    ) {
        let own = self.id();
        let name = self.name();
        for retry in retries {
            let Some(target) = retry_target(&retry) else { continue };
            let refused = ActorEvent::RetryRefused { target, reason };
            if let Err(e) = Answering::to(&retry, own, name.to_string()).send(refused, output).await
            {
                warn!("Agent {} cannot refuse a request to answer again: {:#}", own, e);
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
    use crate::prompt::external_context::ExternalContextProvider;
    use crate::testing::{agent_runtime, detached_actor_runtime, detached_participant};
    use caelumora_domain::agent::Personality;
    use caelumora_kernel::kernel::AgentRuntime;

    /// What the tests make of an agent's body past its own API.
    impl AgentBody {
        /// The body of an agent in a channel outside a session, perceiving
        /// through `processors`: for tests of what it takes in.
        pub(in crate::actor) fn detached(
            processors: Vec<ComponentImpl<dyn PerceiveInProcessor>>,
        ) -> Self {
            Self {
                runtime: detached_actor_runtime(AgentId::new()),
                perceive_in_processors: processors,
                context_provider: ComponentImpl {
                    id: "context".to_string(),
                    fail_policy: Default::default(),
                    facet: Arc::new(ExternalContextProvider { prompt_providers: Vec::new() }),
                },
                hand_over: hand_over_nothing,
                commands: CommandSet::empty(),
                profile: ModelProfile::default(),
                built_from: ComponentConfig::default(),
                state: Mutex::new(AgentChannelState::default()),
                control: RunControl::new(ActorState::Idle, None),
                followed: sync::Mutex::new(BTreeSet::new()),
            }
            .following_its_threads()
        }

        /// This body, following the threads its runtime names.
        fn following_its_threads(self) -> Self {
            let followed = sync::Mutex::new(self.runtime.threads());
            Self { followed, ..self }
        }

        /// This body, of a personality that hears between the steps of a
        /// turn as `hears` says (issue 431).
        pub(in crate::actor) fn hearing(self, hears: HearsDuringTurn) -> Self {
            let personality = Personality {
                hears_during_turn: hears,
                ..Personality::clone(&self.runtime.personality())
            };
            let runtime = detached_participant(self.agent_id());
            let runtime = AgentRuntime::in_test(runtime).with_personality(personality);
            Self { runtime: Arc::new(runtime), ..self }
        }

        /// This body, its agent taking part in `thread` as well as in the one it joined in
        /// (issue 602), with a runtime of its own: what `hearing` gave it before is gone.
        pub(in crate::actor) fn taking_part_also_in(self, thread: ThreadId) -> Self {
            let runtime = detached_participant(self.agent_id()).taking_part_also_in(thread);
            Self { runtime: agent_runtime(runtime), ..self }.following_its_threads()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::actor::EventContext;
    use caelumora_domain::chain::Position;
    use caelumora_domain::confidentiality::Classification;
    use caelumora_domain::message::MessageId;
    use caelumora_domain::value::time_zone::TimeZone;
    use caelumora_kernel::transport::InMemoryEventSender;
    use chrono::Utc;
    use tokio::sync::mpsc;

    /// Issue 602: what happens in a thread the agent takes part in goes on that thread's branch,
    /// what happens in another on the branch of the one it joined in.
    #[test]
    fn an_envelope_goes_on_the_branch_of_its_thread_where_the_agent_takes_part() {
        let (joined, there, elsewhere) = (ThreadId::default(), ThreadId::new(), ThreadId::new());
        let body = AgentBody::detached(Vec::new()).taking_part_also_in(there);
        let in_thread = |thread_id| {
            let context = EventContext { thread_id, ..EventContext::default() };
            Envelope::new(context, ActorEvent::WakeupRequested)
        };
        assert_eq!(body.branch_of(&in_thread(joined)), joined, "the one it joined in");
        assert_eq!(body.branch_of(&in_thread(there)), there, "a further one");
        assert_eq!(body.branch_of(&in_thread(elsewhere)), joined, "one it takes no part in");
    }

    /// Issue 602: what the agent recorded is told in the thread of the branch it was recorded on,
    /// what was recorded on one branch after another in one span.
    #[tokio::test]
    async fn what_it_recorded_is_told_in_the_thread_of_its_branch() {
        let (joined, there) = (ThreadId::default(), ThreadId::new());
        let body = AgentBody::detached(Vec::new()).taking_part_also_in(there);
        let mut state = AgentChannelState::default();
        let withdrawn =
            |_| ActorEvent::MessageWithdrawn { message: MessageId::new(), at: Utc::now() };
        state.to_tell = vec![(joined, withdrawn(1)), (joined, withdrawn(2)), (there, withdrawn(3))];
        let (tx, mut rx) = mpsc::channel(8);
        body.tell_recorded(&mut state, &InMemoryEventSender::new(tx)).await;

        let mut told = Vec::new();
        while let Ok(envelope) = rx.try_recv() {
            told.push((envelope.context.thread_id, envelope.context.span_id));
        }
        let threads: Vec<_> = told.iter().map(|(thread, _)| *thread).collect();
        assert_eq!(threads, [joined, joined, there], "each in its thread, in order");
        assert_eq!(told[0].1, told[1].1, "one branch's in one span");
        assert_ne!(told[1].1, told[2].1, "another's in a span of its own");
        assert!(state.to_tell.is_empty(), "taken");
    }

    /// Issue 602: an agent's state is stored with where its actor stands in the thread of its turn,
    /// and before its first turn in the thread it joined in.
    #[tokio::test]
    async fn where_it_stands_is_stored_in_the_thread_of_its_turn() {
        let (joined, there) = (ThreadId::default(), ThreadId::new());
        let body = AgentBody::detached(Vec::new()).taking_part_also_in(there);
        let mut state = AgentChannelState::default();
        body.control.go_to(ActorState::Sleeping);
        body.store(&mut state).await;
        let standing = |state: &AgentChannelState, thread| {
            state.thread(thread).map(|part| part.processing_state)
        };
        assert_eq!(standing(&state, joined), Some(ActorState::Sleeping), "before its first turn");

        let work = Work::new(EventContext::default(), None, Classification::PUBLIC, TimeZone::UTC);
        body.begin_turn(&mut state, there, work);
        body.control.go_to(ActorState::Planning);
        body.store(&mut state).await;
        assert_eq!(standing(&state, there), Some(ActorState::Planning), "in its turn's thread");
        assert_eq!(standing(&state, joined), Some(ActorState::Sleeping), "not where it joined");
    }

    /// Issue 602: let go from the thread of its last turn, the agent stands in the one it counts as
    /// joined in, and nothing of the thread it left comes back as its state is stored; a leave of a
    /// thread it takes part in again - one read back as it joins the channel again - leaves what
    /// it kept there as it was.
    #[tokio::test]
    async fn let_go_from_the_thread_of_its_last_turn_it_stands_where_it_joined() {
        let (joined, there) = (ThreadId::default(), ThreadId::new());
        let body = AgentBody::detached(Vec::new());
        let in_thread = |thread_id, event| {
            Envelope::new(EventContext { thread_id, ..EventContext::default() }, event)
        };
        let name = "helper".to_string();
        let its_join = ActorEvent::ParticipantJoined { participant: body.id(), name: name.clone() };
        let until = Position::START;
        let its_leave =
            || ActorEvent::ParticipantLeft { participant: body.id(), name: name.clone(), until };
        let mut state = AgentChannelState::default();
        body.take_in_said(&mut state, in_thread(there, its_join)).await;
        let work = Work::new(EventContext::default(), None, Classification::PUBLIC, TimeZone::UTC);
        body.begin_turn(&mut state, there, work);
        body.store(&mut state).await;
        assert!(state.thread(there).is_some(), "its part of the thread of its turn");

        body.take_in_said(&mut state, in_thread(there, its_leave())).await;
        body.store(&mut state).await;
        assert!(state.thread(there).is_none(), "nothing of the thread it left comes back");
        assert_eq!(body.standing_in(), joined, "it stands where it joined");

        body.take_in_said(&mut state, in_thread(joined, its_leave())).await;
        assert!(state.thread(joined).is_some(), "a thread it takes part in keeps its part");
    }
}
