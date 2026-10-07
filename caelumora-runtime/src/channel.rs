use caelumora_model::value::name::Name;
use crate::actor::Actor;
use crate::actor_impl::agent::AgentActor;
use crate::actor_impl::approval::ApproverActor;
use crate::actor_impl::external_agent::ExternalAgentActor;
use crate::actor_impl::user::UserActor;
use crate::component::{ComponentContainer, ComponentContext, ComponentImpl};
use crate::moderator::Moderator;
use crate::session::SessionError;
use crate::recorder::Recorder;
use crate::runtime::{ChannelRuntime, RuntimeManager};
use crate::transport::{EventSender, TransportError};
use crate::transport_impl::{FilteringEventSender, InMemoryEventReceiver, InMemoryEventSender, OriginEventSender};
use anyhow::{anyhow, Result};
use arc_swap::ArcSwap;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext, RetryTarget};
use caelumora_model::channel::{Channel, ChannelId, ChannelState, HistoryAccess, HistoryStart, Participant};
use crate::controller::{AgentController, ChannelController, ControllerManager, UserController};
use caelumora_model::agent::{Personality, PersonalityKind};
use caelumora_model::approval::{ApprovalRule, Approver};
use caelumora_model::confidentiality::{user_roles, Classification};
use caelumora_model::mention::{handle_of, MentionDirectory};
use caelumora_model::question::{QuestionError, QuestionId, Questions};
use caelumora_model::message::{Message, MessageId};
use caelumora_model::work::TurnId;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, Reclassified, SecurityAttributes, SecurityContext, SecurityManager};
use caelumora_model::scope::Scope;
use caelumora_model::session::SessionId;
use caelumora_model::store::{EntityStore, StateStore, StoreError, StoreResultExt};
use caelumora_model::user::UserId;
use tracing::{debug, info, trace, warn, Instrument};
use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use crate::session::SessionLoop;
use std::time::Duration;
#[cfg(test)]
use tokio::sync::broadcast;
use tokio::sync::mpsc::Sender;
use tokio::sync::{mpsc, Mutex};
use tokio::task::{AbortHandle, JoinSet};
use tokio_util::task::TaskTracker;

/// A participant of a channel, with the handle it is `@`-mentioned by.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelMember {
    pub id: ActorId,
    pub name: String,
    /// The handle the participant is mentioned by (see `caelumora_model::mention`),
    /// or `None` if it cannot be mentioned - approvers, and names that yield
    /// no handle.
    pub handle: Option<String>,
    /// The actor kind, e.g. `"user"` or the agent's kind.
    pub kind: String,
}

/// The task of a running actor and its inbox. Owned by `run`, which keeps
/// them apart from `ChannelLoop::actors`: that table says who takes part and
/// is read from anywhere; this one is `run`'s, whose `JoinSet` owns the tasks
/// - so that they end with the channel's own task - and which starts and
/// stops them as `ActorChange`s tell it (issue 91).
struct RunningActor {
    id: ActorId,
    abort_handle: AbortHandle,
    /// What to wait on for the task to end (`ChannelActor::task`).
    task: TaskTracker,
    sender: Box<dyn EventSender>,
}

/// Something that may not be done in a channel. Raised by `ChannelLoop`;
/// the messages are meant for users (see `crate::error::user_message`).
#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    /// The `SecurityManager` refused the action. Deliberately silent about
    /// which rule refused it and about what the caller would have to be
    /// instead - like a refused tool call (issue 15).
    #[error("{action} channel {channel} is not permitted")]
    Forbidden { action: &'static str, channel: ChannelId },
    /// Only participants of the session can be added to its channels
    /// (issue 46).
    #[error("{participant} is not a participant of the session of channel {channel}")]
    NotInSession { participant: ActorId, channel: ChannelId },
    /// An agent of another session's or channel's scope, which takes part
    /// only there (issue 178).
    #[error("{participant} lies in the scope of another session or channel than channel {channel}")]
    OutOfScope { participant: ActorId, channel: ChannelId },
    #[error("{participant} already takes part in channel {channel}")]
    AlreadyParticipant { participant: ActorId, channel: ChannelId },
    #[error("{participant} takes no part in channel {channel}")]
    NotAParticipant { participant: ActorId, channel: ChannelId },
    /// A participant of the session whose user or agent was deleted
    /// (issue 120).
    #[error("{participant} of the session of channel {channel} no longer exists")]
    ParticipantGone { participant: ActorId, channel: ChannelId },
    /// Every channel keeps its approver.
    #[error("the approver of channel {channel} cannot be removed")]
    ApproverStays { channel: ChannelId },
    /// A message classified so that it may not enter the channel - above
    /// it, for an audience the channel's would not be covered by, or with a
    /// compartment that is not configured or may not be given (issue 133).
    #[error("a message classified {classification} cannot be written into channel {channel}")]
    ClassificationRefused { classification: Classification, channel: ChannelId },
    #[error("channel {channel} has no message {message}")]
    MessageNotFound { message: MessageId, channel: ChannelId },
    /// A message taken back cannot be changed any more (issue 234).
    #[error("message {message} of channel {channel} was taken back")]
    MessageRevoked { message: MessageId, channel: ChannelId },
    /// Only an agent of the channel can be asked to answer again instead
    /// of its message (issue 234).
    #[error("message {message} of channel {channel} is not one of an agent in it")]
    NotAnAgentsMessage { message: MessageId, channel: ChannelId },
    /// An answer is asked for again while under way only as long as its
    /// turn is under way (issue 235).
    #[error("turn {turn} is not under way in channel {channel}")]
    NotUnderWay { turn: TurnId, channel: ChannelId },
    /// A participant not cleared for the channel's classification - to
    /// join it, or for the classification it would be raised to (issue
    /// 133).
    #[error("{participant} is not cleared for channel {channel} as it is, or would be, classified")]
    NotCleared { participant: ActorId, channel: ChannelId },
    /// A participant added again while the actor it had before is still
    /// ending - one that does not end even when aborted (issue 171).
    #[error("{participant} is still leaving channel {channel}")]
    StillLeaving { participant: ActorId, channel: ChannelId },
    /// Questions of an agent's call cannot be answered so (issue 165): they
    /// are not open, not put to the one answering, or not answered as they
    /// ask. The reason never repeats an answer.
    #[error("questions {question} in channel {channel} cannot be answered: {reason}")]
    Question { question: QuestionId, channel: ChannelId, reason: QuestionError },
}

/// A participant with what its actor is built from: the instance's
/// controller of its user or agent (issue 121), or the approver itself. An
/// agent is run here or elsewhere, as its personality's kind says (issue
/// 179). What it is cleared for follows from it as well (`clearances`).
#[derive(Debug, Clone)]
enum Member {
    User(Arc<UserController>),
    Agent(Arc<AgentController>, Personality),
    ExternalAgent(Arc<AgentController>, Personality),
    Approver(Approver),
}

impl Member {
    /// `participant` of `channel` with its controller - an agent also with
    /// its personality, read once, for its actor to be built from - or
    /// `None` if its user or agent no longer exists. An agent runs only with
    /// a personality it may read, in the channel, as `security` decides
    /// (`GetPersonality`): one whose owner lost it - a share withdrawn - is
    /// not run (issue 191).
    ///
    /// # Errors
    /// Whatever else the store reports when loading it - also that an
    /// agent's personality is gone - and `StoreError::PermissionDenied` if
    /// it is not the agent's to read: the agent exists, but cannot be run.
    async fn of(participant: &Participant, channel: &Channel, controllers: &ControllerManager, security: &dyn SecurityManager) -> Result<Option<Self>> {
        let agent = match participant {
            Participant::User(id) => return Ok(controllers.user_controller(*id).await.optional()?.map(Member::User)),
            Participant::Approver(approver) => return Ok(Some(Member::Approver(approver.clone()))),
            Participant::Agent(id) => match controllers.agent_controller(*id).await.optional()? {
                Some(agent) => agent,
                None => return Ok(None),
            },
        };
        let personality = agent.personality().await?;
        let context = agent.load().security_context(channel.session_id, channel.id);
        security.require(&context, ActionType::GetPersonality(&personality.security_attributes), || StoreError::PermissionDenied).await?;
        Ok(Some(match personality.kind {
            PersonalityKind::Internal => Member::Agent(agent, personality),
            PersonalityKind::External => Member::ExternalAgent(agent, personality),
        }))
    }

    fn actor_id(&self) -> ActorId {
        match self {
            Member::User(user) => ActorId::User(user.id()),
            Member::Agent(agent, _) | Member::ExternalAgent(agent, _) => ActorId::Agent(agent.id()),
            Member::Approver(approver) => ActorId::Approver(approver.id),
        }
    }

    /// The sets of roles the member has to be cleared by, each of them
    /// (issues 133, 179): a user's own; an agent's owner's - it must not
    /// see more than the person responsible for it - and, if its
    /// personality limits it, those roles too, which narrow the owner's and
    /// never widen them. None for an approver. The roles are read from the
    /// store each time, not from the controllers: they are changed past
    /// them, at a login.
    ///
    /// # Errors
    /// Whatever reading a user reports, but that they are gone.
    async fn clearances(&self, store: &dyn EntityStore) -> Result<Vec<Vec<String>>> {
        match self {
            Member::User(user) => Ok(vec![user_roles(store, ActorId::User(user.id())).await?]),
            Member::Agent(agent, personality) | Member::ExternalAgent(agent, personality) => {
                let owner = agent.load().security_attributes.owner_id;
                let mut clearances = vec![user_roles(store, owner).await?];
                if let Some(limit) = personality.limiting_roles() {
                    clearances.push(limit.to_vec());
                }
                Ok(clearances)
            }
            Member::Approver(_) => Ok(Vec::new()),
        }
    }

    /// Nothing if the member may take part in channel `channel_id` of
    /// session `session_id` by where it lies: an agent of a session's or a
    /// channel's scope only within it (`Scope::admits`, issue 178).
    ///
    /// # Errors
    /// `ChannelError::OutOfScope` if it may not.
    fn require_in_scope(&self, session_id: SessionId, channel_id: ChannelId) -> Result<(), ChannelError> {
        let (Member::Agent(agent, _) | Member::ExternalAgent(agent, _)) = self else { return Ok(()) };
        if agent.load().name.scope.admits(&Scope::ChannelScope { session_id, channel_id }) {
            Ok(())
        } else {
            Err(ChannelError::OutOfScope { participant: self.actor_id(), channel: channel_id })
        }
    }

    /// Nothing if the member is cleared for channel `channel`, classified
    /// `classification` (`JoinChannel`), by every set of roles that binds
    /// it - to join it, to take part in it when it is created, or for what
    /// it would be reclassified to.
    ///
    /// # Errors
    /// `ChannelError::NotCleared` if it is not; whatever reading its roles
    /// reports.
    async fn require_cleared(&self, store: &dyn EntityStore, security: &dyn SecurityManager, context: &SecurityContext, classification: &Classification, channel: ChannelId) -> Result<()> {
        let participant = self.actor_id();
        for roles in self.clearances(store).await? {
            let action = ActionType::JoinChannel { participant, roles: &roles, channel: classification };
            security.require(context, action, || ChannelError::NotCleared { participant, channel }).await?;
        }
        Ok(())
    }
}

/// Nothing if every participant of `channel` that still exists may take
/// part in it - lies where it may (`Member::require_in_scope`) and is
/// cleared for its classification (`Member::require_cleared`) - asked
/// before anything of a new or restored channel or session is stored.
///
/// # Errors
/// `ChannelError::OutOfScope` or `NotCleared` for the first who may not;
/// whatever reading a participant reports.
pub(crate) async fn require_admitted(runtime: &RuntimeManager, context: &SecurityContext, channel: &Channel) -> Result<()> {
    for participant in &channel.participants {
        if let Some(member) = Member::of(participant, channel, runtime.controller_manager(), runtime.security_manager().as_ref()).await? {
            member.require_in_scope(channel.session_id, channel.id)?;
            member.require_cleared(runtime.entity_store().as_ref(), runtime.security_manager().as_ref(), context, &channel.classification, channel.id).await?;
        }
    }
    Ok(())
}

/// Whether `error` of `Member::of` says that the member cannot be run -
/// its agent's personality is gone, or not the agent's to read - rather
/// than that the store failed.
fn cannot_run(error: &anyhow::Error) -> bool {
    matches!(error.downcast_ref::<StoreError>(), Some(StoreError::NotFound { .. } | StoreError::PermissionDenied))
}

/// The actor of a channel's participant, with what it was built from and
/// what tells when its task has ended.
#[derive(Debug, Clone)]
struct ChannelActor {
    actor: Arc<dyn Actor>,
    member: Member,
    /// Tracks the actor's task while `run` runs it, so that whoever stops
    /// the actor can wait for the task to end without going through `run`
    /// (issue 187) - `run`'s `JoinSet` alone owns the task.
    task: TaskTracker,
}

/// The actors of a channel's participants.
type Actors = HashMap<ActorId, ChannelActor>;

/// How long a stopped actor gets to end its task by itself before it is
/// aborted (issue 75). An agent's tool under way is interrupted by the stop
/// rather than waited for (issue 161); this is for whatever else hangs.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// How long adding a participant again waits for the actor it had before
/// to end (issue 171): past `STOP_GRACE`, after which that actor's task is
/// aborted, by as long as the abort takes to reach it.
const LEAVE_GRACE: Duration = Duration::from_secs(STOP_GRACE.as_secs() + 1);

/// Waits until the tasks `tasks` track have ended, at most `STOP_GRACE`,
/// and returns whether they did (issue 187). An actor whose task never
/// started has nothing tracked, and is not waited for.
async fn stopped(tasks: &[TaskTracker]) -> bool {
    for task in tasks {
        task.close();
    }
    tokio::time::timeout(STOP_GRACE, futures::future::join_all(tasks.iter().map(TaskTracker::wait))).await.is_ok()
}

/// A change to the actors a running channel runs (issue 46), carried out by
/// `run`, which owns their tasks.
enum ActorChange {
    Start(ChannelActor),
    /// The actor to stop, and on whose behalf it was removed - in whose name
    /// `run` announces it (`ParticipantRemoved`, issue 162).
    Stop(ActorId, SecurityContext),
}

#[derive(Debug)]
pub struct ChannelLoop {
    id: ChannelId,
    session_id: SessionId,
    channel: Arc<ChannelController>,
    /// What builds the runtimes of its actors.
    runtime: ChannelRuntime,
    /// The services it works with, taken from `runtime` once (issue 229).
    security: Arc<dyn SecurityManager>,
    entities: Arc<dyn EntityStore>,
    controllers: Arc<ControllerManager>,
    /// The actors of the participants, each with what it was built from -
    /// what a reclassification asks its clearances of; replaced as a whole
    /// when someone joins or leaves (issue 46), so that every reader sees
    /// one set.
    actors: ArcSwap<Actors>,
    /// The tasks of the actors that were removed, which may still be ending
    /// (issue 171): a participant added again waits for its predecessor
    /// (`left`). Kept until the participant is added again; the task of one
    /// that has ended is waited for at once. Nothing else cleans it up: an
    /// entry of a participant never added again stays as long as the
    /// channel runs - a closed `TaskTracker`, a few bytes each, which only
    /// grows with removals.
    leaving: std::sync::Mutex<HashMap<ActorId, TaskTracker>>,
    moderator: ComponentImpl<dyn Moderator>,
    mentions: ArcSwap<MentionDirectory>,
    /// Actors for `run` to start or stop; `run` takes the receiver.
    changes_tx: mpsc::UnboundedSender<ActorChange>,
    changes_rx: Mutex<mpsc::UnboundedReceiver<ActorChange>>,
    recorder: Mutex<Box<dyn Recorder>>,
    state: Mutex<ChannelState>,
    state_store: Arc<dyn StateStore>,
    /// Set by `cancel`, cleared when the channel is woken: while set, what is
    /// said is still delivered, but nobody is asked to respond to it
    /// (`route_event_impl`). The turns themselves are cancelled on the actors.
    quiet: AtomicBool,
    /// Every event the channel took in, for tests that watch what happens
    /// in it beyond what a user is told; nobody follows a channel this way
    /// otherwise - a user follows their actor (issue 220).
    #[cfg(test)]
    observed: broadcast::Sender<Envelope>,
    max_route_decisions: i32,
    run_lock: Mutex<()>,
    /// The session the channel belongs to (issue 123), set by `attach` once
    /// the session exists; unset outside of one, e.g. in tests. Weak, since
    /// the session owns its channels.
    session: OnceLock<Weak<SessionLoop>>,
}

impl ChannelLoop {
    /// Builds the channel and its actors, whose runtimes `runtime` creates and
    /// which reach the channel through it once it exists.
    pub async fn new(channel: Channel, runtime: ChannelRuntime) -> Result<Arc<Self>> {
        let id = channel.id;
        let session_id = channel.session_id;
        let state_store = Arc::clone(runtime.state_store());
        // Its components (issue 174) act on behalf of the channel's owner,
        // as an agent's act on behalf of its owner.
        let security_context = SecurityContext::new(channel.security_attributes.owner_id, session_id, id);
        let components = ComponentContainer::from_config(
            &channel.components,
            runtime.component_registry(),
            &ComponentContext::new(security_context, Arc::clone(runtime.security_manager()), Arc::clone(runtime.secret_manager())),
        )
        .await
        .map_err(SessionError::InvalidModerator)?;
        let moderator = components.one::<dyn Moderator>().await.map_err(SessionError::InvalidModerator)?;
        
        let state = match state_store.get_channel_state(id).await {
            Ok(s) => s,
            Err(StoreError::NotFound { entity: _, id: _ }) => {
                let mut state = ChannelState::new(id);
                state_store.put_channel_state(id, &mut state).await?;
                state
            }
            Err(e) => return Err(e.into()),
        };
        let state = Mutex::new(state);

        let recorder = runtime.new_recorder()?;

        let channel_controller = runtime.controller_manager().new_channel_controller(channel);

        let (changes_tx, changes_rx) = mpsc::unbounded_channel();
        let channel_loop = Arc::new(Self {
            id,
            session_id,
            channel: channel_controller,
            security: Arc::clone(runtime.security_manager()),
            entities: Arc::clone(runtime.entity_store()),
            controllers: Arc::clone(runtime.controller_manager()),
            runtime,
            actors: ArcSwap::from_pointee(HashMap::new()),
            leaving: std::sync::Mutex::new(HashMap::new()),
            moderator,
            mentions: ArcSwap::from_pointee(MentionDirectory::default()),
            changes_tx,
            changes_rx: Mutex::new(changes_rx),
            recorder: Mutex::new(recorder),
            state,
            state_store,
            quiet: AtomicBool::new(false),
            #[cfg(test)]
            observed: broadcast::channel(128).0,
            max_route_decisions: 100,
            run_lock: Mutex::new(()),
            session: OnceLock::new(),
        });
        channel_loop.runtime.attach(&channel_loop);
        // The participants the channel has; `run` starts their tasks.
        let participants = channel_loop.channel.load().participants.clone();
        for participant in &participants {
            // A user or agent deleted since it joined, or an agent that may
            // not run - its personality withdrawn or gone: the channel runs
            // without it rather than not at all (issues 120, 198). Its
            // owner's loss must not keep everyone else out, nor can a third
            // party - the personality's owner - stop someone's channel so.
            let found = channel_loop.member_of(participant).await;
            let member = match found {
                Ok(Some(member)) => member,
                Ok(None) => {
                    warn!("{} of channel {} no longer exists and is left out", participant.actor_id(), channel_loop.id);
                    continue;
                }
                Err(e) if cannot_run(&e) => {
                    warn!("{} of channel {} cannot be run and is left out: {:#}", participant.actor_id(), channel_loop.id, e);
                    continue;
                }
                Err(e) => return Err(e),
            };
            let actor = Self::create_actor(Arc::clone(&channel_loop.channel), &member, &channel_loop.runtime).await?;
            channel_loop.adopt(Arc::from(actor), member).await;
        }
        Ok(channel_loop)
    }

    /// `participant` with what its actor is built from (`Member::of`).
    async fn member_of(&self, participant: &Participant) -> Result<Option<Member>> {
        Member::of(participant, &self.channel.load(), &self.controllers, self.security.as_ref()).await
    }

    /// Makes `session` the session of this channel. Called once by the
    /// session, as soon as it exists and before the channel starts.
    pub(crate) fn attach(&self, session: &Arc<SessionLoop>) {
        let _ = self.session.set(Arc::downgrade(session));
    }

    /// The session the channel belongs to, or `None` outside of one (e.g. in
    /// tests) or once it is gone.
    pub(crate) fn session_loop(&self) -> Option<Arc<SessionLoop>> {
        self.session.get().and_then(Weak::upgrade)
    }

    /// Takes `actor` into the channel: the moderator routes to it, it is
    /// among the participants' actors, and it can be mentioned. The one way
    /// an actor enters the channel - when the channel is built and when a
    /// participant joins (issue 91). Its task is `run`'s to start.
    async fn adopt(&self, actor: Arc<dyn Actor>, member: Member) -> ChannelActor {
        self.moderator.add_actor(Arc::clone(&actor));
        let id = actor.id();
        let adopted = ChannelActor { actor, member, task: TaskTracker::new() };
        self.actors.rcu(|actors| {
            let mut actors = HashMap::clone(actors);
            actors.insert(id, adopted.clone());
            actors
        });
        self.rebuild_mentions();
        adopted
    }

    /// Every event the channel takes in from now on (see `observed`).
    #[cfg(test)]
    pub(crate) fn observe(&self) -> broadcast::Receiver<Envelope> {
        self.observed.subscribe()
    }

    /// The whole history, whoever may see it; outside the channel, what a
    /// participant sees is `visible_history`.
    pub(crate) async fn history(&self) -> Vec<Message> {
        self.state.lock().await.history.messages()
    }

    /// The message `id` of the history as it stands - on whichever branch -
    /// or `None` if it was never said here.
    pub(crate) async fn message(&self, id: MessageId) -> Option<Message> {
        self.state.lock().await.history.message(id)
    }


    pub fn id(&self) -> ChannelId { self.id }

    pub fn session_id(&self) -> SessionId { self.session_id }
    
    pub fn name(&self) -> Name {
        self.channel.name()
    }

    /// Who the channel belongs to and who contributes to it (issue 11).
    pub fn security_attributes(&self) -> SecurityAttributes {
        self.channel.load().security_attributes.clone()
    }

    /// The channel as it is now.
    pub fn channel(&self) -> arc_swap::Guard<Arc<Channel>> {
        self.channel.load()
    }

    /// Renames the channel on behalf of `context`: stored first, which is
    /// where the permission is checked (`UpdateChannel`, issue 70).
    ///
    /// # Errors
    /// `StoreError::Invalid` if `new_name` is no name,
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// channel, or whatever else storing it reports.
    pub async fn update_name(&self, context: &SecurityContext, new_name: &str) -> Result<()> {
        Ok(self.channel.update_name(context, new_name).await?)
    }

    /// Has the channel's conversation held in `language`, or in none in
    /// particular, on behalf of `context` (issue 56): stored first, which is
    /// where the permission is checked (`UpdateChannel`), like a new name.
    /// Its agents are told from their next turn on; nothing said before is
    /// translated.
    ///
    /// # Errors
    /// `StoreError::Invalid` if `language` is no language tag,
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// channel, or whatever else storing it reports.
    pub async fn update_language(&self, context: &SecurityContext, language: Option<&str>) -> Result<()> {
        self.channel.update_language(context, language).await?;
        info!(channel = %self.id, language = language.unwrap_or("none"), by = %context.caller_id, "Channel language changed");
        Ok(())
    }

    /// Says what the channel is for (issue 245) on behalf of `context`:
    /// stored first, which is where the permission is checked
    /// (`UpdateChannel`), like a new name. Its agents are told from their
    /// next turn on.
    ///
    /// # Errors
    /// `StoreError::Validation` if `summary` or `briefing` is too long or
    /// holds what it may not, `StoreError::PermissionDenied` if `context`
    /// may not change the channel, or whatever else storing it reports.
    pub async fn update_description(&self, context: &SecurityContext, summary: Option<&str>, briefing: Option<&str>) -> Result<()> {
        self.channel.update_description(context, summary, briefing).await?;
        info!(channel = %self.id, by = %context.caller_id, "Channel description changed");
        Ok(())
    }

    /// Sets what the channel restricts of the tool calls made in it (issue
    /// 13) on behalf of `context`: stored first, which is where the
    /// permission is checked (`UpdateChannel`). Its agents' next calls are
    /// decided by them.
    ///
    /// # Errors
    /// `StoreError::Validation` if a rule does not only ask or deny,
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// channel, or whatever else storing it reports.
    pub async fn update_approval_rules(&self, context: &SecurityContext, rules: Vec<ApprovalRule>) -> Result<()> {
        let count = rules.len();
        self.channel.update_approval_rules(context, rules).await?;
        info!(channel = %self.id, rules = count, by = %context.caller_id, "Channel approval rules changed");
        Ok(())
    }

    /// Reclassifies the channel as `classification` on behalf of `context`
    /// (issue 133): asked as `ActionType::Reclassify`, then stored
    /// (`UpdateChannel`) and announced. Lowering it releases what is in it:
    /// every message is fitted into the new classification
    /// (`MessageHistory::fit_into`), since the agents in the channel carry
    /// what they know into what they write next. Raising it changes nothing
    /// already written.
    ///
    /// A turn that is running keeps the classification it began with. What
    /// it saw is released by nobody, so lowering the channel ends it (issue
    /// 141): once stored, the channel's turns are cancelled and the channel
    /// kept quiet, as `cancel` does. Each agent then finds at its next step
    /// that its turn does not flow into the channel any more and gives it
    /// up (`GivenUpReason::Lowered`) instead of taking it over. Lowered
    /// means: what the channel held, classified as it was, may not be put
    /// into it as it is now (`AccessQuestions::may_publish`, in `context`'s name).
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if `context` may not reclassify the
    /// channel as asked; whatever storing it reports.
    pub async fn reclassify(&self, context: &SecurityContext, classification: Classification) -> Result<()> {
        let channel = self.classification();
        let attributes = self.security_attributes();
        let action = ActionType::Reclassify {
            target: Reclassified::Channel { attributes: &attributes },
            current: &channel,
            new: &classification,
            channel: &channel,
        };
        self.require(context, action, "reclassifying").await?;
        // Raised above a participant, the channel would show them what they
        // may not see: refused rather than anyone removed.
        for ChannelActor { member, .. } in self.actors.load().values() {
            member.require_cleared(self.entities.as_ref(), self.security.as_ref(), context, &classification, self.id).await?;
        }
        self.channel.update_classification(context, classification.clone()).await?;
        let lowered = !self.security.may_publish(context, context.caller_id, &channel, &classification).await;
        if lowered {
            self.cancel_actors();
        }
        let mut state = self.state.lock().await;
        let fitted = state.history.fit_into(&classification);
        if fitted > 0 {
            self.state_store.put_channel_state(self.id, &mut state).await?;
        }
        info!(channel = %self.id, %classification, fitted, lowered, by = %context.caller_id, "Channel reclassified");
        Ok(())
    }

    /// The part of the history `participant` may see (issue 46): all of it,
    /// unless they joined later with a limit - with what a reader records as
    /// read (issue 72). The one place the limits are applied: for users over
    /// REST, for agents reading the channel and for the transcript of an
    /// agent that joins.
    pub async fn visible_history(&self, participant: ActorId) -> VisibleHistory {
        let (history, digest) = {
            let state = self.state.lock().await;
            (state.history.messages(), state.history.digest())
        };
        let start = self.history_start_of(participant);
        VisibleHistory { messages: visible_part(start, history), digest, start }
    }

    /// Where `participant`'s view of the history begins; `None`: all of it.
    fn history_start_of(&self, participant: ActorId) -> Option<HistoryStart> {
        self.channel.load().history_limits.iter().find(|l| l.participant == participant).map(|l| l.start)
    }

    /// Adds `participant` - a participant of the session - to the channel on
    /// behalf of `context` (issue 46), seeing as much of the history as
    /// `history` grants. Stored first (`UpdateChannel`, also asked as
    /// `JoinChannel`), then its actor built and given the part of the
    /// history it may see (`Actor::catch_up`, issue 190), then started, the
    /// moderator and the mention directory told, and the join announced.
    /// A participant removed shortly before is added only once its old
    /// actor has ended (`left`, issue 171).
    ///
    /// # Errors
    /// `ChannelError::NotInSession`, `OutOfScope`, `AlreadyParticipant`,
    /// `StillLeaving` or `Forbidden`; a `StoreError` if `context` may not
    /// change the channel.
    pub async fn add_participant(&self, context: &SecurityContext, participant: ActorId, history: HistoryAccess) -> Result<()> {
        // Whether the actor may change the channel at all, before anything
        // is asked of the participant (issue 198, F2): who may not learns
        // nothing of its clearance or its personality.
        let attributes = self.security_attributes();
        self.require(context, ActionType::UpdateChannel(&attributes), "adding a participant to").await?;
        if self.actors.load().contains_key(&participant) {
            return Err(ChannelError::AlreadyParticipant { participant, channel: self.id }.into());
        }
        self.left(participant).await?;
        let found = self.session_loop()
            .and_then(|s| s.session().participants.iter().find(|p| p.actor_id() == participant).cloned())
            .ok_or(ChannelError::NotInSession { participant, channel: self.id })?;
        let member = self.member_of(&found).await?
            .ok_or(ChannelError::ParticipantGone { participant, channel: self.id })?;
        member.require_in_scope(self.session_id, self.id)?;
        member.require_cleared(self.entities.as_ref(), self.security.as_ref(), context, &self.classification(), self.id).await?;

        let messages = self.history().await;
        let start = history_start(history, &messages, chrono::Utc::now());
        self.channel.add_participant(context, found.clone(), start).await?;

        let started = async {
            let actor = Self::create_actor(Arc::clone(&self.channel), &member, &self.runtime).await?;
            actor.catch_up(visible_part(self.history_start_of(participant), messages)).await?;
            Ok::<_, anyhow::Error>(actor)
        }.await;
        let actor: Arc<dyn Actor> = match started {
            Ok(actor) => Arc::from(actor),
            Err(error) => {
                // Stored, but it cannot run: taken back out, in the runtime's
                // name - adding it was permitted.
                if let Err(undo) = self.channel.remove_participant(&SecurityContext::system(), participant).await {
                    warn!("{} was added to channel {} but could not be started nor removed again: {}", participant, self.id, undo);
                }
                return Err(error);
            }
        };
        let adopted = self.adopt(actor, member).await;
        let _ = self.changes_tx.send(ActorChange::Start(adopted));
        info!("{} joined channel {}", participant, self.id);
        Ok(())
    }

    /// Removes `participant` from the channel on behalf of `context` (issue
    /// 46): stored first (`UpdateChannel`), then its actor stopped - an agent
    /// at once, without finishing a turn; the approver discards its pending
    /// approvals. Until its task has ended, the participant cannot be added
    /// again (`left`, issue 171). The leave is announced by the
    /// `ChannelController` on the event bus (issue 73), and in the channel by
    /// `run` with a `ParticipantRemoved` in the name of `context`'s caller,
    /// after which nothing of the participant enters the channel any more
    /// (issue 162).
    /// Its state is kept, for the record and for when it joins again.
    ///
    /// # Errors
    /// `ChannelError::ApproverStays` or `NotAParticipant`; a `StoreError` if
    /// `context` may not change the channel.
    pub async fn remove_participant(&self, context: &SecurityContext, participant: ActorId) -> Result<()> {
        if matches!(participant, ActorId::Approver(_)) {
            return Err(ChannelError::ApproverStays { channel: self.id }.into());
        }
        let Some(ChannelActor { actor, task, .. }) = self.actors.load().get(&participant).cloned() else {
            return Err(ChannelError::NotAParticipant { participant, channel: self.id }.into());
        };
        self.channel.remove_participant(context, participant).await?;
        self.leaving.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(participant, task);
        self.actors.rcu(|actors| {
            let mut actors = HashMap::clone(actors);
            actors.remove(&participant);
            actors
        });
        self.moderator.remove_actor(participant);
        self.rebuild_mentions();
        actor.stop();
        let _ = self.changes_tx.send(ActorChange::Stop(participant, context.clone()));
        info!("{} left channel {}, removed by {}", participant, self.id, context.caller_id);
        Ok(())
    }

    /// Waits until the actor `participant` had in the channel before has
    /// ended its task, if it was removed (issue 171): until then it may
    /// still store the participant's state - a turn given up - which a new
    /// actor loads as it is built, and what it sends would pass as the new
    /// actor's. At most `LEAVE_GRACE`: by then `run` has aborted it.
    ///
    /// # Errors
    /// `ChannelError::StillLeaving` if it has not ended even so - it blocks
    /// without ever reaching an `await`.
    async fn left(&self, participant: ActorId) -> Result<(), ChannelError> {
        let Some(task) = self.leaving.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&participant).cloned() else {
            return Ok(());
        };
        // Closed by `run` once it stops the actor; not if the channel does
        // not run, and then nothing is tracked to wait for.
        task.close();
        if tokio::time::timeout(LEAVE_GRACE, task.wait()).await.is_err() {
            warn!("{} is added to channel {} again, but its old actor has not ended", participant, self.id);
            return Err(ChannelError::StillLeaving { participant, channel: self.id });
        }
        self.leaving.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&participant);
        Ok(())
    }

    fn rebuild_mentions(&self) {
        let mentions = Self::mention_directory(&self.actors.load());
        for handle in mentions.ambiguous() {
            warn!("Channel {}: handle @{} is shared by several participants; mentioning it addresses all of them", self.id, handle);
        }
        self.mentions.store(Arc::new(mentions));
    }

    /// Ends the current turn of every actor in the channel, on behalf of
    /// `context`'s initiator, and keeps the channel quiet - nobody is asked
    /// to respond - until it is woken again, usually by the next message.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `SecurityManager` refuses
    /// `CancelChannel`; the channel is then untouched. Deliberately one
    /// decision for the whole channel: a per-actor one could leave half of it
    /// cancelled (issue 30).
    pub async fn cancel(&self, context: &SecurityContext) -> Result<()> {
        let channel = self.channel.load();
        self.require(context, ActionType::CancelChannel(&channel.security_attributes), "cancelling").await?;
        self.cancel_actors();
        Ok(())
    }

    /// Ends the current turn of every actor and keeps the channel quiet:
    /// what `cancel` does once permitted, and `reclassify` as a consequence
    /// of lowering the channel.
    fn cancel_actors(&self) {
        self.quiet.store(true, Ordering::SeqCst);
        for ChannelActor { actor, .. } in self.actors.load().values() {
            actor.cancel();
        }
    }

    /// Wakes the channel's sleeping actors because someone asked for it -
    /// a `/wakeup` and nothing else. Returns the names of the actors that
    /// were asleep.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `SecurityManager` refuses
    /// `WakeChannel`.
    pub async fn wakeup(&self, context: &SecurityContext) -> Result<Vec<String>> {
        let channel = self.channel.load();
        self.require(context, ActionType::WakeChannel(&channel.security_attributes), "waking the agents of").await?;
        let asleep = self.actors.load().values()
            .map(|a| &a.actor)
            .filter(|a| a.state() == ActorState::Sleeping)
            .map(|a| a.name())
            .collect();
        self.wake_for_delivery();
        Ok(asleep)
    }

    /// Ends the current turn of one actor deliberately (issue 235), like
    /// `cancel` and under the same permission: the policy that lets someone
    /// stop the channel lets them stop one of its actors. The channel is not
    /// kept quiet - that is the channel's state, not the actor's: the others
    /// are still asked to respond, and the answer the actor's turn is taken
    /// over with reaches everyone as any answer does.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `SecurityManager` refuses
    /// `CancelChannel`, before anything of the actor is looked at;
    /// `NotAParticipant` if `actor` is not in this channel.
    pub async fn cancel_actor(&self, context: &SecurityContext, actor: ActorId) -> Result<()> {
        let channel = self.channel.load();
        self.require(context, ActionType::CancelChannel(&channel.security_attributes), "cancelling").await?;
        self.actor_by_id(actor)
            .ok_or(ChannelError::NotAParticipant { participant: actor, channel: self.id })?
            .cancel();
        Ok(())
    }

    /// Wakes one actor deliberately, like `wakeup` and under the same
    /// permission. Returns whether it was asleep.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `SecurityManager` refuses
    /// `WakeChannel`; `NotAParticipant` if `actor` is not in this channel.
    pub async fn wakeup_actor(&self, context: &SecurityContext, actor: ActorId) -> Result<bool> {
        let channel = self.channel.load();
        self.require(context, ActionType::WakeChannel(&channel.security_attributes), "waking the agents of").await?;
        let actor = self.actor_by_id(actor)
            .ok_or(ChannelError::NotAParticipant { participant: actor, channel: self.id })?;
        let was_asleep = actor.state() == ActorState::Sleeping;
        self.quiet.store(false, Ordering::SeqCst);
        actor.wakeup();
        Ok(was_asleep)
    }

    /// Tells every actor of the channel to stop for good (`Actor::stop`) -
    /// for a channel that is deleted: nothing of it is resumed, and whoever
    /// follows it through their actor is told it ended, whether or not the
    /// actors' tasks ever ran (issue 104). Returns once their tasks have
    /// ended, or `STOP_GRACE` has passed (issue 187): an external agent
    /// cancels its open remote tasks on its way out, and every actor stores
    /// its state consistent. Those still running then are the caller's to
    /// abort, with the channel's task.
    pub(crate) async fn stop_actors(&self) {
        let actors = self.actors.load();
        for ChannelActor { actor, .. } in actors.values() {
            actor.stop();
        }
        let tasks: Vec<TaskTracker> = actors.values().map(|a| a.task.clone()).collect();
        if !stopped(&tasks).await {
            warn!("Not every actor of channel {} stopped by itself: the rest is aborted", self.id);
        }
    }

    /// Wakes the channel's actors because something is on its way to them:
    /// a `WakeupRequested`, which a user's actor sends with every message. A
    /// mention wakes only whom it addresses (`route_event_impl`). That is the
    /// consequence of an action already permitted, so it carries no
    /// permission of its own - otherwise a user allowed to write but not to
    /// wake could not write.
    fn wake_for_delivery(&self) {
        self.quiet.store(false, Ordering::SeqCst);
        for ChannelActor { actor, .. } in self.actors.load().values() {
            actor.wakeup();
        }
    }

    /// `action`, refused as a `ChannelError::Forbidden` that names what was
    /// refused, not why.
    async fn require(&self, context: &SecurityContext, action: ActionType<'_>, what: &'static str) -> Result<()> {
        let channel = self.id;
        Ok(self.security.require(context, action, || ChannelError::Forbidden { action: what, channel }).await?)
    }

    pub fn actor_ids(&self) -> Vec<ActorId> {
        self.actors.load().keys().copied().collect()
    }

    /// All participants of the channel, in no particular order.
    pub fn members(&self) -> Vec<ChannelMember> {
        self.actors.load().values()
            .map(|ChannelActor { actor: a, .. }| {
                let name = a.name();
                let handle = Some(handle_of(&name))
                    .filter(|h| Self::is_mentionable(a.id()) && !h.is_empty());
                ChannelMember { id: a.id(), name, handle, kind: a.kind() }
            })
            .collect()
    }

    /// `questions` as put to the users of the channel (issue 165): every
    /// addressee found among them - a handle as its participants are
    /// mentioned, the agent's owner as `owner` (`Questions::resolve`).
    pub(crate) fn put(&self, questions: Questions, owner: ActorId) -> Result<Questions, QuestionError> {
        let users: Vec<ActorId> = self.actor_ids().into_iter().filter(|id| matches!(id, ActorId::User(_))).collect();
        questions.resolve(&self.mentions.load(), &users, owner)
    }

    /// The classification of the channel's contents (issue 133).
    pub fn classification(&self) -> Classification {
        self.channel.load().classification.clone()
    }

    fn is_mentionable(id: ActorId) -> bool {
        matches!(id, ActorId::User(_) | ActorId::Agent(_))
    }

    fn mention_directory(actors: &Actors) -> MentionDirectory {
        let names: Vec<(ActorId, String)> = actors.values()
            .map(|a| &a.actor)
            .filter(|a| Self::is_mentionable(a.id()))
            .map(|a| (a.id(), a.name()))
            .collect();
        MentionDirectory::new(names.iter().map(|(id, name)| (*id, name.as_str())))
    }

    pub(crate) fn actor_by_id(&self, actor_id: ActorId) -> Option<Arc<dyn Actor>> {
        self.actors.load().get(&actor_id).map(|a| Arc::clone(&a.actor))
    }

    /// The turn under way of the participant `actor` (issue 235), if it is
    /// in one that can still be ended - what a request to answer it again
    /// names.
    pub(crate) fn turn_under_way(&self, actor: ActorId) -> Option<TurnId> {
        self.actor_by_id(actor)?.turn_under_way()
    }

    /// The actor `user` acts through in this channel, or `None` if they are
    /// not a participant of it.
    ///
    /// Every envelope is built by the actor itself, so this is what decides
    /// whose name a message or an approval carries: there is deliberately no
    /// way to reach "some" user actor (issue 30).
    pub fn user_actor(&self, user: UserId) -> Option<Arc<UserActor>> {
        let actor = self.actor_by_id(ActorId::User(user))?;
        let any_actor: Arc<dyn Any + Send + Sync> = actor;
        Arc::downcast::<UserActor>(any_actor).ok()
    }

    /// The actor of `member`.
    async fn create_actor(channel: Arc<ChannelController>, member: &Member, runtime: &ChannelRuntime) -> Result<Box<dyn Actor>> {
        let (session_id, channel_id) = { let c = channel.load(); (c.session_id, c.id) };
        let actor_runtime = runtime.new_actor_runtime(member.actor_id(), session_id, channel_id);
        Ok(match member.clone() {
            Member::User(user) => Box::new(UserActor::new(Arc::clone(&channel), user, actor_runtime).await?),
            Member::Agent(agent, personality) => Box::new(AgentActor::new(Arc::clone(&channel), agent, personality, actor_runtime).await?),
            Member::ExternalAgent(agent, personality) => Box::new(ExternalAgentActor::new(&channel.load(), agent, personality, actor_runtime).await?),
            Member::Approver(approver) => Box::new(ApproverActor::new(channel, approver, actor_runtime)),
        })
    }

    pub async fn run(&self) -> Result<()> {
        let _guard = self.run_lock.try_lock()
            .map_err(|_| anyhow!("Already running"))?;

        info!("Channel {}/{} is starting", self.channel.name(), self.id);
        let mut changes_rx = self.changes_rx.lock().await;
        let (event_tx, mut event_rx) = mpsc::channel(10);
        let mut join_set = JoinSet::new();
        let mut running = self.spawn_actors(event_tx.clone(), &mut join_set);

        loop {
            tokio::select! {
                // Events before changes (issue 171): what a removed actor
                // sent before it ended is routed - or dropped, as its
                // sender's - before an actor that joins under its id starts.
                biased;
                maybe_event = event_rx.recv() => {
                    if self.handle_actor_event(maybe_event, &running).await {
                        break;
                    }
                }
                Some(change) = changes_rx.recv() => {
                    self.apply_change(change, &mut running, &event_tx, &mut join_set).await;
                }
                // Finished tasks are dropped as they end; once the last one
                // is gone, the channel stops - the sender kept for actors
                // joining later keeps the event queue from closing by itself.
                Some(_) = join_set.join_next(), if !join_set.is_empty() => {
                    if join_set.is_empty() {
                        break;
                    }
                }
            }
        }

        info!("Channel {}/{} stopped", self.channel.name(), self.id);
        Ok(())
    }

    /// Starts or stops an actor as a participant joined or left (issue 46).
    async fn apply_change(&self, change: ActorChange, running: &mut HashMap<ActorId, RunningActor>, event_tx: &Sender<Envelope>, join_set: &mut JoinSet<ActorId>) {
        match change {
            ActorChange::Start(joined) => {
                // Joined before `run` started: already running. Removed
                // again before `run` got to start it: never started, so that
                // it cannot run next to whoever joins under its id after it
                // (issue 171).
                let id = joined.actor.id();
                let still_joined = self.actors.load().get(&id).is_some_and(|current| Arc::ptr_eq(&current.actor, &joined.actor));
                if !running.contains_key(&id) && still_joined {
                    let handle = self.spawn_actor(&joined, event_tx.clone(), join_set);
                    running.insert(handle.id, handle);
                }
            }
            ActorChange::Stop(id, remover) => {
                // Dropping the handle closes the actor's inbox; told to stop
                // (`Actor::stop`), it ends its task itself once its state is
                // stored consistent. Only if it does not - a tool that hangs -
                // is it aborted.
                if let Some(handle) = running.remove(&id) {
                    let (abort_handle, task) = (handle.abort_handle.clone(), handle.task.clone());
                    let (channel, actor) = (self.id, id);
                    tokio::spawn(async move {
                        if !stopped(&[task]).await {
                            warn!("Actor {} did not stop by itself after leaving channel {}: aborted", actor, channel);
                            abort_handle.abort();
                        }
                    });
                    info!("Actor {} is stopping: it left channel {}", id, self.id);
                }
                // Announced here, between the events `run` routes: what the
                // actor sent before is routed, what it sends after is not
                // (`handle_actor_event`), so that nothing of it follows the
                // announcement (issue 162).
                let name = self.controllers.actor_name(remover.caller_id).await.unwrap_or_else(|e| {
                    warn!("Cannot name {} for channel {}: {}", remover.caller_id, self.id, e);
                    remover.caller_id.to_string()
                });
                let sender = EventContext::new(self.session_id, self.id, remover.caller_id, name);
                let removed = Envelope::new(remover, sender, ActorEvent::ParticipantRemoved { participant: id });
                self.process_envelope(removed, running).await;
            }
        }
    }

    async fn handle_actor_event(&self, maybe_event: Option<Envelope>, running: &HashMap<ActorId, RunningActor>) -> bool {
        if let Some(event) = maybe_event {
            // An actor that was removed may still finish the step it was in;
            // what it sends then no longer enters the channel (issue 162).
            // Its sender is the actor's own: its `OriginEventSender` passes
            // on nothing else (issue 228).
            if !running.contains_key(&event.context.sender_id) {
                debug!("Dropped {}: its sender left channel {}", event.describe(), self.id);
                return false;
            }
            let span = crate::logging::envelope_span(&event.context, event.event.kind());
            self.process_envelope(event, running).instrument(span).await;
            false
        } else {
            true
        }
    }

    async fn process_envelope(&self, event: Envelope, running: &HashMap<ActorId, RunningActor>) {
        match event.event {
            // One per streamed chunk: far finer than per message.
            ActorEvent::PartialMessageGenerated(_) | ActorEvent::IntermediatePartialGenerated(_) => {
                trace!("Received {}", event.describe())
            }
            _ => debug!("Received {}", event.describe()),
        }
        // The full envelope carries message content: trace level only.
        trace!("Envelope: {:?}", event);
        if !self.admits(&event).await || !self.admits_change(&event).await {
            return;
        }
        let event = self.resolve_mentions(event);
        self.record_receive_event(&event).await;
        self.append_history(&event).await;
        self.route_event(running, event).await;
    }

    /// Starts the tasks of the channel's actors as `run` begins.
    fn spawn_actors(&self, actor_output: Sender<Envelope>, join_set: &mut JoinSet<ActorId>) -> HashMap<ActorId, RunningActor> {
        self.actors.load().iter()
            .map(|(id, actor)| (*id, self.spawn_actor(actor, actor_output.clone(), join_set)))
            .collect()
    }

    fn spawn_actor(&self, channel_actor: &ChannelActor, actor_output: Sender<Envelope>, join_set: &mut JoinSet<ActorId>) -> RunningActor {
        let ChannelActor { actor, task, .. } = channel_actor.clone();
        let id = actor.id();
        let filter = actor.event_filter();
        let (tx, rx) = mpsc::channel(100);

        let actor_rx = Box::new(InMemoryEventReceiver::new(rx));
        // It sends only in its own name (issue 228).
        let actor_tx = Box::new(OriginEventSender::new(id, Box::new(InMemoryEventSender::new(actor_output))));

        // Tracked inside the `JoinSet`'s task: a task that is aborted counts
        // as ended, too.
        let abort_handle = join_set.spawn(task.track_future(async move {
            let result = actor.run(actor_rx, actor_tx).await;
            if let Err(e) = result {
                tracing::error!("Actor {} stopped with an error: {:#}", actor.id(), e)
            }
            id
        }));

        // Wrap the actor's input sender with its declared filter predicate.
        let filtered_sender = Box::new(FilteringEventSender::new(
            Box::new(InMemoryEventSender::new(tx)),
            filter,
        ));

        RunningActor { id, abort_handle, task, sender: filtered_sender }
    }

    /// Whether every message `event` carries may enter the channel as its
    /// sender classified it, and every tool result in them as it is
    /// classified (`AccessQuestions::may_publish`, issue 133): the gate for every
    /// actor, whatever it checked itself. An event with a part that may not
    /// is dropped whole, and logged: its sender misclassified it.
    async fn admits(&self, event: &Envelope) -> bool {
        let channel = self.classification();
        for message in event.event.messages() {
            let parts = std::iter::once(&message.classification).chain(message.tool_results.iter().map(|r| &r.classification));
            for classification in parts {
                if !self.security.may_publish(&event.security, event.context.sender_id, classification, &channel).await {
                    warn!("Dropped {}: a part of its message {} is classified {}, which channel {} does not admit", event.describe(), message.id, classification, self.id);
                    return false;
                }
            }
        }
        true
    }

    /// Whether the change to a message that `event` carries, if any, may
    /// enter the channel (issue 234): the message is in its history, its
    /// author - the sender the history names - may change it, as the policy
    /// says in the envelope's context, and it can be changed so as it stands
    /// (`Message::modify`, `Message::revoke`): not taken back, and modified
    /// by its own sender in its own role. A reclassification (issue 239)
    /// likewise, but by its author or the channel's owner, as the policy
    /// says (`may_reclassify`), never above the channel. A request to
    /// answer again, too:
    /// its sender may ask it (`may_retry_message`), and it names the agent
    /// that said the message, not taken back, as its author. The gate for
    /// every actor, whatever it checked itself; anything else is dropped,
    /// and logged.
    async fn admits_change(&self, event: &Envelope) -> bool {
        let channel = self.security_attributes();
        let admitted = match &event.event {
            ActorEvent::MessageModified(modified) => match self.message(modified.id).await {
                Some(mut message) => self.security.may_modify_message(&event.security, message.sender_id, &channel).await && message.modify(modified),
                None => false,
            },
            ActorEvent::MessageRevoked { message, at } => match self.message(*message).await {
                Some(mut message) => self.security.may_revoke_message(&event.security, message.sender_id, &channel).await && message.revoke(*at),
                None => false,
            },
            ActorEvent::MessageReclassified { message, classification } => match self.message(*message).await {
                Some(mut said) => {
                    let current = said.classification.clone();
                    let target = Reclassified::Message { author: said.sender_id, attributes: &channel };
                    self.security.may_reclassify(&event.security, target, &current, classification, &self.classification()).await
                        && said.reclassify(classification)
                }
                None => false,
            },
            ActorEvent::RetryRequested { target: RetryTarget::Message(message), author } => match self.message(*message).await {
                Some(message) => self.security.may_retry_message(&event.security, &channel).await
                    && message.sender_id == *author && author.as_agent_id().is_some() && message.revoked.is_none(),
                None => false,
            },
            // Whether the turn is still under way is decided as the request
            // is delivered, where the turn is ended (`route_event_impl`).
            ActorEvent::RetryRequested { target: RetryTarget::Turn(_), author } => {
                self.security.may_retry_message(&event.security, &channel).await
                    && author.as_agent_id().is_some() && self.actor_by_id(*author).is_some()
            }
            _ => return true,
        };
        if !admitted {
            warn!("Dropped {}: the message is not one of channel {} its sender may change so", event.describe(), self.id);
        }
        admitted
    }

    /// The participants `text` mentions by their `@`-handles, as the channel
    /// resolves them when a message enters it (issue 29) - for an actor
    /// that shows its own message before (issue 220).
    pub(crate) fn mentions_in(&self, text: &str) -> Vec<ActorId> {
        self.mentions.load().resolve(text)
    }

    /// Resolves the `@`-mentions in every complete message carried by `event`
    /// and stores them in `Message::mentions`. Done here, once, as the message
    /// enters the channel, so that every frontend, the history and the
    /// moderator all see the same resolution (issue 29).
    fn resolve_mentions(&self, mut event: Envelope) -> Envelope {
        let resolve = |m: &mut Message| {
            if let Some(content) = &m.content {
                m.mentions = self.mentions_in(content);
            }
        };
        match &mut event.event {
            ActorEvent::CompleteMessageGenerated(m) | ActorEvent::IntermediateMessageGenerated(m) | ActorEvent::MessageModified(m) => resolve(m),
            ActorEvent::FullResponseGenerated(msgs) => msgs.iter_mut().for_each(resolve),
            _ => {}
        }
        event
    }

    async fn route_event(&self, running: &HashMap<ActorId, RunningActor>, event: Envelope) {
        // Asked for by an actor - a user's actor asks with every message it
        // sends (issue 90); the channel does not decide by itself.
        if matches!(event.event, ActorEvent::WakeupRequested) {
            self.wake_for_delivery();
        }

        self.route_event_impl(running, &event).await;
        #[cfg(test)]
        let _ = self.observed.send(event);
    }

    async fn route_event_impl(&self, running: &HashMap<ActorId, RunningActor>, event: &Envelope) {
        if event.context.span_depth >= self.max_route_decisions {
            warn!("Too many routings, stopping further propagation");
            return
        }

        let span_id = event.context.span_id;
        let span_depth = event.context.span_depth + 1;
        let decision = self.moderator.route(event).await;
        let quiet = self.quiet.load(Ordering::SeqCst);

        // Whom the moderator asks to respond is woken first, before anything
        // of the decision is delivered: a sleeping agent ignores a
        // `ResponseRequested`, and one that took in the message still asleep
        // would ignore the request that follows it. Since the policies never
        // choose a sleeping agent (`actor_candidates`), only the agents a
        // message mentions are woken this way (issue 45). A quiet channel
        // asks nobody, and so wakes nobody.
        if !quiet {
            let actors = self.actors.load();
            let asked = decision.iter()
                .filter(|(target_id, route)| *target_id != event.context.sender_id && matches!(route.event, ActorEvent::ResponseRequested | ActorEvent::RetryRequested { .. }));
            for (target_id, _) in asked {
                if let Some(ChannelActor { actor, .. }) = actors.get(target_id) {
                    actor.wakeup();
                }
            }
        }

        // First, route all messages to all agents...
        for route in decision {
            let target_id = route.0;

            // Do not propagate message to sender again - unless the
            // moderator sends something of its own in another name, e.g. a
            // `ChainCutOff` to the agent whose chain it cut off (issue 200).
            if target_id == event.context.sender_id && route.1.context.sender_id == event.context.sender_id {
                continue;
            }
            // A cancelled channel stays quiet until it is woken: what was said
            // still reaches everyone, so that every transcript keeps up with
            // the history, but nobody is asked to answer it.
            if quiet && matches!(route.1.event, ActorEvent::ResponseRequested) {
                debug!(to = %target_id, "Not asking for a response: the channel is quiet after a cancel");
                continue;
            }

            let mut target_event = route.1;
            let sender_id = target_event.context.sender_id;
            let event_desc = target_event.describe();
            // An answer under way asked for again (issue 235): its turn is
            // ended before the request is delivered, so that the agent finds
            // it given up as it takes the request in - and only if it is
            // still the turn under way. Otherwise there is nothing left to
            // answer again, and the request is dropped: its sender asked
            // before, and its users see the answer end.
            if let ActorEvent::RetryRequested { target: RetryTarget::Turn(turn), .. } = &target_event.event {
                if !self.actor_by_id(target_id).is_some_and(|actor| actor.retry_turn(*turn)) {
                    warn!(to = %target_id, "Dropped {}: turn {} is not under way", event_desc, turn);
                    continue;
                }
            }
            target_event.context.channel_id = self.id;
            target_event.context.session_id = self.session_id;
            target_event.context.span_id = span_id;
            target_event.context.span_depth = span_depth;
            if let Some(actor) = running.get(&target_id) {
                debug!(to = %target_id, "Forwarding {}", event_desc);
                self.record_send_event(&target_event).await;
                if let Err(e) = actor.sender.try_send(target_event) {
                    match e {
                        TransportError::Full => warn!("Queue full for actor {}", target_id),
                        _ => warn!("Could not forward event {} from={} to={}: {:?}", event_desc, sender_id, target_id, e)
                    }
                }
            }
        }
    }

    async fn append_history(&self, envelope: &Envelope) {
        let mut state = self.state.lock().await;
        let changed = match &envelope.event {
            ActorEvent::FullResponseGenerated(msgs) => {
                state.history.extend(msgs.clone());
                true
            }
            // Admitted, so they apply (`admits_change`, issue 234).
            ActorEvent::MessageModified(modified) => state.history.modify(modified.clone()),
            ActorEvent::MessageRevoked { message, at } => state.history.revoke(*message, *at),
            ActorEvent::MessageReclassified { message, classification } => state.history.reclassify(*message, classification.clone()),
            _ => false,
        };

        if changed {
            if let Err(e) = self.state_store.put_channel_state(self.id, &mut state).await {
                warn!("Cannot store state of channel {}: {}", self.id, e);
            }
        }
    }
    async fn record_send_event(&self, event: &Envelope) {
        let mut recorder = self.recorder.lock().await;
        recorder.record_send(&event).await;
        drop(recorder);
    }
    async fn record_receive_event(&self, event: &Envelope) {
        let mut recorder = self.recorder.lock().await;
        recorder.record_receive(&event).await;
        drop(recorder);
    }
}

/// The part of a channel's history a participant sees (issue 46), as
/// `ChannelLoop::visible_history` gives it.
#[derive(Debug, Clone)]
pub struct VisibleHistory {
    /// The messages they see, from `start` on.
    pub messages: Vec<Message>,
    /// The digest of the channel's history at the time: up to where it was
    /// read. It names the whole chain, so it is recorded with `start`.
    pub digest: Option<caelumora_model::digest::Digest>,
    /// Where their view begins - from where it was read; `None`: all of it.
    pub start: Option<HistoryStart>,
}

/// `history` as seen from `start`: all of it if there is none.
pub(crate) fn visible_part(start: Option<HistoryStart>, history: Vec<Message>) -> Vec<Message> {
    match start {
        Some(start) => visible_from(start, history),
        None => history,
    }
}

/// Where the view of `history` begins for a participant granted `access`,
/// fixed now - so that "the last three days" does not move day by day.
/// `None` if all of it is visible.
fn history_start(access: HistoryAccess, history: &[Message], now: chrono::DateTime<chrono::Utc>) -> Option<HistoryStart> {
    let last = || history.last().map(|m| HistoryStart::After(m.id));
    let from = |index: usize| match index {
        0 => None,
        i if i >= history.len() => last(),
        i => Some(HistoryStart::From(history[i].id)),
    };
    match access {
        HistoryAccess::All => None,
        HistoryAccess::None => last(),
        HistoryAccess::LastMessages { count } => from(history.len().saturating_sub(count as usize)),
        HistoryAccess::LastDays { count } => {
            let since = now - chrono::Duration::days(i64::from(count));
            from(history.iter().position(|m| m.timestamp >= since).unwrap_or(history.len()))
        }
    }
}

/// The part of `history` visible from `start` - nothing if the message it
/// begins at is not there, rather than more than was granted.
fn visible_from(start: HistoryStart, history: Vec<Message>) -> Vec<Message> {
    let (id, skip) = match start {
        HistoryStart::From(id) => (id, 0),
        HistoryStart::After(id) => (id, 1),
    };
    match history.iter().position(|m| m.id == id) {
        Some(index) => history.into_iter().skip(index + skip).collect(),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::value::name::{Name, ScopedName};
    use caelumora_model::store::EntityStore;
    use crate::command::CommandExecutor;
    use crate::command_impl::help_command::HelpCommandProvider;
    use crate::command_impl::wakeup_command::WakeupCommandProvider;
    use caelumora_model::component::ComponentFailPolicy;
    use crate::runtime::RuntimeManager;
    use caelumora_model::command::default_commands;
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{DummySecurityManager, SecurityAttributes, SecurityManager};
    use caelumora_model::store_impl::memory::MemoryStore;
    use caelumora_model::user::User;

    /// Refuses everything, to see that a refusal reaches the caller.
    #[derive(Debug)]
    struct DenyAll;

    #[async_trait::async_trait]
    impl SecurityManager for DenyAll {
        async fn check(&self, _context: &SecurityContext, _action: ActionType<'_>) -> bool {
            false
        }
    }

    /// Decides by a rule over the action alone, to refuse one thing and
    /// allow the rest.
    #[derive(Debug)]
    struct Policy(fn(ActionType<'_>) -> bool);

    #[async_trait::async_trait]
    impl SecurityManager for Policy {
        async fn check(&self, _context: &SecurityContext, action: ActionType<'_>) -> bool {
            (self.0)(action)
        }
    }

    fn user(username: &str) -> User {
        User {
            id: UserId::new(),
            username: Name::new(username).unwrap(),
            email: String::new(),
            roles: Vec::new(),
            deleted: None,
        }
    }

    /// A channel of `users`, who are recorded in the store its actors are
    /// read from.
    async fn channel_with(users: &[&User], security: Arc<dyn SecurityManager>) -> Arc<ChannelLoop> {
        let store = Arc::new(MemoryStore::new());
        let mut participants = Vec::new();
        for user in users {
            store.create_user(&SecurityContext::system(), user).await.unwrap();
            participants.push(Participant::User(user.id));
        }
        channel_of(store, participants, security).await
    }

    /// A channel of `participants`, whose actors are read from `store`.
    async fn channel_of(store: Arc<MemoryStore>, participants: Vec<Participant>, security: Arc<dyn SecurityManager>) -> Arc<ChannelLoop> {
        let registry = crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store));
        channel_built(store, participants, security, registry, caelumora_model::channel::moderation_components(None, Some("broadcast_policy"))).await
    }

    /// A channel of `participants` with `components`, built by `registry`.
    async fn channel_built(store: Arc<MemoryStore>, participants: Vec<Participant>, security: Arc<dyn SecurityManager>, registry: crate::component::ComponentRegistry, components: caelumora_model::component::ComponentConfig) -> Arc<ChannelLoop> {
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&security)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, security, default_commands(), Default::default(), Vec::new(), registry).unwrap());
        let channel = Channel {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            id: ChannelId::new(),
            session_id: SessionId::new(),
            name: Name::new("main").unwrap(),
            components,
            participants,
            security_attributes: SecurityAttributes::default(),
            classification: caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT,
            language: None,
            history_limits: Vec::new(),
            deleted: None,
            former: Vec::new(),
        };
        let builtin = |id: &str, facet: Arc<dyn crate::command::CommandProvider>| ComponentImpl { id: id.to_string(), fail_policy: ComponentFailPolicy::FailOpen, facet };
        let executor = CommandExecutor::new(
            Arc::clone(runtime.security_manager()),
            vec![builtin("help", Arc::new(HelpCommandProvider)), builtin("wakeup", Arc::new(WakeupCommandProvider))],
        ).unwrap();
        let session_id = channel.session_id;
        ChannelLoop::new(channel, ChannelRuntime::outside_a_session(runtime, Arc::new(executor), session_id)).await.unwrap()
    }

    #[tokio::test]
    async fn a_participant_that_no_longer_exists_is_left_out() {
        let alice = user("alice");
        let store = Arc::new(MemoryStore::new());
        store.create_user(&SecurityContext::system(), &alice).await.unwrap();
        let (gone_user, gone_agent) = (UserId::new(), caelumora_model::agent::AgentId::new());

        let channel = channel_of(
            store,
            vec![Participant::User(alice.id), Participant::User(gone_user), Participant::Agent(gone_agent)],
            Arc::new(DummySecurityManager::new()),
        ).await;

        assert!(channel.user_actor(alice.id).is_some(), "the others take part");
        assert!(channel.actor_by_id(ActorId::User(gone_user)).is_none(), "a deleted user has no actor");
        assert!(channel.actor_by_id(ActorId::Agent(gone_agent)).is_none(), "nor has a deleted agent");
    }

    #[tokio::test]
    async fn every_user_has_their_own_actor_and_a_stranger_has_none() {
        let alice = user("alice");
        let bob = user("bob");
        let channel = channel_with(&[&alice, &bob], Arc::new(DummySecurityManager::new())).await;

        for participant in [&alice, &bob] {
            let actor = channel.user_actor(participant.id).expect("a participant has an actor");
            assert_eq!(actor.id(), ActorId::User(participant.id));
            assert_eq!(actor.name(), participant.username.as_str(), "the actor writes in this user's name");
        }
        assert!(channel.user_actor(UserId::new()).is_none(), "someone who is not a participant has no actor");
    }

    #[tokio::test]
    async fn cancelling_and_waking_deliberately_ask_the_security_manager() {
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let refused = channel_with(&[&alice], Arc::new(DenyAll)).await;
        // Refused before the actor is looked up: who may not learns nothing
        // of who is in the channel.
        let somebody = ActorId::Agent(caelumora_model::agent::AgentId::new());
        for error in [
            refused.cancel(&context).await.expect_err("cancelling is refused"),
            refused.cancel_actor(&context, somebody).await.expect_err("cancelling one actor is refused"),
            refused.wakeup(&context).await.expect_err("waking is refused"),
            refused.wakeup_actor(&context, somebody).await.expect_err("waking one actor is refused"),
        ] {
            assert!(crate::error::is_forbidden(&error), "the caller can tell a refusal from a failure: {error}");
            assert!(!error.to_string().contains("DenyAll"), "a refusal does not disclose the rule: {error}");
        }
        // Waking because something is on its way is not a wakeup of its own:
        // it follows from an action that was permitted.
        refused.wake_for_delivery();

        let allowed = channel_with(&[&alice], Arc::new(DummySecurityManager::new())).await;
        allowed.cancel(&context).await.expect("permitted");
        allowed.wakeup(&context).await.expect("permitted");
    }

    /// Five messages, one a day, the last one `now`.
    fn daily(now: chrono::DateTime<chrono::Utc>) -> Vec<Message> {
        (0..5).rev()
            .map(|days| Message { id: caelumora_model::message::MessageId::new(), timestamp: now - chrono::Duration::days(days), ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) })
            .collect()
    }

    fn ids(history: &[Message]) -> Vec<caelumora_model::message::MessageId> {
        history.iter().map(|m| m.id).collect()
    }

    fn granted(access: HistoryAccess, history: &[Message], now: chrono::DateTime<chrono::Utc>) -> Vec<caelumora_model::message::MessageId> {
        let messages = match history_start(access, history, now) {
            Some(start) => visible_from(start, history.to_vec()),
            None => history.to_vec(),
        };
        ids(&messages)
    }

    #[test]
    fn history_access_all_and_none_grant_everything_and_nothing() {
        let now = chrono::Utc::now();
        let history = daily(now);
        assert_eq!(granted(HistoryAccess::All, &history, now), ids(&history), "All sees every message");
        assert!(granted(HistoryAccess::None, &history, now).is_empty(), "None sees no earlier message");
        assert_eq!(history_start(HistoryAccess::None, &[], now), None, "an empty channel has nothing to hide");
    }

    #[test]
    fn history_access_last_messages_and_days_cut_the_history() {
        let now = chrono::Utc::now();
        let history = daily(now);
        assert_eq!(granted(HistoryAccess::LastMessages { count: 2 }, &history, now), ids(&history[3..]));
        assert_eq!(granted(HistoryAccess::LastMessages { count: 9 }, &history, now), ids(&history),
                   "more than there is grants all of it");
        assert!(granted(HistoryAccess::LastMessages { count: 0 }, &history, now).is_empty());
        // Messages 1, 2 and 3 days ago plus today's - within the last three days.
        assert_eq!(granted(HistoryAccess::LastDays { count: 3 }, &history, now), ids(&history[1..]));
        assert!(granted(HistoryAccess::LastDays { count: 3 }, &history, now + chrono::Duration::days(10)).is_empty(),
                "nothing within the last days sees nothing");
    }

    #[test]
    fn a_history_start_shows_later_messages_and_nothing_if_its_message_is_gone() {
        let now = chrono::Utc::now();
        let mut later = daily(now);
        let start = HistoryStart::After(later[1].id);
        later.push(Message { id: caelumora_model::message::MessageId::new(), ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) });
        assert_eq!(ids(&visible_from(start, later.clone())), ids(&later[2..]), "messages sent after joining stay visible");
        assert!(visible_from(HistoryStart::From(caelumora_model::message::MessageId::new()), later).is_empty(),
                "an unknown start shows nothing rather than everything");
    }

    fn command_error(error: &anyhow::Error) -> &crate::command::CommandError {
        error.downcast_ref().unwrap_or_else(|| panic!("expected a CommandError, got {error:#}"))
    }

    #[tokio::test]
    async fn help_answers_the_issuer_alone_and_lists_what_they_may_see() {
        let alice = user("alice");
        // `/wakeup` is hidden from everybody.
        let policy = Policy(|action| !matches!(action, ActionType::ShowCommand("wakeup")));
        let channel = channel_with(&[&alice], Arc::new(policy)).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let mut events = channel.observe();
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

        let reply = actor.execute_command("help", "").await.expect("/help runs");
        assert_eq!(reply.visibility, caelumora_model::command::CommandVisibility::Private, "/help answers the issuer alone");
        let text = reply.output;
        assert!(text.contains("/help"), "{text}");
        assert!(!text.contains("/wakeup"), "a hidden command is not listed: {text}");
        let names: Vec<String> = actor.visible_commands().await.into_iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["help"]);

        let error = actor.execute_command("wakeup", "").await.expect_err("hidden");
        assert!(matches!(command_error(&error), crate::command::CommandError::Unknown(_)), "a hidden command looks unknown: {error}");
        let error = actor.execute_command("help", "wakeup").await.expect_err("hidden");
        assert!(matches!(command_error(&error), crate::command::CommandError::Unknown(_)), "/help does not describe it either: {error}");

        let quiet = tokio::time::timeout(std::time::Duration::from_millis(200), events.recv()).await;
        assert!(quiet.is_err(), "a private answer is not sent into the channel: {quiet:?}");
        assert!(channel.history().await.is_empty(), "commands never enter the history");
        running.abort();
    }

    #[tokio::test]
    async fn wakeup_is_announced_to_the_channel_in_the_issuers_name() {
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(DummySecurityManager::new())).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let mut events = channel.observe();
        let mut following = actor.watch().events;
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

        let reply = actor.execute_command("wakeup", "").await.expect("/wakeup runs");
        assert!(matches!(following.try_recv().map(|e| e.event), Ok(ActorEvent::CommandExecuted { .. })), "the issuer sees it from its own actor (issue 220)");
        assert_eq!(reply.visibility, caelumora_model::command::CommandVisibility::Channel, "the answer goes to the channel");
        assert_eq!(reply.output, "No agent was asleep.", "and back to the issuer, whom the channel does not route it to");

        let envelope = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
            .await
            .expect("the announcement arrives")
            .unwrap();
        assert_eq!(envelope.context.sender_id, ActorId::User(alice.id));
        match envelope.event {
            ActorEvent::CommandExecuted { command, output } => {
                assert_eq!(command, "wakeup");
                assert_eq!(output, "No agent was asleep.");
            }
            other => panic!("expected CommandExecuted, got {:?}", other.kind()),
        }
        assert!(channel.history().await.is_empty(), "commands never enter the history");
        running.abort();
    }

    #[tokio::test]
    async fn wakeup_reports_refusals_and_unknown_agents() {
        let alice = user("alice");
        let refuse_waking = Policy(|action| !matches!(action, ActionType::WakeChannel(_)));
        let channel = channel_with(&[&alice], Arc::new(refuse_waking)).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let error = actor.execute_command("wakeup", "").await.expect_err("waking is refused");
        assert!(matches!(command_error(&error), crate::command::CommandError::Forbidden(_)), "{error}");
        assert!(crate::error::is_forbidden(&error), "{error}");

        let refuse_running = Policy(|action| !matches!(action, ActionType::ExecuteCommand("wakeup")));
        let channel = channel_with(&[&alice], Arc::new(refuse_running)).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let error = actor.execute_command("wakeup", "").await.expect_err("running it is refused");
        assert!(matches!(command_error(&error), crate::command::CommandError::Forbidden(_)), "visible but not runnable: {error}");

        let channel = channel_with(&[&alice], Arc::new(DummySecurityManager::new())).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let error = actor.execute_command("wakeup", "@nobody").await.expect_err("no such agent");
        assert!(matches!(command_error(&error), crate::command::CommandError::InvalidArguments { .. }), "{error}");
        let error = actor.execute_command("", "").await.expect_err("empty");
        assert!(matches!(command_error(&error), crate::command::CommandError::Empty), "{error}");
        let error = actor.execute_command("nonsense", "").await.expect_err("unknown");
        assert!(matches!(command_error(&error), crate::command::CommandError::Unknown(_)), "{error}");
    }

    /// Answers every event with the same decision: tell `to` what was said,
    /// and ask it to respond.
    #[derive(Debug)]
    struct AskBack(ActorId);

    #[async_trait::async_trait]
    impl crate::moderator::Moderator for AskBack {
        async fn route(&self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
            vec![(self.0, event.clone()), (self.0, event.forward(ActorEvent::ResponseRequested))]
        }
        fn add_actor(&self, _: Arc<dyn Actor>) {}
        fn remove_actor(&self, _: ActorId) {}
    }

    /// A channel of `users` whose moderator is `moderator`: a component of a
    /// kind of its own (issue 174).
    async fn channel_moderated_by(users: &[&User], moderator: Arc<dyn crate::moderator::Moderator>) -> Arc<ChannelLoop> {
        use crate::component::{ComponentFacets, ComponentKind};
        let store = Arc::new(MemoryStore::new());
        let mut participants = Vec::new();
        for user in users {
            store.create_user(&SecurityContext::system(), user).await.unwrap();
            participants.push(Participant::User(user.id));
        }
        let mut registry = crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store));
        registry.register(ComponentKind::new("test_moderator").provides::<dyn crate::moderator::Moderator>(), move |_, _| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn crate::moderator::Moderator>(Arc::clone(&moderator));
            Ok(facets)
        });
        let components = caelumora_model::channel::moderation_components(Some("test_moderator"), None);
        channel_built(store, participants, Arc::new(DummySecurityManager::new()), registry, components).await
    }

    #[tokio::test]
    async fn a_cancelled_channel_delivers_what_is_said_but_asks_nobody_until_it_is_asked_to_wake() {
        use caelumora_model::actor::EventContext;
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let bob = ActorId::Agent(caelumora_model::agent::AgentId::new());
        let channel = channel_moderated_by(&[&alice], Arc::new(AskBack(bob))).await;
        let (tx, mut rx) = mpsc::channel(8);
        let abort_handle = tokio::spawn(async {}).abort_handle();
        let actors = HashMap::from([(bob, RunningActor { id: bob, abort_handle, task: TaskTracker::new(), sender: Box::new(InMemoryEventSender::new(tx)) })]);
        let from_alice = |event| Envelope::new(
            context.clone(),
            EventContext { sender_id: ActorId::User(alice.id), ..EventContext::default() },
            event,
        );
        let said = || from_alice(ActorEvent::FullResponseGenerated(vec![Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)]));
        let delivered = |rx: &mut mpsc::Receiver<Envelope>| {
            let mut kinds = Vec::new();
            while let Ok(envelope) = rx.try_recv() { kinds.push(envelope.event.kind()); }
            kinds
        };

        channel.cancel(&context).await.expect("permitted");
        channel.route_event(&actors, said()).await;
        assert_eq!(delivered(&mut rx), ["FullResponseGenerated"], "what was said still arrives, but nobody is asked");

        // A message alone does not wake it: its sender asks for that, as a
        // user's actor does with every message (issue 90).
        channel.route_event(&actors, from_alice(ActorEvent::WakeupRequested)).await;
        delivered(&mut rx); // this test's moderator forwards the request, too
        channel.route_event(&actors, said()).await;
        assert_eq!(delivered(&mut rx), ["FullResponseGenerated", "ResponseRequested"], "woken, the channel asks again");
    }

    #[tokio::test]
    async fn a_users_own_message_is_shown_with_its_mentions_resolved() {
        let (alice, bob) = (user("alice"), user("bob"));
        let channel = channel_with(&[&alice, &bob], Arc::new(DummySecurityManager::new())).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let mut following = actor.watch().events;

        actor.send_message("@bob, look".to_string(), None).await.expect("sent");

        let shown = following.try_recv().expect("shown at once, before the channel takes it in");
        let ActorEvent::CompleteMessageGenerated(message) = shown.event else { panic!("its message, got {}", shown.event.kind()) };
        assert_eq!(message.mentions, vec![ActorId::User(bob.id)], "as the channel resolves them (issue 220)");
    }

    /// Sends every event back to its sender, once as it is and once as a
    /// `ChainCutOff` of its own, in the name of the system.
    #[derive(Debug)]
    struct TellsTheSender;

    #[async_trait::async_trait]
    impl crate::moderator::Moderator for TellsTheSender {
        async fn route(&self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
            let mut notice = event.forward(ActorEvent::ChainCutOff { message: MessageId::new(), max_agent_hops: 0, not_asked: Vec::new() });
            notice.context.sender_id = ActorId::System;
            vec![(event.context.sender_id, event.clone()), (event.context.sender_id, notice)]
        }
        fn add_actor(&self, _: Arc<dyn Actor>) {}
        fn remove_actor(&self, _: ActorId) {}
    }

    #[tokio::test]
    async fn the_sender_gets_back_only_what_the_moderator_sends_in_another_name() {
        use caelumora_model::actor::EventContext;
        let alice = user("alice");
        let bob = ActorId::Agent(caelumora_model::agent::AgentId::new());
        let channel = channel_moderated_by(&[&alice], Arc::new(TellsTheSender)).await;
        let (tx, mut rx) = mpsc::channel(8);
        let abort_handle = tokio::spawn(async {}).abort_handle();
        let actors = HashMap::from([(bob, RunningActor { id: bob, abort_handle, task: TaskTracker::new(), sender: Box::new(InMemoryEventSender::new(tx)) })]);
        let said = Envelope::new(
            SecurityContext::default(),
            EventContext { sender_id: bob, ..EventContext::default() },
            ActorEvent::FullResponseGenerated(vec![Message::new(Classification::CHANNEL_DEFAULT)]),
        );

        channel.route_event(&actors, said).await;
        let mut kinds = Vec::new();
        while let Ok(envelope) = rx.try_recv() { kinds.push(envelope.event.kind()); }
        assert_eq!(kinds, ["ChainCutOff"], "its own message is not sent back, but what the system tells it is (issue 200)");
    }

    /// An agent that says one thing as it starts, as the answer to a request
    /// one hop into a chain between agents, and waits.
    #[derive(Debug)]
    struct SaysOnce(ActorId);

    #[async_trait::async_trait]
    impl Actor for SaysOnce {
        fn id(&self) -> ActorId { self.0 }
        fn kind(&self) -> String { "says_once".to_string() }
        fn name(&self) -> String { "talker".to_string() }
        fn event_filter(&self) -> fn(&Envelope) -> bool { |_| true }
        async fn run(&self, mut rx: Box<dyn crate::transport::EventReceiver>, tx: Box<dyn EventSender>) -> Result<()> {
            use caelumora_model::actor::EventContext;
            let mut context = EventContext { sender_id: self.0, ..EventContext::default() };
            context.agent_hops = 1;
            let said = Message { sender_id: self.0, role: caelumora_model::message::MessageRole::Assistant, content: Some("and you?".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
            let _ = tx.send(Envelope::new(SecurityContext::default().next(self.0), context, ActorEvent::FullResponseGenerated(vec![said]))).await;
            while rx.recv().await.is_some() {}
            Ok(())
        }
        fn state(&self) -> ActorState { ActorState::Idle }
        fn wakeup(&self) {}
        fn stop(&self) {}
    }

    #[tokio::test]
    async fn a_user_following_the_channel_is_told_when_a_chain_is_cut_off() {
        use crate::moderator_impl::policy_moderator::PolicyModerator;
        use caelumora_model::approval::{ApprovalStrategy, ApproverId};
        let alice = user("alice");
        let responders = ComponentImpl { id: "responders".to_string(), fail_policy: Default::default(), facet: Arc::new(crate::moderator_impl::broadcast_policy::BroadcastPolicy) as Arc<dyn crate::moderator_impl::ResponderPolicy> };
        let moderator = Arc::new(PolicyModerator::new(responders, 1));
        let channel = channel_moderated_by(&[&alice], Arc::<PolicyModerator>::clone(&moderator)).await;
        let talker = Arc::new(SaysOnce(ActorId::Agent(caelumora_model::agent::AgentId::new())));
        channel.adopt(Arc::<SaysOnce>::clone(&talker), Member::Approver(Approver { id: ApproverId::new(), strategy: ApprovalStrategy::Any })).await;
        let mut following = channel.user_actor(alice.id).unwrap().watch().events;
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

        let told = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let ActorEvent::ChainCutOff { max_agent_hops, .. } = following.recv().await.expect("the channel runs").event {
                    break max_agent_hops;
                }
            }
        }).await.expect("the web UI follows the user's actor, which is told (issues 200, 220)");
        assert_eq!(told, 1);
        running.abort();
    }

    /// An actor that says what it is given to say as it starts, as it is
    /// written - in whatever name - and waits.
    #[derive(Debug)]
    struct Says(ActorId, std::sync::Mutex<Vec<Envelope>>);

    #[async_trait::async_trait]
    impl Actor for Says {
        fn id(&self) -> ActorId { self.0 }
        fn kind(&self) -> String { "says".to_string() }
        fn name(&self) -> String { "talker".to_string() }
        fn event_filter(&self) -> fn(&Envelope) -> bool { |_| true }
        async fn run(&self, mut rx: Box<dyn crate::transport::EventReceiver>, tx: Box<dyn EventSender>) -> Result<()> {
            let said = std::mem::take(&mut *self.1.lock().unwrap());
            for envelope in said {
                let _ = tx.send(envelope).await;
            }
            while rx.recv().await.is_some() {}
            Ok(())
        }
        fn state(&self) -> ActorState { ActorState::Idle }
        fn wakeup(&self) {}
        fn stop(&self) {}
    }

    #[tokio::test]
    async fn an_actor_cannot_speak_in_the_name_of_another() {
        use caelumora_model::approval::{ApprovalStrategy, ApproverId};
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(DummySecurityManager::new())).await;
        let talker = ActorId::Agent(caelumora_model::agent::AgentId::new());
        let her = ActorId::User(alice.id);
        let said = |sender: ActorId, caller: ActorId, author: ActorId, text: &str| {
            let message = Message { sender_id: author, role: caelumora_model::message::MessageRole::Assistant, content: Some(text.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
            Envelope::new(SecurityContext::default().next(caller), EventContext { sender_id: sender, ..EventContext::default() }, ActorEvent::FullResponseGenerated(vec![message]))
        };
        let envelopes = vec![
            said(her, talker, talker, "as alice's envelope"),
            said(talker, her, talker, "in alice's security context"),
            said(talker, talker, her, "as alice's message"),
            // On alice's behalf - she asked - but said by itself: as an agent answers.
            Envelope { security: SecurityContext::new(her, channel.session_id, channel.id).next(talker), ..said(talker, talker, talker, "its own") },
        ];
        let says = Arc::new(Says(talker, std::sync::Mutex::new(envelopes)));
        channel.adopt(Arc::<Says>::clone(&says), Member::Approver(caelumora_model::approval::Approver { id: ApproverId::new(), strategy: ApprovalStrategy::Any })).await;
        let mut events = channel.observe();
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

        let entered = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let ActorEvent::FullResponseGenerated(messages) = events.recv().await.expect("the channel runs").event {
                    break messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
                }
            }
        }).await.expect("what it said in its own name enters");
        assert_eq!(entered, ["its own"], "nothing it said in another's name entered before it");
        let history: Vec<_> = channel.history().await.iter().filter_map(|m| m.content.clone()).collect();
        assert_eq!(history, ["its own"], "nor the history");
        running.abort();
    }

    /// An agent asleep that only tells whether it was woken or cancelled.
    #[derive(Debug)]
    struct Sleeper {
        id: ActorId,
        woken: AtomicBool,
        cancelled: AtomicBool,
    }

    #[async_trait::async_trait]
    impl Actor for Sleeper {
        fn id(&self) -> ActorId { self.id }
        fn kind(&self) -> String { "sleeper".to_string() }
        fn name(&self) -> String { "sleeper".to_string() }
        fn event_filter(&self) -> fn(&Envelope) -> bool { |_| true }
        async fn run(&self, _rx: Box<dyn crate::transport::EventReceiver>, _tx: Box<dyn EventSender>) -> Result<()> { Ok(()) }
        fn state(&self) -> ActorState {
            if self.woken.load(Ordering::SeqCst) { ActorState::Idle } else { ActorState::Sleeping }
        }
        fn wakeup(&self) { self.woken.store(true, Ordering::SeqCst); }
        fn cancel(&self) { self.cancelled.store(true, Ordering::SeqCst); }
        fn stop(&self) {}
    }

    fn sleeper() -> Arc<Sleeper> {
        Arc::new(Sleeper { id: ActorId::Agent(caelumora_model::agent::AgentId::new()), woken: AtomicBool::new(false), cancelled: AtomicBool::new(false) })
    }

    #[tokio::test]
    async fn cancelling_one_actor_ends_its_turn_alone_and_keeps_the_channel_answering() {
        use caelumora_model::actor::EventContext;
        use caelumora_model::approval::{ApprovalStrategy, ApproverId};
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let (cancelled, other) = (sleeper(), sleeper());
        let channel = channel_moderated_by(&[&alice], Arc::new(AskBack(other.id))).await;
        for actor in [&cancelled, &other] {
            channel.adopt(Arc::<Sleeper>::clone(actor), Member::Approver(Approver { id: ApproverId::new(), strategy: ApprovalStrategy::Any })).await;
        }

        channel.cancel_actor(&context, cancelled.id).await.expect("permitted");
        assert!(cancelled.cancelled.load(Ordering::SeqCst), "its turn is ended");
        assert!(!other.cancelled.load(Ordering::SeqCst), "the other's goes on");

        let said = Envelope::new(
            context.clone(),
            EventContext { sender_id: ActorId::User(alice.id), ..EventContext::default() },
            ActorEvent::FullResponseGenerated(vec![Message::new(Classification::CHANNEL_DEFAULT)]),
        );
        channel.route_event(&HashMap::new(), said).await;
        assert!(other.woken.load(Ordering::SeqCst), "the channel is not quiet: the next message is answered");

        let stranger = ActorId::Agent(caelumora_model::agent::AgentId::new());
        let error = channel.cancel_actor(&context, stranger).await.expect_err("not in the channel");
        assert!(matches!(error.downcast_ref::<ChannelError>(), Some(ChannelError::NotAParticipant { .. })), "{error}");
        let error = channel.wakeup_actor(&context, stranger).await.expect_err("nor to be woken");
        assert!(matches!(error.downcast_ref::<ChannelError>(), Some(ChannelError::NotAParticipant { .. })), "the same error: {error}");
    }

    #[tokio::test]
    async fn the_channel_wakes_only_whom_the_moderator_asks_to_respond() {
        use caelumora_model::actor::EventContext;
        use caelumora_model::approval::{ApprovalStrategy, ApproverId};
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let (asked, other) = (sleeper(), sleeper());
        let channel = channel_moderated_by(&[&alice], Arc::new(AskBack(asked.id))).await;
        for actor in [&asked, &other] {
            // Who it is does not matter to routing: the channel only looks it up.
            channel.adopt(Arc::<Sleeper>::clone(actor), Member::Approver(Approver { id: ApproverId::new(), strategy: ApprovalStrategy::Any })).await;
        }
        let said = || Envelope::new(
            context.clone(),
            EventContext { sender_id: ActorId::User(alice.id), ..EventContext::default() },
            ActorEvent::FullResponseGenerated(vec![Message::new(Classification::CHANNEL_DEFAULT)]),
        );

        channel.route_event(&HashMap::new(), said()).await;
        assert!(asked.woken.load(Ordering::SeqCst), "asked to respond, it is woken to do so");
        assert!(!other.woken.load(Ordering::SeqCst), "not addressed, it sleeps on");

        // A quiet channel asks nobody, and wakes nobody.
        asked.woken.store(false, Ordering::SeqCst);
        channel.cancel(&context).await.expect("permitted");
        channel.route_event(&HashMap::new(), said()).await;
        assert!(!asked.woken.load(Ordering::SeqCst), "nobody is asked in a quiet channel");
    }

    #[tokio::test]
    async fn a_channel_admits_only_what_may_flow_into_it_in_its_senders_name() {
        use caelumora_model::actor::EventContext;
        use caelumora_model::confidentiality::Confidentiality;
        use caelumora_model::security::OwnershipSecurityManager;
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(OwnershipSecurityManager::new())).await;
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(UserId::new()));
        let from = |sender: ActorId, context: ActorId, classification: Classification| Envelope::new(
            SecurityContext::new(context, Default::default(), Default::default()),
            EventContext { sender_id: sender, ..EventContext::default() },
            ActorEvent::FullResponseGenerated(vec![Message::new(classification)]),
        );
        let internal = Classification::general(Confidentiality::Internal);
        let actors = HashMap::new();
        channel.process_envelope(from(alice_id, alice_id, internal.clone()), &actors).await;
        channel.process_envelope(from(alice_id, alice_id, Classification::general(Confidentiality::Confidential)), &actors).await;
        channel.process_envelope(from(alice_id, bob_id, internal.clone()), &actors).await;
        assert_eq!(channel.history().await.len(), 1, "neither above the channel nor in another's name");

        // A tool result is a part of its own.
        let call = caelumora_model::tool::ToolCall { id: None, function_name: "f".to_string(), arguments: serde_json::json!({}), visibility: Default::default(), presentation: Default::default() };
        let result = caelumora_model::tool::ToolCallResult::of(&call, Default::default(), "secret".to_string(), Classification::general(Confidentiality::Confidential));
        let mut with_result = from(alice_id, alice_id, internal);
        if let ActorEvent::FullResponseGenerated(messages) = &mut with_result.event {
            messages[0].tool_results.push(result);
        }
        channel.process_envelope(with_result, &actors).await;
        assert_eq!(channel.history().await.len(), 1, "nor with a result above the channel");
    }

    #[tokio::test]
    async fn a_change_to_a_message_enters_the_channel_only_from_its_author_as_it_stands() {
        use caelumora_model::actor::EventContext;
        use caelumora_model::security::OwnershipSecurityManager;
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(OwnershipSecurityManager::new())).await;
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(UserId::new()));
        let from = |sender: ActorId, event| Envelope::new(
            SecurityContext::new(sender, Default::default(), Default::default()),
            EventContext { sender_id: sender, ..EventContext::default() },
            event,
        );
        let said = Message { id: MessageId::new(), sender_id: alice_id, role: caelumora_model::message::MessageRole::User, content: Some("helo".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let modified = |sender: ActorId, content: &str| Message { sender_id: sender, content: Some(content.to_string()), timestamp: chrono::Utc::now(), ..said.clone() };
        let revoked = || ActorEvent::MessageRevoked { message: said.id, at: chrono::Utc::now() };
        let actors = HashMap::new();
        channel.process_envelope(from(alice_id, ActorEvent::FullResponseGenerated(vec![said.clone()])), &actors).await;
        let entries = || async { channel.state.lock().await.history.all_entries().len() };

        channel.process_envelope(from(bob_id, ActorEvent::MessageModified(modified(bob_id, "bob's"))), &actors).await;
        channel.process_envelope(from(bob_id, revoked()), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::MessageModified(Message { id: MessageId::new(), ..modified(alice_id, "nothing said") })), &actors).await;
        assert_eq!(entries().await, 1, "neither by another nor of what was never said");

        channel.process_envelope(from(alice_id, ActorEvent::MessageModified(modified(alice_id, "hello"))), &actors).await;
        let standing = channel.message(said.id).await.expect("said");
        assert_eq!((standing.content.as_deref(), standing.modified.is_some()), (Some("hello"), true), "its author modifies it");

        // Reclassified as the policy says (issue 239): here, with nobody
        // owning the channel, by its author alone.
        let reclassified = |classification| ActorEvent::MessageReclassified { message: said.id, classification };
        let above = Classification::general(caelumora_model::confidentiality::Confidentiality::Confidential);
        channel.process_envelope(from(bob_id, reclassified(Classification::PUBLIC)), &actors).await;
        channel.process_envelope(from(alice_id, reclassified(above)), &actors).await;
        assert_eq!(entries().await, 2, "neither by another nor above the channel");
        channel.process_envelope(from(alice_id, reclassified(Classification::PUBLIC)), &actors).await;
        assert_eq!(channel.message(said.id).await.map(|m| m.classification), Some(Classification::PUBLIC), "its author releases it");

        channel.process_envelope(from(alice_id, revoked()), &actors).await;
        assert!(channel.message(said.id).await.is_some_and(|m| m.revoked.is_some() && m.content.is_none()), "and takes it back");
        channel.process_envelope(from(alice_id, revoked()), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::MessageModified(modified(alice_id, "again"))), &actors).await;
        channel.process_envelope(from(alice_id, reclassified(Classification::CHANNEL_DEFAULT)), &actors).await;
        assert_eq!(entries().await, 4, "nothing changes it once taken back");
        assert_eq!(channel.history().await.len(), 1, "it stays in its place");

        // Only an agent is asked to answer again, and only for its own message.
        let mut routed = channel.observe();
        channel.process_envelope(from(alice_id, ActorEvent::RetryRequested { target: RetryTarget::Message(said.id), author: alice_id }), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::RetryRequested { target: RetryTarget::Message(said.id), author: ActorId::Agent(caelumora_model::agent::AgentId::new()) }), &actors).await;
        // An answer under way only of an agent of the channel.
        channel.process_envelope(from(alice_id, ActorEvent::RetryRequested { target: RetryTarget::Turn(TurnId::new()), author: alice_id }), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::RetryRequested { target: RetryTarget::Turn(TurnId::new()), author: ActorId::Agent(caelumora_model::agent::AgentId::new()) }), &actors).await;
        assert!(routed.try_recv().is_err(), "neither a user's message nor one another said, nor a turn of someone who is no agent here");
    }

    #[tokio::test]
    async fn a_user_changes_only_their_own_message_and_learns_why_not() {
        use caelumora_model::actor::EventContext;
        use caelumora_model::security::OwnershipSecurityManager;
        let (alice, bob) = (user("alice"), user("bob"));
        let channel = channel_with(&[&alice, &bob], Arc::new(OwnershipSecurityManager::new())).await;
        let alice_id = ActorId::User(alice.id);
        let said = Message { id: MessageId::new(), sender_id: alice_id, role: caelumora_model::message::MessageRole::User, content: Some("helo".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let from_alice = |event| Envelope::new(SecurityContext::new(alice_id, Default::default(), Default::default()), EventContext { sender_id: alice_id, ..EventContext::default() }, event);
        channel.process_envelope(from_alice(ActorEvent::FullResponseGenerated(vec![said.clone()])), &HashMap::new()).await;
        let (hers, his) = (channel.user_actor(alice.id).unwrap(), channel.user_actor(bob.id).unwrap());
        let error = |result: anyhow::Result<uuid::Uuid>| result.expect_err("refused").downcast::<ChannelError>().expect("a channel's error");

        assert!(matches!(error(his.modify_message(said.id, "bob's".to_string(), None).await), ChannelError::Forbidden { .. }), "not his");
        assert!(matches!(error(his.revoke_message(said.id).await), ChannelError::Forbidden { .. }), "not his to take back");
        assert!(matches!(error(hers.revoke_message(MessageId::new()).await), ChannelError::MessageNotFound { .. }), "never said");
        let above = Classification::general(caelumora_model::confidentiality::Confidentiality::Confidential);
        assert!(matches!(error(hers.modify_message(said.id, "hello".to_string(), Some(above.clone())).await), ChannelError::ClassificationRefused { .. }), "not above the channel");
        assert!(matches!(error(hers.request_retry(said.id).await), ChannelError::NotAnAgentsMessage { .. }), "no agent said it");
        assert!(matches!(error(his.request_retry(MessageId::new()).await), ChannelError::MessageNotFound { .. }));
        assert!(matches!(error(his.request_retry_of_turn(alice_id, TurnId::new()).await), ChannelError::NotAParticipant { .. }), "no agent answers here");
        assert!(matches!(error(his.reclassify_message(said.id, Classification::PUBLIC).await), ChannelError::Forbidden { .. }), "not his to release");
        assert!(matches!(error(hers.reclassify_message(said.id, above.clone()).await), ChannelError::Forbidden { .. }), "never above the channel");
        assert!(matches!(error(hers.reclassify_message(MessageId::new(), Classification::PUBLIC).await), ChannelError::MessageNotFound { .. }));
        hers.modify_message(said.id, "hello".to_string(), None).await.expect("hers to modify");
        hers.reclassify_message(said.id, Classification::PUBLIC).await.expect("and to release");
        hers.revoke_message(said.id).await.expect("and to take back");

        channel.process_envelope(from_alice(ActorEvent::MessageRevoked { message: said.id, at: chrono::Utc::now() }), &HashMap::new()).await;
        assert!(matches!(error(hers.modify_message(said.id, "hello".to_string(), None).await), ChannelError::MessageRevoked { .. }), "taken back, it stays so");
        assert!(matches!(error(hers.reclassify_message(said.id, Classification::PUBLIC).await), ChannelError::MessageRevoked { .. }), "with nothing left to release");
    }

    #[tokio::test]
    async fn an_agent_takes_a_change_in_after_its_turn_and_is_given_what_stands_from_then_on() {
        let model = Arc::new(Gated {
            answers: std::sync::Mutex::new(["first", "second"].into()),
            gate: Arc::new(tokio::sync::Semaphore::new(0)),
            seen: Default::default(),
        });
        let Stuck { channel, context, agent, mut events, running, .. } = with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while model.seen.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("the agent asks its model about hello");
        let alice = channel.user_actor(alice_of(&context)).unwrap();
        let hello = channel.history().await[0].id;
        let until = |what: &'static str, done: Box<dyn Fn(&caelumora_model::agent::AgentChannelState) -> bool + Send>| {
            let channel = Arc::clone(&channel);
            async move {
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        if let Ok(state) = channel.state_store.get_agent_channel_state(agent, channel.id).await {
                            if done(&state) { break }
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }).await.unwrap_or_else(|_| panic!("{what}"));
            }
        };

        // Modified while the turn about it runs: the turn ends as it began.
        alice.modify_message(hello, "good evening".to_string(), None).await.expect("hers");
        tokio::time::timeout(Duration::from_secs(5), async {
            while channel.message(hello).await.and_then(|m| m.content).as_deref() != Some("good evening") {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("the channel takes it in");
        model.gate.add_permits(1);
        let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
        assert_eq!(said(answered(&mut events, agent).await), ["first"]);

        alice.send_message("and now?".to_string(), None).await.unwrap();
        model.gate.add_permits(1);
        assert_eq!(said(answered(&mut events, agent).await), ["second"]);
        let seen = model.seen.lock().unwrap().clone();
        let heard = |call: &[String], text: &str| call.iter().any(|c| c.contains(text));
        assert!(heard(&seen[0], "hello"), "the turn under way saw what was said then");
        assert!(heard(&seen[1], "good evening") && !heard(&seen[1], "hello"), "the next is given what stands");
        until("the agent's history holds the modification", Box::new(move |state| state.history.message(hello).is_some_and(|m| m.modified.is_some()))).await;

        let question = channel.history().await.iter().find(|m| m.content.as_deref() == Some("and now?")).expect("said").id;
        alice.revoke_message(question).await.expect("hers to take back");
        until("the agent takes it back", Box::new(move |state| {
            state.history.message(question).is_some_and(|m| m.revoked.is_some())
                && !state.transcript.messages().iter().any(|m| m.id == question)
        })).await;
        assert!(channel.message(question).await.is_some_and(|m| m.revoked.is_some()), "so does the channel");

        // Another's message released (issue 239), as the policy lets alice
        // here: it reaches the agent that said it, which is not its sender.
        let first = channel.history().await.iter().find(|m| m.content.as_deref() == Some("first")).expect("answered").id;
        alice.reclassify_message(first, Classification::PUBLIC).await.expect("hers to release");
        until("the agent takes the release in", Box::new(move |state| {
            state.history.message(first).is_some_and(|m| m.classification == Classification::PUBLIC)
                && state.transcript.messages().iter().any(|m| m.id == first && m.classification == Classification::PUBLIC)
        })).await;
        running.abort();
    }

    /// What the channel shows next of `wanted`'s kind from `agent`.
    async fn shown_by(events: &mut tokio::sync::broadcast::Receiver<Envelope>, agent: caelumora_model::agent::AgentId, wanted: &str) -> ActorEvent {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == ActorId::Agent(agent) && envelope.event.kind() == wanted {
                    break envelope.event;
                }
            }
        }).await.unwrap_or_else(|_| panic!("the agent sends {wanted}"))
    }

    #[tokio::test]
    async fn an_agent_asked_to_answer_again_takes_its_answer_back_and_answers_without_it() {
        let model = Arc::new(Gated {
            answers: std::sync::Mutex::new(["first", "second"].into()),
            gate: Arc::new(tokio::sync::Semaphore::new(1)),
            seen: Default::default(),
        });
        let Stuck { channel, context, agent, mut events, running, .. } = with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
        let first = answered(&mut events, agent).await.pop().expect("an answer");
        let alice = channel.user_actor(alice_of(&context)).unwrap();

        alice.request_retry(first.id).await.expect("asked");
        assert!(matches!(shown_by(&mut events, agent, "RetryAccepted").await, ActorEvent::RetryAccepted { target } if target == RetryTarget::Message(first.id)));
        assert!(matches!(shown_by(&mut events, agent, "MessageRevoked").await, ActorEvent::MessageRevoked { message, .. } if message == first.id));
        model.gate.add_permits(1);
        let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
        assert_eq!(said(answered(&mut events, agent).await), ["second"], "answered again");

        let seen = model.seen.lock().unwrap().clone();
        let given = |call: &[String], text: &str| call.iter().any(|c| c.contains(text));
        assert!(given(&seen[1], "hello") && !given(&seen[1], "first"), "given what was said before it, not the answer replaced: {:?}", seen[1]);
        assert!(channel.message(first.id).await.is_some_and(|m| m.revoked.is_some()), "the channel shows it taken back");
        let history: Vec<_> = channel.history().await.into_iter().filter_map(|m| m.content).collect();
        assert_eq!(history, ["hello", "second"], "in its place, the new answer at the end");
        running.abort();
    }

    #[tokio::test]
    async fn an_agent_does_not_answer_again_once_more_was_said() {
        let model = Arc::new(Gated {
            answers: std::sync::Mutex::new(["first", "second", "third"].into()),
            gate: Arc::new(tokio::sync::Semaphore::new(2)),
            seen: Default::default(),
        });
        let Stuck { channel, context, agent, mut events, running, .. } = with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
        let first = answered(&mut events, agent).await.pop().expect("an answer");
        let alice = channel.user_actor(alice_of(&context)).unwrap();
        alice.send_message("and now?".to_string(), None).await.unwrap();
        answered(&mut events, agent).await;

        alice.request_retry(first.id).await.expect("asked");
        let ActorEvent::RetryRejected { target, reason } = shown_by(&mut events, agent, "RetryRejected").await else { unreachable!() };
        assert_eq!(target, RetryTarget::Message(first.id));
        assert_eq!(reason, caelumora_model::agent::RetryRefused::NotLast);
        assert!(channel.message(first.id).await.is_some_and(|m| m.revoked.is_none()), "nothing taken back");
        assert_eq!(model.seen.lock().unwrap().len(), 2, "and no model asked again");
        running.abort();
    }

    /// A gated model, its agent in a channel, and the agent's first turn
    /// asking the model about "hello", its answers held back.
    async fn asking_a_gated_model(answers: &[&'static str]) -> (Stuck, Arc<Gated>) {
        let model = Arc::new(Gated {
            answers: std::sync::Mutex::new(answers.iter().copied().collect()),
            gate: Arc::new(tokio::sync::Semaphore::new(0)),
            seen: Default::default(),
        });
        let stuck = with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
        asked(&model, 1).await;
        (stuck, model)
    }

    /// Until `model` was asked `calls` times.
    async fn asked(model: &Gated, calls: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while model.seen.lock().unwrap().len() < calls {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap_or_else(|_| panic!("the model is asked {calls} times"));
    }

    #[tokio::test]
    async fn an_answer_under_way_asked_for_again_is_given_up_and_answered_anew() {
        let (Stuck { channel, context, agent, mut events, running, .. }, model) = asking_a_gated_model(&["first", "second"]).await;
        let ActorEvent::ResponseStarted { turn_id: turn } = shown_by(&mut events, agent, "ResponseStarted").await else { unreachable!() };
        assert_eq!(channel.turn_under_way(ActorId::Agent(agent)), Some(turn), "the turn its start announced");
        let alice = channel.user_actor(alice_of(&context)).unwrap();
        let error = |result: anyhow::Result<uuid::Uuid>| result.expect_err("refused").downcast::<ChannelError>().expect("a channel's error");
        assert!(matches!(error(alice.request_retry_of_turn(ActorId::Agent(agent), TurnId::new()).await), ChannelError::NotUnderWay { .. }), "not another turn");

        alice.request_retry_of_turn(ActorId::Agent(agent), turn).await.expect("asked");
        assert!(matches!(shown_by(&mut events, agent, "RetryAccepted").await, ActorEvent::RetryAccepted { target } if target == RetryTarget::Turn(turn)));
        let ActorEvent::ResponseStarted { turn_id: next } = shown_by(&mut events, agent, "ResponseStarted").await else { unreachable!() };
        assert_ne!(next, turn, "a turn of its own answers");
        model.gate.add_permits(1);
        let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
        assert_eq!(said(answered(&mut events, agent).await), ["second"], "the answer under way never came");

        let seen = model.seen.lock().unwrap().clone();
        assert!(seen[1].iter().any(|c| c.contains("hello")), "asked about what the given-up turn was asked: {:?}", seen[1]);
        let history: Vec<_> = channel.history().await.into_iter().filter_map(|m| m.content).collect();
        assert_eq!(history, ["hello", "second"], "nothing of the given-up turn reached the channel");
        assert!(matches!(error(alice.request_retry_of_turn(ActorId::Agent(agent), turn).await), ChannelError::NotUnderWay { .. }), "and it is over");
        running.abort();
    }

    #[tokio::test]
    async fn what_was_said_during_a_turn_asked_for_again_is_answered_in_the_one_that_follows() {
        let (Stuck { channel, context, agent, mut events, running, .. }, model) = asking_a_gated_model(&["first", "second", "third"]).await;
        let ActorEvent::ResponseStarted { turn_id: turn } = shown_by(&mut events, agent, "ResponseStarted").await else { unreachable!() };
        let alice_id = alice_of(&context);
        let alice = channel.user_actor(alice_id).unwrap();
        alice.send_message("and this too".to_string(), None).await.expect("taken during a turn");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == ActorId::User(alice_id) && matches!(envelope.event, ActorEvent::FullResponseGenerated(_)) {
                    break;
                }
            }
        }).await.expect("the message is said in the channel");

        alice.request_retry_of_turn(ActorId::Agent(agent), turn).await.expect("asked");
        assert!(matches!(shown_by(&mut events, agent, "RetryAccepted").await, ActorEvent::RetryAccepted { target } if target == RetryTarget::Turn(turn)));
        model.gate.add_permits(3);
        let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
        assert_eq!(said(answered(&mut events, agent).await), ["second"]);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let seen = model.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "one turn answers both, not one each: {seen:?}");
        assert!(seen[1].iter().any(|c| c.contains("hello")) && seen[1].iter().any(|c| c.contains("and this too")), "{:?}", seen[1]);
        running.abort();
    }

    #[tokio::test]
    async fn an_older_answer_asked_for_again_leaves_the_answer_under_way_alone() {
        let (Stuck { channel, context, agent, helper, mut events, running }, model) = asking_a_gated_model(&["first", "second", "third"]).await;
        model.gate.add_permits(1);
        let first = answered(&mut events, agent).await.pop().expect("an answer");
        let alice = channel.user_actor(alice_of(&context)).unwrap();
        alice.send_message("and now?".to_string(), None).await.unwrap();
        asked(&model, 2).await;

        // Asked while the next answer is under way: it waits for it, and
        // so does what is said meanwhile.
        alice.request_retry(first.id).await.expect("asked");
        alice.send_message("one more".to_string(), None).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(helper.state(), ActorState::Planning, "the answer under way goes on");
        model.gate.add_permits(2);
        let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
        assert_eq!(said(answered(&mut events, agent).await), ["second"], "it ends as it began");

        // Judged against what stands, beside the request to answer "one
        // more": not its last answer any more.
        let ActorEvent::RetryRejected { target, reason } = shown_by(&mut events, agent, "RetryRejected").await else { unreachable!() };
        assert_eq!((target, reason), (RetryTarget::Message(first.id), caelumora_model::agent::RetryRefused::NotLast));
        assert_eq!(said(answered(&mut events, agent).await), ["third"], "and the request is answered");
        assert!(channel.message(first.id).await.is_some_and(|m| m.revoked.is_none()), "nothing taken back");
        running.abort();
    }

    #[tokio::test]
    async fn a_stopped_channel_refuses_what_a_user_sends() {
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(DummySecurityManager::new())).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let mut said = channel.observe();
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });
        actor.send_message("hello".to_string(), None).await.expect("a running channel takes it");
        // Taken out of the inbox: `run` holds it now.
        tokio::time::timeout(Duration::from_secs(5), said.recv()).await.expect("the message went through").unwrap();

        // Aborted, as a stopped session aborts its channels: the inbox closes
        // with it, so a sender learns so instead of waiting for ever.
        running.abort();
        let _ = running.await;
        let refused = tokio::time::timeout(Duration::from_secs(5), actor.send_message("anyone?".to_string(), None)).await
            .expect("an answer, not a wait");
        assert!(refused.is_err(), "a stopped channel takes nothing");
    }

    /// A model whose answer never comes: what a cancel has to interrupt.
    #[derive(Debug)]
    struct NeverAnswers;

    #[async_trait::async_trait]
    impl crate::llm::LLMClient for NeverAnswers {
        async fn chat_completion(&self, _: &[Message], _: &[caelumora_model::tool::Tool])
            -> futures::stream::BoxStream<'static, crate::llm::LLMResult<caelumora_model::message::MessageEvent>>
        {
            Box::pin(futures::stream::pending())
        }

        fn profile(&self) -> caelumora_model::context::ModelProfile {
            Default::default()
        }
    }

    /// A running channel with alice and an agent whose model never answers,
    /// the agent already in a turn: what a cancel or a stop has to end.
    struct Stuck {
        channel: Arc<ChannelLoop>,
        context: SecurityContext,
        agent: caelumora_model::agent::AgentId,
        helper: Arc<dyn Actor>,
        events: broadcast::Receiver<Envelope>,
        running: tokio::task::JoinHandle<Result<()>>,
    }

    async fn stuck_in_a_model_call() -> Stuck {
        let stuck = with_an_agent(Arc::new(NeverAnswers), Default::default()).await;
        settled(&stuck.helper, ActorState::Planning).await;
        stuck
    }

    /// A running channel with alice and an agent that talks to `model` and
    /// has `components`, after alice said "hello".
    async fn with_an_agent(model: Arc<dyn crate::llm::LLMClient>, components: caelumora_model::component::ComponentConfig) -> Stuck {
        with_an_agent_under(Arc::new(DummySecurityManager::new()), model, components).await
    }

    /// `with_an_agent`, under `security`.
    async fn with_an_agent_under(security: Arc<dyn SecurityManager>, model: Arc<dyn crate::llm::LLMClient>, components: caelumora_model::component::ComponentConfig) -> Stuck {
        start(agent_channel(security, model, components).await).await
    }

    /// A channel of alice and an agent that talks to `model` and has
    /// `components`, under `security`, built but not running.
    async fn agent_channel(security: Arc<dyn SecurityManager>, model: Arc<dyn crate::llm::LLMClient>, components: caelumora_model::component::ComponentConfig) -> (Arc<ChannelLoop>, SecurityContext, caelumora_model::agent::AgentId, UserId) {
        use caelumora_model::agent::{Agent, Personality, PersonalityId};
        use caelumora_model::scope::Scope;
        use caelumora_model::store::EntityStore;

        let store = Arc::new(MemoryStore::new());
        let personality = Personality {
            description: None,
            introduction: None,
            reasoning_visibility: caelumora_model::confidentiality::Visibility::Channel,
            id: PersonalityId::new(),
            name: ScopedName::new("plain", Default::default()).unwrap(),
            kind: Default::default(),
            roles: None,
            components,
            security_attributes: Default::default(),
            deleted: None,
        };
        store.create_personality(&SecurityContext::system(), &personality).await.unwrap();
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let agent = Agent::new(ScopedName::new("helper", Scope::GlobalScope).unwrap(), &personality, context.initiator_id);
        let agent_id = agent.id;
        store.create_user(&SecurityContext::system(), &alice).await.unwrap();
        store.create_agent(&SecurityContext::system(), &agent).await.unwrap();

        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&security)));
        // The model is a component (issue 130): the stand-in is registered as
        // a kind of its own and made every agent's default.
        let mut registry = crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store));
        registry.register(
            crate::component::ComponentKind::new("test_model").provides::<dyn crate::llm::LLMClient>(),
            move |_descriptor, _context| {
                let mut facets = crate::component::ComponentFacets::empty();
                facets.insert::<dyn crate::llm::LLMClient>(Arc::clone(&model));
                Ok(facets)
            },
        );
        register_no_secrets(&mut registry);
        register_asking_tool(&mut registry);
        let defaults = caelumora_model::component::ComponentConfig {
            component_nodes: vec![caelumora_model::component::ComponentDescriptor {
                id: "model".to_string(),
                kind: "test_model".to_string(),
                params: serde_json::Value::Null,
                tags: Vec::new(),
                fail_policy: Default::default(),
            }],
        };
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), store, secrets, security, default_commands(), defaults, Vec::new(), registry).unwrap());
        let channel = Channel {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            id: ChannelId::new(),
            session_id: SessionId::new(),
            name: Name::new("main").unwrap(),
            components: caelumora_model::channel::moderation_components(None, Some("broadcast_policy")),
            participants: vec![
                Participant::User(alice.id),
                Participant::Agent(agent_id),
                Participant::Approver(caelumora_model::approval::Approver { id: caelumora_model::approval::ApproverId::new(), strategy: caelumora_model::approval::ApprovalStrategy::Any }),
            ],
            security_attributes: SecurityAttributes::default(),
            classification: caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT,
            language: None,
            history_limits: Vec::new(),
            deleted: None,
            former: Vec::new(),
        };
        // Stored, so that leaving it can be stored too.
        runtime.entity_store().create_channel(&SecurityContext::system(), &channel).await.unwrap();
        let executor = CommandExecutor::empty(Arc::clone(runtime.security_manager()));
        let session_id = channel.session_id;
        let channel = ChannelLoop::new(channel, ChannelRuntime::outside_a_session(runtime, Arc::new(executor), session_id)).await.unwrap();
        (channel, context, agent_id, alice.id)
    }

    /// A model that gives the answers it was given, one per call, and then
    /// none.
    #[derive(Debug)]
    struct Scripted(std::sync::Mutex<std::collections::VecDeque<Message>>);

    #[async_trait::async_trait]
    impl crate::llm::LLMClient for Scripted {
        async fn chat_completion(&self, _: &[Message], _: &[caelumora_model::tool::Tool])
            -> futures::stream::BoxStream<'static, crate::llm::LLMResult<caelumora_model::message::MessageEvent>>
        {
            match self.0.lock().unwrap().pop_front() {
                Some(answer) => Box::pin(futures::stream::iter([Ok(caelumora_model::message::MessageEvent::Complete(answer))])),
                None => Box::pin(futures::stream::pending()),
            }
        }

        fn profile(&self) -> caelumora_model::context::ModelProfile {
            Default::default()
        }
    }

    /// A model that answers each call with the next of its answers only once
    /// `gate` lets it, and remembers the contents each call was given.
    #[derive(Debug)]
    struct Gated {
        answers: std::sync::Mutex<std::collections::VecDeque<&'static str>>,
        gate: Arc<tokio::sync::Semaphore>,
        seen: std::sync::Mutex<Vec<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl crate::llm::LLMClient for Gated {
        async fn chat_completion(&self, messages: &[Message], _: &[caelumora_model::tool::Tool])
            -> futures::stream::BoxStream<'static, crate::llm::LLMResult<caelumora_model::message::MessageEvent>>
        {
            self.seen.lock().unwrap().push(messages.iter().filter_map(|m| m.content.clone()).collect());
            let Some(answer) = self.answers.lock().unwrap().pop_front() else { return Box::pin(futures::stream::pending()) };
            let gate = Arc::clone(&self.gate);
            Box::pin(futures::stream::once(async move {
                gate.acquire().await.unwrap().forget();
                // An id of its own, as every model client gives its answers.
                Ok(caelumora_model::message::MessageEvent::Complete(Message {
                    id: MessageId::new(),
                    role: caelumora_model::message::MessageRole::Assistant,
                    content: Some(answer.to_string()),
                    ..Message::new(Classification::CHANNEL_DEFAULT)
                }))
            }))
        }

        fn profile(&self) -> caelumora_model::context::ModelProfile {
            Default::default()
        }
    }

    #[tokio::test]
    async fn what_a_user_says_during_a_turn_is_answered_after_it() {
        let model = Arc::new(Gated {
            answers: std::sync::Mutex::new(["first", "second"].into()),
            gate: Arc::new(tokio::sync::Semaphore::new(0)),
            seen: Default::default(),
        });
        let Stuck { channel, context, agent, mut events, running, .. } = with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while model.seen.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("the agent asks its model about hello");

        // Taken while the turn waits for its model: nothing holds the user back.
        let alice = alice_of(&context);
        channel.user_actor(alice).unwrap().send_message("and this too".to_string(), None).await.expect("taken during a turn");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == ActorId::User(alice) && matches!(envelope.event, ActorEvent::FullResponseGenerated(_)) {
                    break;
                }
            }
        }).await.expect("the message is said in the channel");
        model.gate.add_permits(2);

        let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
        assert_eq!(said(answered(&mut events, agent).await), ["first"], "the running turn ends as it began");
        assert_eq!(said(answered(&mut events, agent).await), ["second"], "and a turn of its own answers the message");
        let seen = model.seen.lock().unwrap().clone();
        let heard = |call: &[String], text: &str| call.iter().any(|c| c.contains(text));
        assert!(heard(&seen[0], "hello") && !heard(&seen[0], "and this too"), "the running turn does not see it");
        assert!(heard(&seen[1], "and this too"), "the next one does");
        running.abort();
    }

    #[tokio::test]
    async fn an_agent_asked_several_times_during_a_turn_answers_once_after_it() {
        let model = Arc::new(Gated {
            answers: std::sync::Mutex::new(["first", "second", "third"].into()),
            gate: Arc::new(tokio::sync::Semaphore::new(0)),
            seen: Default::default(),
        });
        let Stuck { channel, context, agent, mut events, running, .. } = with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while model.seen.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("the agent asks its model about hello");

        // Three messages, each asking the agent, while its turn waits.
        let alice = alice_of(&context);
        let queued = ["one", "two", "three"];
        for text in queued {
            channel.user_actor(alice).unwrap().send_message(text.to_string(), None).await.expect("taken during a turn");
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut said = 0;
            while said < queued.len() {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == ActorId::User(alice) && matches!(envelope.event, ActorEvent::FullResponseGenerated(_)) {
                    said += 1;
                }
            }
        }).await.expect("the messages are said in the channel");
        model.gate.add_permits(3);

        let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
        assert_eq!(said(answered(&mut events, agent).await), ["first"], "the running turn ends as it began");
        assert_eq!(said(answered(&mut events, agent).await), ["second"], "one turn answers what was queued");
        let seen = model.seen.lock().unwrap().clone();
        assert!(queued.iter().all(|text| seen[1].iter().any(|c| c.contains(text))), "having heard all of it: {:?}", seen[1]);

        // Nothing is left to answer: told something new, the agent answers
        // it with the next call of its model, not with a stale one.
        channel.user_actor(alice).unwrap().send_message("four".to_string(), None).await.expect("sent");
        assert_eq!(said(answered(&mut events, agent).await), ["third"]);
        let seen = model.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 3, "one call per turn, one turn per batch");
        assert!(seen[2].iter().any(|c| c.contains("four")), "the third call answers the new message");
        running.abort();
    }

    #[tokio::test]
    async fn a_sleeping_agent_woken_by_a_mention_answers_it() {
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        let answer = |text: &str, tool_calls| Message {
            role: MessageRole::Assistant,
            content: Some(text.to_string()),
            tool_calls,
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let stop = vec![ToolCall { id: Some("1".to_string()), function_name: "stop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() }];
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "stop".to_string(),
            kind: crate::tool_impl::stop_tool::STOP_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }] };
        let model = Scripted(std::sync::Mutex::new([answer("Good bye.", stop), answer("I am back.", Vec::new())].into()));
        let Stuck { channel, context, agent, helper, mut events, running } = with_an_agent(Arc::new(model), components).await;
        settled(&helper, ActorState::Sleeping).await;
        let alice = alice_of(&context);

        // The message and the request reach it together, and it was woken
        // before either: it answers (issue 222).
        channel.user_actor(alice).unwrap().send_message("@helper, are you there?".to_string(), None).await.expect("sent");
        let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
        loop {
            if said(answered(&mut events, agent).await) == ["I am back."] {
                break;
            }
        }
        running.abort();
    }

    #[tokio::test]
    async fn an_agent_that_calls_stop_ends_its_turn_with_the_call_and_sleeps() {
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        // One answer only: a second call of the model would never return.
        let bye = Message {
            role: MessageRole::Assistant,
            content: Some("Good bye.".to_string()),
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "stop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "stop".to_string(),
            kind: crate::tool_impl::stop_tool::STOP_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }] };
        let Stuck { channel, agent, helper, running, .. } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([bye].into()))), components).await;

        // The turn ends with the call - the model is not asked again - and
        // the agent sleeps, as the tool asked; no turn is left behind.
        settled(&helper, ActorState::Sleeping).await;
        let stored = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert!(stored.turn.is_none(), "the turn was taken over");
        assert!(stored.history.messages().iter().any(|m| m.content.as_deref() == Some("Good bye.")), "and its answer recorded");
        running.abort();
    }

    /// An agent tells the users where it stands as it starts and moves
    /// (issue 243): through its turn into its sleep - and woken by `/wakeup`,
    /// which sends it no message, it tells them so too.
    #[tokio::test]
    async fn an_agent_tells_the_users_where_it_stands_and_that_it_was_woken() {
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        let bye = Message {
            role: MessageRole::Assistant,
            content: Some("Good bye.".to_string()),
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "stop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "stop".to_string(),
            kind: crate::tool_impl::stop_tool::STOP_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }] };
        let (channel, context, agent, alice) = agent_channel(Arc::new(DummySecurityManager::new()), Arc::new(Scripted(std::sync::Mutex::new([bye].into()))), components).await;
        // What it sends, watched since before it ran.
        let mut events = channel.observe();
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });
        assert_eq!(told_states(&mut events, agent, ActorState::Idle).await, [ActorState::Idle], "where it starts, before it is asked");
        // What reaches alice, from before she asks.
        let mut shown = channel.user_actor(alice).unwrap().watch().events;
        channel.user_actor(alice).unwrap().send_message("hello".to_string(), None).await.unwrap();
        let helper = channel.actor_by_id(ActorId::Agent(agent)).unwrap();
        assert_eq!(told_states(&mut shown, agent, ActorState::Sleeping).await, [ActorState::Planning, ActorState::ExecutingTools, ActorState::Reflecting, ActorState::Sleeping], "each step it moves to, once");

        let woken = channel.wakeup(&context).await.expect("alice may wake it");
        assert_eq!(woken, ["helper"]);
        assert_eq!(told_states(&mut shown, agent, ActorState::Idle).await, [ActorState::Idle], "woken without a message, it tells so itself");
        assert_eq!(helper.state(), ActorState::Idle);
        running.abort();
    }

    /// The states `agent` told the users of through `shown`, up to `until`.
    async fn told_states(shown: &mut broadcast::Receiver<Envelope>, agent: caelumora_model::agent::AgentId, until: ActorState) -> Vec<ActorState> {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut states = Vec::new();
            loop {
                let envelope = shown.recv().await.expect("the channel is running");
                if let (ActorEvent::StateChanged(state), true) = (envelope.event, envelope.context.sender_id == ActorId::Agent(agent)) {
                    states.push(state);
                    if state == until { break states }
                }
            }
        }).await.unwrap_or_else(|_| panic!("the agent tells of {until:?}"))
    }

    #[tokio::test]
    async fn a_plan_runs_its_steps_as_intermediate_messages_and_ends_with_the_final_answer() {
        use caelumora_model::approval::ApprovalResponse;
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        let said = |content: &str| Message { role: MessageRole::Assistant, content: Some(content.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let plan = Message {
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall {
                id: Some("1".to_string()),
                function_name: "create_plan".to_string(),
                arguments: serde_json::json!({ "summary": "two steps", "steps": [{ "description": "first" }, { "description": "second" }] }),
                visibility: ToolVisibility::SHOWN,
                presentation: Default::default(),
            }],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let script = [plan, said("first done"), said("second done"), said("all done")];
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "plan".to_string(),
            kind: crate::component_impl::create_plan_tool::CREATE_PLAN_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }] };
        let Stuck { channel, context, agent, helper, mut events, running } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new(script.into()))), components).await;

        // The plan needs approval; alice gives it.
        let mut seen = Vec::new();
        let request = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let ActorEvent::ApprovalRequested(request) = events.recv().await.expect("the channel is running").event {
                    break request;
                }
            }
        }).await.expect("the plan is put up for approval");
        let ActorId::User(alice) = context.initiator_id else { panic!("alice acts") };
        channel.user_actor(alice).unwrap().send_approval(ApprovalResponse::new(request.id, agent), true).await.unwrap();

        let final_messages = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id != ActorId::Agent(agent) { continue }
                match envelope.event {
                    ActorEvent::FullResponseGenerated(messages) => break messages,
                    ActorEvent::IntermediateMessageGenerated(m) => seen.push(format!("intermediate:{}", m.content.unwrap_or_default())),
                    // Not the plan's call nor the tool's result: they say nothing.
                    ActorEvent::CompleteMessageGenerated(m) => seen.extend(m.content.map(|c| format!("complete:{c}"))),
                    ActorEvent::PlanStepStarted { step_index, .. } => seen.push(format!("started:{step_index}")),
                    ActorEvent::PlanStepFinished { step_index, .. } => seen.push(format!("finished:{step_index}")),
                    _ => {}
                }
            }
        }).await.expect("the turn ends");
        assert_eq!(seen, [
            "started:0", "intermediate:first done", "finished:0",
            "started:1", "intermediate:second done", "finished:1",
            "complete:all done",
        ]);
        let contents: Vec<_> = final_messages.iter().filter_map(|m| m.content.as_deref()).collect();
        assert_eq!(contents, ["all done"], "the steps are not part of the answer");
        settled(&helper, ActorState::Idle).await;
        running.abort();
    }

    #[tokio::test]
    async fn every_message_of_a_turn_carries_the_turn_its_start_announced() {
        use caelumora_model::approval::ApprovalResponse;
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        // A plan: a call, its result, the steps' internal answers and the
        // final one - every kind of message a turn produces.
        let said = |content: &str| Message { role: MessageRole::Assistant, content: Some(content.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let plan = Message {
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall {
                id: Some("1".to_string()),
                function_name: "create_plan".to_string(),
                arguments: serde_json::json!({ "summary": "one step", "steps": [{ "description": "first" }] }),
                visibility: ToolVisibility::SHOWN,
                presentation: Default::default(),
            }],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let script = [plan, said("first done"), said("all done")];
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "plan".to_string(),
            kind: crate::component_impl::create_plan_tool::CREATE_PLAN_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }] };
        let Stuck { channel, context, agent, helper, mut events, running } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new(script.into()))), components).await;

        let mut started = None;
        let mut shown = Vec::new();
        let request = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                let agents = envelope.context.sender_id == ActorId::Agent(agent);
                match envelope.event {
                    ActorEvent::ResponseStarted { turn_id } if agents => started = Some(turn_id),
                    ActorEvent::CompleteMessageGenerated(m) | ActorEvent::IntermediateMessageGenerated(m) if agents => shown.push(m.turn_id),
                    ActorEvent::ApprovalRequested(request) => break request,
                    _ => {}
                }
            }
        }).await.expect("the plan is put up for approval");
        let ActorId::User(alice) = context.initiator_id else { panic!("alice acts") };
        channel.user_actor(alice).unwrap().send_approval(ApprovalResponse::new(request.id, agent), true).await.unwrap();

        let final_messages = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id != ActorId::Agent(agent) { continue }
                match envelope.event {
                    ActorEvent::FullResponseGenerated(messages) => break messages,
                    ActorEvent::CompleteMessageGenerated(m) | ActorEvent::IntermediateMessageGenerated(m) => shown.push(m.turn_id),
                    _ => {}
                }
            }
        }).await.expect("the turn ends");
        settled(&helper, ActorState::Idle).await;

        let started = started.expect("the turn announced itself");
        assert_eq!(shown.len(), 4, "the call, its result, the step's answer and the final answer: {shown:?}");
        assert!(shown.iter().all(|turn| *turn == Some(started)), "each shown as the turn's: {shown:?}");
        assert!(final_messages.iter().all(|m| m.turn_id == Some(started)), "and published so");
        let answer = final_messages.iter().find(|m| m.content.as_deref() == Some("all done")).expect("the final answer");
        assert_eq!(channel.message(answer.id).await.and_then(|m| m.turn_id), Some(started), "the channel's history keeps it");

        let stored = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        let (own, heard): (Vec<_>, Vec<_>) = stored.transcript.messages().into_iter().partition(|m| m.sender_id == ActorId::Agent(agent));
        assert!(own.len() >= 4 && own.iter().all(|m| m.turn_id == Some(started)), "its transcript keeps every message of the turn as the turn's");
        assert!(heard.iter().all(|m| m.turn_id.is_none()), "what it heard is no turn's of its own");
        running.abort();
    }

    #[tokio::test]
    async fn a_turn_is_stored_with_the_step_that_comes_next() {
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        // The model calls a tool that needs no approval, and then never
        // answers again: the agent waits in `Planning` with the result.
        let calculate = Message {
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "calculate".to_string(), arguments: serde_json::json!({ "expression": "1+1" }), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "calculator".to_string(),
            kind: crate::tool_impl::calculator_tool::CALCULATOR_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }] };
        let Stuck { channel, agent, running, .. } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([calculate].into()))), components).await;

        // What a crash now would leave: `Planning`, with the tool's result
        // in the turn - so that a resume asks the model about the result
        // instead of asking again for the call or running the tool twice.
        let stored = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
                let has_result = state.turn.as_ref().is_some_and(|t| t.messages().any(|m| !m.tool_results.is_empty()));
                if has_result && state.processing_state == ActorState::Planning { break state }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("stored in Planning, with the tool's result");
        assert!(stored.turn.is_some_and(|t| t.tool_calls.is_empty()), "the calls of the step are done");
        running.abort();
    }

    /// A model that gives the answers it was given, one per call, and then
    /// fails.
    #[derive(Debug)]
    struct FailsAfter(std::sync::Mutex<std::collections::VecDeque<Message>>);

    #[async_trait::async_trait]
    impl crate::llm::LLMClient for FailsAfter {
        async fn chat_completion(&self, _: &[Message], _: &[caelumora_model::tool::Tool])
            -> futures::stream::BoxStream<'static, crate::llm::LLMResult<caelumora_model::message::MessageEvent>>
        {
            match self.0.lock().unwrap().pop_front() {
                Some(answer) => Box::pin(futures::stream::iter([Ok(caelumora_model::message::MessageEvent::Complete(answer))])),
                None => Box::pin(futures::stream::iter([Err(crate::llm::LLMError::Backend {
                    endpoint: "test".to_string(), model: "test".to_string(), message: "gone".to_string(),
                })])),
            }
        }

        fn profile(&self) -> caelumora_model::context::ModelProfile {
            Default::default()
        }
    }

    #[tokio::test]
    async fn a_turn_that_fails_after_a_tool_ran_stays_as_a_branch_and_the_agent_is_told() {
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::transcript::GivenUpReason;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        // The model calls a tool that needs no approval, and fails when
        // asked about its result.
        let calculate = Message {
            id: MessageId::new(),
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "calculate".to_string(), arguments: serde_json::json!({ "expression": "1+1" }), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "calculator".to_string(),
            kind: crate::tool_impl::calculator_tool::CALCULATOR_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }] };
        let Stuck { channel, context: _, agent, helper, mut events, running } =
            with_an_agent(Arc::new(FailsAfter(std::sync::Mutex::new([calculate.clone()].into()))), components).await;

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == ActorId::Agent(agent) && matches!(envelope.event, ActorEvent::TurnFailed { .. }) { break }
            }
        }).await.expect("the turn fails");
        settled(&helper, ActorState::Idle).await;
        let stored = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();

        // The turn's messages are a branch no longer active, marked as given
        // up; what the model saw for them can still be told.
        let seen = crate::view::AgentContextView::of(ActorId::Agent(agent), stored.clone());
        let turns = seen.given_up_turns();
        assert_eq!(turns.len(), 1);
        assert!(matches!(turns[0].given_up.reason, GivenUpReason::Failed { .. }));
        let roles: Vec<_> = turns[0].messages.iter().map(|m| m.role).collect();
        assert_eq!(roles, [MessageRole::Assistant, MessageRole::Tool], "the call and its result");
        assert!(seen.context_of(calculate.id).is_ok(), "the call's context is resolved on its branch");

        // On the active branch the agent is told what ran, and nothing else
        // of the turn is there - neither in its transcript nor its history.
        let transcript = stored.transcript.messages();
        assert!(transcript.iter().all(|m| m.id != calculate.id), "{transcript:?}");
        let told = transcript.last().expect("the agent is told");
        assert!((told.role, told.internal) == (MessageRole::System, true), "{told:?}");
        let text = told.content.as_deref().unwrap_or_default();
        assert!(text.contains("calculate") && text.contains("1+1"), "in full: {text}");
        assert!(stored.history.messages().iter().all(|m| m.id != calculate.id && m.id != told.id));
        running.abort();
    }

    #[tokio::test]
    async fn an_agent_removed_during_a_turn_gives_it_up() {
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::transcript::GivenUpReason;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        // The model calls a tool, and never answers again.
        let calculate = Message {
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "calculate".to_string(), arguments: serde_json::json!({ "expression": "1+1" }), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "calculator".to_string(),
            kind: crate::tool_impl::calculator_tool::CALCULATOR_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }] };
        let Stuck { channel, context, agent, running, .. } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([calculate].into()))), components).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
                if state.turn.as_ref().is_some_and(|t| t.messages().any(|m| !m.tool_results.is_empty())) { break }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("the tool ran");

        channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");

        let turns = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let turns = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap().transcript.given_up_turns();
                if !turns.is_empty() { break turns }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("the turn is given up");
        assert_eq!(turns[0].given_up.reason, GivenUpReason::Stopped);
        running.abort();
    }

    /// A policy that permits everything, but puts into a channel only what
    /// flows into it (`ActionType::Publish`) - what lowering a channel is
    /// judged by.
    #[derive(Debug)]
    struct FlowRule;

    #[async_trait::async_trait]
    impl SecurityManager for FlowRule {
        async fn check(&self, _context: &SecurityContext, action: ActionType<'_>) -> bool {
            match action {
                ActionType::Publish { classification, channel, .. } => classification.flows_to(channel, &Default::default()),
                _ => true,
            }
        }
    }

    fn calculator() -> (Message, caelumora_model::component::ComponentConfig) {
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};
        let calculate = Message {
            id: MessageId::new(),
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "calculate".to_string(), arguments: serde_json::json!({ "expression": "1+1" }), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "calculator".to_string(),
            kind: crate::tool_impl::calculator_tool::CALCULATOR_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }] };
        (calculate, components)
    }

    /// The `TurnFailed` the agent `agent` sends next.
    async fn turn_failed(events: &mut tokio::sync::broadcast::Receiver<Envelope>, agent: caelumora_model::agent::AgentId) -> String {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == ActorId::Agent(agent) {
                    if let ActorEvent::TurnFailed { message } = envelope.event { break message }
                }
            }
        }).await.expect("the turn fails")
    }

    #[tokio::test]
    async fn lowering_the_channel_gives_up_a_running_turn() {
        use caelumora_model::confidentiality::Confidentiality;
        use caelumora_model::transcript::GivenUpReason;

        // The model calls a tool, and then is asked about its result for
        // ever.
        let (calculate, components) = calculator();
        let Stuck { channel, context, agent, helper, mut events, running } =
            with_an_agent_under(Arc::new(FlowRule), Arc::new(Scripted(std::sync::Mutex::new([calculate.clone()].into()))), components).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
                if state.turn.as_ref().is_some_and(|t| t.messages().any(|m| !m.tool_results.is_empty())) { break }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("the tool ran");

        let public = Classification::general(Confidentiality::Public);
        channel.reclassify(&context, public.clone()).await.expect("permitted");

        // The model call is interrupted, and the turn given up rather than
        // taken over: the users are told, nothing of it is in the history.
        let told = turn_failed(&mut events, agent).await;
        assert!(told.contains("lowered"), "{told}");
        settled(&helper, ActorState::Idle).await;
        let stored = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        let turns = stored.transcript.given_up_turns();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].given_up.reason, GivenUpReason::Lowered { channel: public.clone() });
        assert!(channel.history().await.iter().all(|m| m.id != calculate.id));

        // The agent learns which tool ran, but not what it saw.
        let told = stored.transcript.messages().pop().expect("the agent is told");
        let text = told.content.unwrap_or_default();
        assert!(text.contains("calculate") && !text.contains("1+1"), "{text}");
        assert_eq!(told.classification, public);
        running.abort();
    }

    #[tokio::test]
    async fn lowering_the_channel_ends_a_turn_waiting_for_approval_at_once() {
        use caelumora_model::confidentiality::Confidentiality;

        // Nobody approves the call.
        let (call, components) = a_call_needing_approval();
        let Stuck { channel, context, agent, helper, mut events, running } =
            with_an_agent_under(Arc::new(FlowRule), Arc::new(Scripted(std::sync::Mutex::new([call].into()))), components).await;
        settled(&helper, ActorState::WaitingForUser).await;

        channel.reclassify(&context, Classification::general(Confidentiality::Public)).await.expect("permitted");

        turn_failed(&mut events, agent).await;
        settled(&helper, ActorState::Idle).await;
        running.abort();
    }

    #[tokio::test]
    async fn raising_the_channel_lets_a_running_turn_go_on() {
        use caelumora_model::confidentiality::Confidentiality;
        let stuck = with_an_agent_under(Arc::new(FlowRule), Arc::new(NeverAnswers), Default::default()).await;
        settled(&stuck.helper, ActorState::Planning).await;

        stuck.channel.reclassify(&stuck.context, Classification::general(Confidentiality::Confidential)).await.expect("permitted");

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(stuck.helper.state(), ActorState::Planning, "the model call goes on");
        stuck.running.abort();
    }

    #[tokio::test]
    async fn a_cancel_ends_a_turn_waiting_for_approval_at_once() {

        let (call, components) = a_call_needing_approval();
        let Stuck { channel, context, agent, helper, mut events, running } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([call].into()))), components).await;
        settled(&helper, ActorState::WaitingForUser).await;

        channel.cancel(&context).await.expect("permitted");

        // Without anyone deciding on the call.
        settled(&helper, ActorState::Idle).await;

        // The request is withdrawn, and the approver tells the users
        // (issue 148).
        let (requested, withdrawn) = tokio::time::timeout(Duration::from_secs(5), async {
            let mut requested = None;
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                match envelope.event {
                    ActorEvent::ApprovalRequested(request) if matches!(envelope.context.sender_id, ActorId::Approver(_)) => requested = Some(request.id),
                    ActorEvent::ApprovalRequestWithdrawn { id } if matches!(envelope.context.sender_id, ActorId::Approver(_)) => break (requested, id),
                    _ => {}
                }
            }
        }).await.expect("the users learn that the request was withdrawn");
        assert_eq!(requested, Some(withdrawn));
        running.abort();

        // The call is answered as not executed (issue 148).
        let stored = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        let last = stored.transcript.messages().pop().expect("the turn was taken over");
        let answered: Vec<_> = last.tool_results.iter().map(|r| r.status).collect();
        assert_eq!(answered, [caelumora_model::tool::ToolResultStatus::NotExecuted]);
    }

    /// A call of `sleep` that needs approval, and the component that gives
    /// it, for an agent to wait for its approval.
    fn a_call_needing_approval() -> (Message, caelumora_model::component::ComponentConfig) {
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        let call = Message {
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "sleep".to_string(), arguments: serde_json::json!({ "seconds": 0 }), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "sleep".to_string(),
            kind: crate::tool_impl::sleep_tool::SLEEP_TOOL_KIND.to_string(),
            params: serde_json::json!({ "requires_approval": true }),
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }] };
        (call, components)
    }

    #[tokio::test]
    async fn a_rule_of_the_channel_asks_for_a_call_its_tool_would_run() {
        use caelumora_model::approval::{ApprovalRule, Reach, RuleDecision, ToolMatcher};

        let (call, mut components) = a_call_needing_approval();
        components.component_nodes[0].params = serde_json::json!({ "requires_approval": false });
        let built = agent_channel(Arc::new(DummySecurityManager::new()), Arc::new(Scripted(std::sync::Mutex::new([call].into()))), components).await;
        let ask = ApprovalRule::new(ToolMatcher { tool: "sleep".to_string(), arguments: Default::default() }, RuleDecision::Ask, Reach::Everywhere, None).unwrap();
        built.0.update_approval_rules(&SecurityContext::system(), vec![ask]).await.expect("the system may");

        let Stuck { helper, mut events, running, .. } = start(built).await;
        let request = approval_requested(&mut events).await;
        assert!(request.explicit, "for a decision of a person's own");
        settled(&helper, ActorState::WaitingForUser).await;
        running.abort();
    }

    /// The approval the approver asks the users for, from `events`.
    async fn approval_requested(events: &mut broadcast::Receiver<Envelope>) -> caelumora_model::approval::ApprovalRequest {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if let ActorEvent::ApprovalRequested(request) = envelope.event {
                    if matches!(envelope.context.sender_id, ActorId::Approver(_)) {
                        break request;
                    }
                }
            }
        }).await.expect("the users are asked")
    }

    #[tokio::test]
    async fn a_turn_waiting_for_approval_can_be_decided_after_a_restart() {
        use caelumora_model::message::MessageRole;

        let (call, components) = a_call_needing_approval();
        let done = Message { role: MessageRole::Assistant, content: Some("Done.".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let Stuck { channel, agent, helper, mut events, running, .. } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([call, done].into()))), components).await;
        settled(&helper, ActorState::WaitingForUser).await;
        let asked = approval_requested(&mut events).await;

        // The turn is the record of what it waits for (issue 172).
        let stored = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        let pending: Vec<_> = stored.turn.expect("a turn under way").pending_approvals().into_iter().map(|r| r.id).collect();
        assert_eq!(pending, [asked.id]);

        // A restart: the channel and its actors are gone, the approver's
        // memory with them; the channel is built again from the stores.
        running.abort();
        let _ = running.await;
        let document = Channel::clone(&channel.channel());
        let manager = Arc::clone(channel.runtime.new_actor_runtime(ActorId::System, document.session_id, document.id).runtime_manager());
        let executor = CommandExecutor::empty(Arc::clone(manager.security_manager()));
        let restarted = ChannelLoop::new(document.clone(), ChannelRuntime::outside_a_session(manager, Arc::new(executor), document.session_id)).await.unwrap();
        let mut events = restarted.observe();
        let running = tokio::spawn({ let restarted = Arc::clone(&restarted); async move { restarted.run().await } });
        let helper = restarted.actor_by_id(ActorId::Agent(agent)).unwrap();

        // The agent asks again, with the same id, and the users can decide.
        let asked_again = approval_requested(&mut events).await;
        assert_eq!(asked_again.id, asked.id);
        let alice = document.participants.iter().find_map(|p| match p { Participant::User(id) => Some(*id), _ => None }).expect("alice");
        let response = caelumora_model::approval::ApprovalResponse::new(asked.id, agent);
        restarted.user_actor(alice).unwrap().send_approval(response, true).await.unwrap();
        let said: Vec<_> = answered(&mut events, agent).await.iter().filter_map(|m| m.content.clone()).collect();
        assert_eq!(said, ["Done."], "the call runs and the turn goes on");
        settled(&helper, ActorState::Idle).await;
        running.abort();
    }

    /// An agent whose model calls the `sleep` tool, which sleeps longer
    /// than any test waits, and would then say what it is never asked for.
    async fn with_an_agent_in_a_long_tool() -> (Stuck, Arc<Scripted>) {
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        let call = Message {
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "sleep".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "sleep".to_string(),
            kind: crate::tool_impl::sleep_tool::SLEEP_TOOL_KIND.to_string(),
            params: serde_json::json!({ "seconds": 60 }),
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }] };
        let unasked = Message { role: MessageRole::Assistant, content: Some("never given".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let model = Arc::new(Scripted(std::sync::Mutex::new([call, unasked].into())));
        let stuck = with_an_agent(Arc::<Scripted>::clone(&model), components).await;
        settled(&stuck.helper, ActorState::ExecutingTools).await;
        (stuck, model)
    }

    #[tokio::test]
    async fn a_turn_asked_for_again_while_it_waits_for_an_approval_withdraws_the_request() {
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        let plan = Message {
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall {
                id: Some("1".to_string()),
                function_name: "create_plan".to_string(),
                arguments: serde_json::json!({ "summary": "one step", "steps": [{ "description": "first" }] }),
                visibility: ToolVisibility::SHOWN,
                presentation: Default::default(),
            }],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let instead = Message { role: MessageRole::Assistant, content: Some("no plan needed".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "plan".to_string(),
            kind: crate::component_impl::create_plan_tool::CREATE_PLAN_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }] };
        let Stuck { channel, context, agent, helper, mut events, running } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([plan, instead].into()))), components).await;
        let ActorEvent::ApprovalRequested(request) = shown_by(&mut events, agent, "ApprovalRequested").await else { unreachable!() };
        settled(&helper, ActorState::WaitingForUser).await;

        let turn = channel.turn_under_way(ActorId::Agent(agent)).expect("waiting is under way");
        channel.user_actor(alice_of(&context)).unwrap().request_retry_of_turn(ActorId::Agent(agent), turn).await.expect("asked");
        assert!(matches!(shown_by(&mut events, agent, "ApprovalRequestWithdrawn").await, ActorEvent::ApprovalRequestWithdrawn { id } if id == request.id),
            "nobody is asked any longer");
        assert!(matches!(shown_by(&mut events, agent, "RetryAccepted").await, ActorEvent::RetryAccepted { target } if target == RetryTarget::Turn(turn)));
        let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
        assert_eq!(said(answered(&mut events, agent).await), ["no plan needed"]);
        running.abort();
    }

    #[tokio::test]
    async fn a_turn_asked_for_again_while_a_tool_runs_tells_the_next_one_of_the_call() {
        let (Stuck { channel, context, agent, mut events, running, .. }, _) = with_an_agent_in_a_long_tool().await;
        let turn = channel.turn_under_way(ActorId::Agent(agent)).expect("under way");
        channel.user_actor(alice_of(&context)).unwrap().request_retry_of_turn(ActorId::Agent(agent), turn).await.expect("asked");

        let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
        assert_eq!(said(answered(&mut events, agent).await), ["never given"], "the tool is interrupted, and the next turn answers");
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        let given_up = state.transcript.given_up_turns();
        assert_eq!(given_up.len(), 1, "the turn is kept as given up");
        assert_eq!(given_up[0].given_up.reason, caelumora_model::transcript::GivenUpReason::Retried);
        let told = state.transcript.messages().into_iter()
            .find(|m| m.internal && m.content.as_deref().is_some_and(|c| c.contains("sleep")))
            .expect("the model is told of the call that ran");
        assert!(told.content.unwrap().contains("asked for again"), "and why the turn was given up");
        assert_eq!(told.turn_id, Some(turn), "as the record of that turn");
        let history: Vec<_> = channel.history().await.into_iter().filter_map(|m| m.content).collect();
        assert_eq!(history, ["hello", "never given"], "nothing of the given-up turn reached the channel");
        running.abort();
    }

    #[tokio::test]
    async fn a_participant_is_added_again_only_once_its_old_actor_has_ended() {
        let (channel, _, agent, _) = agent_channel(Arc::new(DummySecurityManager::new()), Arc::new(NeverAnswers), Default::default()).await;
        let participant = ActorId::Agent(agent);
        assert!(channel.left(participant).await.is_ok(), "nothing to wait for: it never left");

        // Its old actor takes a while to end, as one that stores its state.
        let old = TaskTracker::new();
        let ended = Arc::new(AtomicBool::new(false));
        let ending = Arc::clone(&ended);
        old.spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            ending.store(true, Ordering::SeqCst);
        });
        channel.leaving.lock().unwrap().insert(participant, old);

        channel.left(participant).await.expect("it ends within the grace");
        assert!(ended.load(Ordering::SeqCst), "not before its old actor ended");
        assert!(!channel.leaving.lock().unwrap().contains_key(&participant), "and is forgotten");
    }

    #[tokio::test]
    async fn a_cancel_while_a_tool_runs_interrupts_the_tool_and_ends_the_turn() {
        use caelumora_model::tool::ToolResultStatus;

        let (Stuck { channel, context, agent, helper, mut events, running }, model) = with_an_agent_in_a_long_tool().await;

        channel.cancel(&context).await.expect("permitted");

        // The turn ends at once, not when the tool would have (issue 161),
        // and the model is not asked again.
        let ended = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == ActorId::Agent(agent) {
                    if let ActorEvent::FullResponseGenerated(messages) = envelope.event { break messages }
                }
            }
        }).await.expect("the cancelled turn ends without waiting for the tool");
        let results: Vec<_> = ended.iter().flat_map(|m| m.tool_results.iter()).collect();
        assert_eq!(results.len(), 1, "{ended:?}");
        assert_eq!(results[0].status, ToolResultStatus::Interrupted);
        assert!(results[0].started_at.is_some(), "it ran");
        settled(&helper, ActorState::Idle).await;
        assert_eq!(model.0.lock().unwrap().len(), 1, "the second answer was never asked for");
        running.abort();
    }

    #[tokio::test]
    async fn an_agent_removed_while_a_tool_runs_gives_up_its_turn_and_is_told_the_tool_ran() {
        use caelumora_model::transcript::GivenUpReason;

        let (Stuck { channel, context, agent, running, .. }, _) = with_an_agent_in_a_long_tool().await;

        channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");

        // Within the grace of a stop (issue 161): the turn is given up rather
        // than the actor aborted with the turn left as it was before the tool.
        let stored = tokio::time::timeout(STOP_GRACE, async {
            loop {
                let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
                if state.turn.is_none() && !state.transcript.given_up_turns().is_empty() { break state }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("the turn is given up before the grace runs out");
        assert_eq!(stored.transcript.given_up_turns()[0].given_up.reason, GivenUpReason::Stopped);
        let told = stored.transcript.messages().last().and_then(|m| m.content.clone()).unwrap_or_default();
        assert!(told.contains("- sleep(") && told.contains("interrupted"), "the agent is told the call ran: {told}");
        running.abort();
    }

    #[tokio::test]
    async fn an_agent_removed_while_waiting_for_approval_gives_up_its_turn() {
        use caelumora_model::transcript::GivenUpReason;

        let (call, components) = a_call_needing_approval();
        let Stuck { channel, context, agent, helper, running, .. } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([call].into()))), components).await;
        settled(&helper, ActorState::WaitingForUser).await;

        channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");

        // It stops waiting and gives the turn up, instead of being aborted
        // with the turn left in its state.
        let stored = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
                if state.turn.is_none() && !state.transcript.given_up_turns().is_empty() { break state }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("the turn is given up");
        assert_eq!(stored.transcript.given_up_turns()[0].given_up.reason, GivenUpReason::Stopped);
        running.abort();
    }

    #[tokio::test]
    async fn a_call_with_invalid_arguments_is_answered_without_asking_anyone() {
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility, ToolResultStatus};

        // `create_plan` needs approval; this call's arguments are no plan.
        let not_a_plan = Message {
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "create_plan".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let answer = Message { role: MessageRole::Assistant, content: Some("Let me answer directly.".to_string()), ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "plan".to_string(),
            kind: crate::component_impl::create_plan_tool::CREATE_PLAN_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }] };
        let Stuck { agent, helper, mut events, running, .. } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([not_a_plan, answer].into()))), components).await;

        // The turn ends without anyone approving anything: the call was
        // answered as invalid, and the model tried again.
        let ended = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == ActorId::Agent(agent) {
                    if let ActorEvent::FullResponseGenerated(messages) = envelope.event { break messages }
                }
            }
        }).await.expect("the turn ends without an approval");
        let results: Vec<_> = ended.iter().flat_map(|m| m.tool_results.iter()).collect();
        assert_eq!(results.len(), 1, "{ended:?}");
        assert_eq!(results[0].status, ToolResultStatus::InvalidArguments, "the model is asked to correct the call");
        assert_eq!(ended.last().and_then(|m| m.content.as_deref()), Some("Let me answer directly."));
        settled(&helper, ActorState::Idle).await;
        running.abort();
    }

    /// Waits for `actor` to reach `wanted`.
    async fn settled(actor: &Arc<dyn Actor>, wanted: ActorState) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while actor.state() != wanted { tokio::time::sleep(Duration::from_millis(10)).await }
        }).await.unwrap_or_else(|_| panic!("the agent does not reach {wanted:?}, it is {:?}", actor.state()));
    }

    #[tokio::test]
    async fn a_cancel_interrupts_a_model_call_that_is_in_flight() {
        let Stuck { channel, context, agent, helper, mut events, running } = stuck_in_a_model_call().await;
        let agent_id = ActorId::Agent(agent);

        channel.cancel(&context).await.expect("permitted");

        // The turn ends at once - with an empty answer, so that observers
        // stop waiting - instead of when the model would have answered.
        let ended = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == agent_id && matches!(envelope.event, ActorEvent::FullResponseGenerated(_)) {
                    break envelope;
                }
            }
        }).await.expect("the cancelled turn ends without the model");
        assert!(matches!(ended.event, ActorEvent::FullResponseGenerated(ref messages) if messages.is_empty()));
        settled(&helper, ActorState::Idle).await;
        running.abort();
    }

    #[tokio::test]
    async fn a_user_learns_that_a_turn_cancelled_before_it_said_anything_ended() {
        let Stuck { channel, context, agent, running, .. } = stuck_in_a_model_call().await;
        let mut followed = channel.user_actor(alice_of(&context)).unwrap().watch().events;

        channel.cancel(&context).await.expect("permitted");

        // Nothing to hear of it - but the end, for whoever shows the answer
        // as under way (issue 224).
        let ended = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = followed.recv().await.expect("the user's actor runs");
                if envelope.context.sender_id == ActorId::Agent(agent) {
                    if let ActorEvent::FullResponseGenerated(messages) = envelope.event { break messages }
                }
            }
        }).await.expect("the user learns that the answer ended");
        assert!(ended.is_empty(), "with nothing said: {ended:?}");
        running.abort();
    }

    #[tokio::test]
    async fn a_removed_agent_abandons_its_turn_and_leaves_an_idle_state_behind() {
        let Stuck { channel, context, agent, helper, mut events, running } = stuck_in_a_model_call().await;

        channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");

        // The turn ends without an answer - nothing is announced - and the
        // state in the store is the one a later actor of the agent starts
        // from: idle, with no turn to resume.
        settled(&helper, ActorState::Idle).await;
        let stored = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
                if state.processing_state == ActorState::Idle { break state }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("the idle state is stored");
        assert!(stored.turn.is_none(), "the turn was abandoned, not stored");
        assert!(stored.transcript.messages().iter().all(|m| m.sender_id != ActorId::Agent(agent)), "nothing of it was recorded");
        while let Ok(envelope) = events.try_recv() {
            assert!(!matches!(envelope.event, ActorEvent::FullResponseGenerated(_)) || envelope.context.sender_id != ActorId::Agent(agent), "a stopped agent announces no answer");
        }
        // The channel is still running: alice is still in it.
        assert!(!running.is_finished());
        running.abort();
    }

    #[tokio::test]
    async fn a_removal_is_announced_in_the_channel_in_the_name_of_who_removed() {
        let Stuck { channel, context, agent, helper, mut events, running } = stuck_in_a_model_call().await;

        channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");

        // What a frontend needs to end the turn it shows: who is gone.
        let removed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if let ActorEvent::ParticipantRemoved { participant } = envelope.event {
                    break (envelope.context.sender_id, envelope.context.sender_name, participant);
                }
            }
        }).await.expect("the removal is announced");
        assert_eq!(removed, (context.caller_id, "alice".to_string(), ActorId::Agent(agent)));
        settled(&helper, ActorState::Idle).await;
        running.abort();
    }

    #[tokio::test]
    async fn nothing_of_a_removed_participant_enters_the_channel() {
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(DummySecurityManager::new())).await;
        let mut events = channel.observe();
        let gone = ActorId::Agent(caelumora_model::agent::AgentId::new());
        let late = Envelope::new(SecurityContext::default(), EventContext::new(channel.session_id, channel.id, gone, "helper".to_string()), ActorEvent::TurnFailed { message: "too late".to_string() });

        // Its actor no longer runs: it was removed while finishing a step.
        channel.handle_actor_event(Some(late), &HashMap::new()).await;

        assert!(events.try_recv().is_err(), "what a removed participant sends is dropped");
    }


    /// A running channel with alice and an external agent (issue 179) that
    /// `remote` stands in for, after alice said "hello". The remote agent is
    /// its personality's component, of a kind registered for the test.
    async fn with_an_external_agent(remote: Arc<crate::external::testing::ScriptedExternalAgent>) -> Stuck {
        let channel = external_channel(remote, external_components(), |_| None).await.expect("the external agent can be run");
        start(channel).await
    }

    /// The components of an external personality: the remote agent, of the
    /// kind `external_channel` registers for it.
    fn external_components() -> caelumora_model::component::ComponentConfig {
        caelumora_model::component::ComponentConfig { component_nodes: vec![caelumora_model::component::ComponentDescriptor::new("remote", "test_remote")] }
    }

    /// A channel of alice and an external agent with `components`, built but
    /// not running - with the agent's state in it `stored`, as a restart
    /// finds it.
    async fn external_channel(remote: Arc<crate::external::testing::ScriptedExternalAgent>, components: caelumora_model::component::ComponentConfig, stored: impl FnOnce(caelumora_model::agent::AgentId) -> Option<caelumora_model::agent::AgentChannelState>) -> Result<(Arc<ChannelLoop>, SecurityContext, caelumora_model::agent::AgentId, UserId)> {
        use caelumora_model::agent::{Agent, Personality, PersonalityId, PersonalityKind};
        use caelumora_model::scope::Scope;

        let store = Arc::new(MemoryStore::new());
        let personality = Personality {
            description: None,
            introduction: None,
            reasoning_visibility: caelumora_model::confidentiality::Visibility::Channel,
            id: PersonalityId::new(),
            name: ScopedName::new("remote", Default::default()).unwrap(),
            kind: PersonalityKind::External,
            roles: None,
            components,
            security_attributes: Default::default(),
            deleted: None,
        };
        store.create_personality(&SecurityContext::system(), &personality).await.unwrap();
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let agent = Agent::new(ScopedName::new("hermes", Scope::GlobalScope).unwrap(), &personality, context.initiator_id);
        let agent_id = agent.id;
        store.create_user(&SecurityContext::system(), &alice).await.unwrap();
        store.create_agent(&SecurityContext::system(), &agent).await.unwrap();

        let security: Arc<dyn SecurityManager> = Arc::new(FlowRule);
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&security)));
        let mut registry = crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store));
        registry.register(
            crate::component::ComponentKind::new("test_remote").provides::<dyn crate::external::ExternalAgent>(),
            move |_descriptor, _context| {
                let mut facets = crate::component::ComponentFacets::empty();
                facets.insert::<dyn crate::external::ExternalAgent>(Arc::<crate::external::testing::ScriptedExternalAgent>::clone(&remote));
                Ok(facets)
            },
        );
        register_no_secrets(&mut registry);
        register_heard(&mut registry);
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), store, secrets, security, default_commands(), Default::default(), Vec::new(), registry).unwrap());
        let channel = Channel {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            id: ChannelId::new(),
            session_id: SessionId::new(),
            name: Name::new("main").unwrap(),
            components: caelumora_model::channel::moderation_components(None, Some("broadcast_policy")),
            participants: vec![Participant::User(alice.id), Participant::Agent(agent_id)],
            security_attributes: SecurityAttributes::default(),
            classification: caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT,
            language: None,
            history_limits: Vec::new(),
            deleted: None,
            former: Vec::new(),
        };
        runtime.entity_store().create_channel(&SecurityContext::system(), &channel).await.unwrap();
        if let Some(mut stored) = stored(agent_id) {
            runtime.state_store().put_agent_channel_state(agent_id, channel.id, &mut stored).await.unwrap();
        }
        let executor = CommandExecutor::empty(Arc::clone(runtime.security_manager()));
        let session_id = channel.session_id;
        let channel = ChannelLoop::new(channel, ChannelRuntime::outside_a_session(runtime, Arc::new(executor), session_id)).await?;
        Ok((channel, context, agent_id, alice.id))
    }

    /// Lets nothing through that mentions a secret.
    #[derive(Debug)]
    struct NoSecrets;

    #[async_trait::async_trait]
    impl crate::message::MessageProcessor for NoSecrets {
        async fn process_messages(&self, input: Vec<Message>, _security: &SecurityContext) -> Result<Vec<Message>> {
            Ok(input.into_iter().filter(|m| !m.content.as_deref().unwrap_or_default().contains("secret")).collect())
        }
    }

    impl crate::message::PerceiveInProcessor for NoSecrets {}

    /// A prompt part that changes with every message the agent hears: how
    /// many its history holds.
    #[derive(Debug)]
    struct Heard;

    #[async_trait::async_trait]
    impl crate::context::PromptProvider for Heard {
        async fn get_prompt(&self, work: &crate::work::AgentWork<'_>) -> Result<crate::context::Prompt> {
            Ok(crate::context::Prompt::from(format!("[Heard]\n{}", work.agent_channel_state().history.messages().len())))
        }
    }

    /// Registers `Heard` as the kind `test_heard`.
    fn register_heard(registry: &mut crate::component::ComponentRegistry) {
        registry.register(
            crate::component::ComponentKind::new("test_heard").provides::<dyn crate::context::PromptProvider>(),
            |_descriptor, _context| {
                let mut facets = crate::component::ComponentFacets::empty();
                facets.insert::<dyn crate::context::PromptProvider>(Arc::new(Heard));
                Ok(facets)
            },
        );
    }

    /// Registers `NoSecrets` as the kind `test_no_secrets`.
    /// A tool that asks the agent's owner one question before it runs - its
    /// questions `sensitive` as its parameter says - and gives what it was
    /// answered, or of sensitive answers only how many there were.
    #[derive(Debug)]
    struct Asking {
        sensitive: bool,
    }

    #[async_trait::async_trait]
    impl crate::tool::ToolProvider for Asking {
        fn description(&self) -> String {
            "asks first".to_string()
        }
        fn tools(&self) -> Vec<caelumora_model::tool::Tool> {
            vec![caelumora_model::tool::Tool {
                name: "asking".to_string(),
                description: "asks first".to_string(),
                parameters: caelumora_model::tool::ToolParameters::object(Vec::<(String, caelumora_model::tool::ToolParameters)>::new(), Vec::<String>::new()),
                requires_approval: false,
                visibility: Default::default(),
                classification: None,
                presentation: Default::default(),
            }]
        }
        async fn execute(&self, _name: &str, _params: &serde_json::Value, answers: Option<&caelumora_model::question::Answers>, _turn: &mut crate::agent_turn::Turn) -> crate::tool::ToolResult<crate::tool::ToolOutput> {
            let answers = answers.ok_or_else(|| crate::tool::ToolError::ExecutionFailed("no answers".to_string()))?;
            if self.sensitive {
                return Ok(serde_json::json!({ "received": answers.responses.len() }).into());
            }
            Ok(serde_json::json!({ "answered_by": answers.answered_by_name, "responses": answers.responses }).into())
        }
        fn questions(&self, _name: &str, _params: &serde_json::Value) -> crate::tool::ToolResult<Option<caelumora_model::question::Questions>> {
            use caelumora_model::question::{Addressee, Question, Questions};
            Ok(Some(Questions {
                questions: vec![Question { header: "Secret".to_string(), question: "What is it?".to_string(), choices: Vec::new(), multi_select: false }],
                ask: vec![Addressee::Owner],
                sensitive: self.sensitive,
            }))
        }
    }

    fn register_asking_tool(registry: &mut crate::component::ComponentRegistry) {
        registry.register(
            crate::component::ComponentKind::new("test_asking_tool").provides::<dyn crate::tool::ToolProvider>(),
            |descriptor, _context| {
                let sensitive = descriptor.params.get("sensitive").and_then(serde_json::Value::as_bool).unwrap_or(false);
                let mut facets = crate::component::ComponentFacets::empty();
                facets.insert::<dyn crate::tool::ToolProvider>(Arc::new(Asking { sensitive }));
                Ok(facets)
            },
        );
    }

    /// A call of the `asking` tool, the component that gives it -
    /// `sensitive` or not - and what the model says once it has its result.
    fn a_call_asking(sensitive: bool) -> (Message, Message, caelumora_model::component::ComponentConfig) {
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        let call = Message {
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "asking".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let done = Message { role: MessageRole::Assistant, content: Some("Done.".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "asking".to_string(),
            kind: "test_asking_tool".to_string(),
            params: serde_json::json!({ "sensitive": sensitive }),
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }] };
        (call, done, components)
    }

    /// The questions the users are asked, from `events`.
    async fn questions_requested(events: &mut broadcast::Receiver<Envelope>) -> caelumora_model::question::QuestionRequest {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if let ActorEvent::QuestionRequested(request) = envelope.event {
                    break request;
                }
            }
        }).await.expect("the users are asked")
    }

    #[tokio::test]
    async fn a_call_waits_for_the_answers_and_the_turn_goes_on_with_them() {
        use caelumora_model::question::Response;
        let (call, done, components) = a_call_asking(false);
        let Stuck { channel, context, agent, helper, mut events, running } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([call, done].into()))), components).await;
        let asked = questions_requested(&mut events).await;
        settled(&helper, ActorState::WaitingForUser).await;
        let alice = alice_of(&context);
        assert_eq!(asked.questions.asked(), [ActorId::User(alice)], "the owner, found among the users");

        // The turn is their record, as for approvals.
        let stored = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert_eq!(stored.pending_questions().into_iter().map(|q| q.id).collect::<Vec<_>>(), [asked.id]);

        let actor = channel.user_actor(alice).unwrap();
        let error = actor.answer_questions(asked.id, Some(vec![])).await.expect_err("one response per question");
        assert!(matches!(error.downcast_ref::<ChannelError>(), Some(ChannelError::Question { .. })), "{error}");
        actor.answer_questions(asked.id, Some(vec![Response::Text { text: "Kaya".to_string() }])).await.expect("answered");

        let said: Vec<_> = answered(&mut events, agent).await.iter().filter_map(|m| m.content.clone()).collect();
        assert_eq!(said, ["Done."], "the turn goes on");
        settled(&helper, ActorState::Idle).await;
        let stored = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        let result = stored.transcript.messages().into_iter().flat_map(|m| m.tool_results).next().expect("the call ran");
        assert!(result.content.contains("Kaya") && result.content.contains("alice"), "the tool gave the model what it was answered: {}", result.content);
        let recorded = |result: &caelumora_model::tool::ToolCallResult| result.answers.as_ref().map(|a| (a.answered_by, a.responses.clone()));
        let kaya = Some((ActorId::User(alice), vec![Response::Text { text: "Kaya".to_string() }]));
        assert_eq!(recorded(&result), kaya, "the transcript keeps the record of the answers");
        let history = channel.history().await;
        assert_eq!(history.iter().flat_map(|m| &m.tool_results).map(recorded).next(), Some(kaya), "and so does the history");
        let error = actor.answer_questions(asked.id, Some(vec![Response::Text { text: "again".to_string() }])).await.expect_err("closed");
        assert!(matches!(error.downcast_ref::<ChannelError>(), Some(ChannelError::Question { reason: caelumora_model::question::QuestionError::NotOpen, .. })), "{error}");
        running.abort();
    }

    #[tokio::test]
    async fn a_message_instead_of_the_answers_declines_them_and_is_heard_after_the_turn() {
        let (call, done, components) = a_call_asking(false);
        let Stuck { channel, context, agent, helper, mut events, running } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([call, done].into()))), components).await;
        questions_requested(&mut events).await;
        settled(&helper, ActorState::WaitingForUser).await;

        channel.user_actor(alice_of(&context)).unwrap().send_message("rather not".to_string(), None).await.unwrap();
        let said: Vec<_> = answered(&mut events, agent).await.iter().filter_map(|m| m.content.clone()).collect();
        assert_eq!(said, ["Done."], "the turn goes on without the answers");
        let stored = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        let result = stored.transcript.messages().into_iter().flat_map(|m| m.tool_results).next().expect("the call is answered");
        assert_eq!(result.status, caelumora_model::tool::ToolResultStatus::Declined);
        tokio::time::timeout(Duration::from_secs(5), async {
            while !channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap().transcript.messages().iter().any(|m| m.content.as_deref() == Some("rather not")) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("the message is heard after the turn");
        running.abort();
    }

    #[tokio::test]
    async fn sensitive_answers_reach_the_tool_alone_and_are_never_stored() {
        use caelumora_model::question::Response;
        let (call, done, components) = a_call_asking(true);
        let Stuck { channel, context, agent, helper, mut events, running } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([call, done].into()))), components).await;
        let asked = questions_requested(&mut events).await;
        assert_eq!(asked.shown_to(), Some(vec![ActorId::User(alice_of(&context))]));
        settled(&helper, ActorState::WaitingForUser).await;

        channel.user_actor(alice_of(&context)).unwrap().answer_questions(asked.id, Some(vec![Response::Text { text: "hunter2".to_string() }])).await.expect("answered");
        answered(&mut events, agent).await;
        settled(&helper, ActorState::Idle).await;
        let stored = serde_json::to_string(&channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap()).unwrap();
        assert!(!stored.contains("hunter2"), "neither in the turn nor in the transcript");
        assert!(channel.history().await.iter().all(|m| !format!("{m:?}").contains("hunter2")), "nor in the history");
        running.abort();
    }

    #[tokio::test]
    async fn ask_user_asks_whom_it_names_and_gives_the_model_the_answers() {
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::question::Response;
        use caelumora_model::tool::{ToolCall, ToolResultStatus, ToolVisibility};

        let ask = |id: &str, whom: &str| Message {
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall {
                id: Some(id.to_string()),
                function_name: "ask_user".to_string(),
                arguments: serde_json::json!({ "questions": [{ "header": "Name", "question": "What should it be called?" }], "ask": [whom] }),
                visibility: ToolVisibility::SHOWN,
                presentation: Default::default(),
            }],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let done = Message { role: MessageRole::Assistant, content: Some("Done.".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor::new("ask", crate::tool_impl::ask_user_tool::ASK_USER_TOOL_KIND)] };
        let model = Scripted(std::sync::Mutex::new([ask("1", "@carol"), ask("2", "@alice"), done].into()));
        let Stuck { channel, context, agent, helper, mut events, running } = with_an_agent(Arc::new(model), components).await;

        // Nobody called carol takes part: the first call is answered at
        // once, and the model asks alice.
        let asked = questions_requested(&mut events).await;
        assert_eq!(asked.questions.asked(), [ActorId::User(alice_of(&context))]);
        settled(&helper, ActorState::WaitingForUser).await;
        channel.user_actor(alice_of(&context)).unwrap().answer_questions(asked.id, Some(vec![Response::Text { text: "Caelumora".to_string() }])).await.expect("answered");
        let said: Vec<_> = answered(&mut events, agent).await.iter().filter_map(|m| m.content.clone()).collect();
        assert_eq!(said, ["Done."]);

        let stored = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        let results: Vec<_> = stored.transcript.messages().into_iter().flat_map(|m| m.tool_results).collect();
        let [unknown, answer] = results.as_slice() else { panic!("two calls: {results:?}") };
        assert_eq!(unknown.status, ToolResultStatus::InvalidArguments);
        assert!(unknown.content.contains("@carol"), "the model is told whom it named: {}", unknown.content);
        assert!(answer.content.contains("Caelumora") && answer.content.contains("alice"), "{}", answer.content);
        running.abort();
    }

    fn register_no_secrets(registry: &mut crate::component::ComponentRegistry) {
        registry.register(
            crate::component::ComponentKind::new("test_no_secrets").provides::<dyn crate::message::PerceiveInProcessor>(),
            |_descriptor, _context| {
                let mut facets = crate::component::ComponentFacets::empty();
                facets.insert::<dyn crate::message::PerceiveInProcessor>(Arc::new(NoSecrets));
                Ok(facets)
            },
        );
    }

    #[tokio::test]
    async fn an_agent_that_joins_hears_another_agents_answer_as_its_words_without_the_tool_traffic() {
        use caelumora_model::message::{MessageId, MessageRole};
        use caelumora_model::tool::{ToolCall, ToolCallResult, ToolVisibility};
        let (channel, _, agent, _) = agent_channel(Arc::new(DummySecurityManager::new()), Arc::new(NeverAnswers), Default::default()).await;
        let other = ActorId::Agent(caelumora_model::agent::AgentId::new());
        let call = ToolCall { id: Some("1".to_string()), function_name: "noop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() };
        let result = ToolCallResult::of(&call, Default::default(), "done".to_string(), Classification::CHANNEL_DEFAULT);
        // Another agent's answer, as the channel's history keeps it.
        let answer = Message { id: MessageId::new(), sender_id: other, sender_name: "hermes".to_string(), role: MessageRole::Assistant, content: Some("Let me check.".to_string()), tool_calls: vec![call], ..Message::new(Classification::CHANNEL_DEFAULT) };
        let tool = Message { id: MessageId::new(), sender_id: other, role: MessageRole::Tool, content: Some("done".to_string()), tool_results: vec![result], ..Message::new(Classification::CHANNEL_DEFAULT) };
        let actor = channel.actor_by_id(ActorId::Agent(agent)).unwrap();

        actor.catch_up(vec![answer.clone(), tool.clone()]).await.unwrap();

        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        let heard = state.transcript.messages();
        assert_eq!(heard.len(), 1, "nothing of the tool message: {heard:?}");
        assert_eq!((heard[0].id, heard[0].role, heard[0].content.as_deref()), (answer.id, MessageRole::User, Some("Let me check.")), "the other's words, as another's");
        assert!(heard[0].tool_calls.is_empty(), "not the other's call, as if its own");
        let kept: Vec<_> = state.history.messages().iter().map(|m| (m.id, m.role)).collect();
        assert_eq!(kept, [(answer.id, MessageRole::Assistant), (tool.id, MessageRole::Tool)], "the history keeps what was said, as it was said");
    }

    #[tokio::test]
    async fn an_agent_that_joins_takes_in_what_it_missed_as_what_it_hears() {
        use caelumora_model::message::{MessageId, MessageRole};
        let components = caelumora_model::component::ComponentConfig { component_nodes: vec![caelumora_model::component::ComponentDescriptor::new("filter", "test_no_secrets")] };
        let (channel, context, agent, _) = agent_channel(Arc::new(DummySecurityManager::new()), Arc::new(NeverAnswers), components).await;
        let said = |text: &str| Message {
            id: MessageId::new(), role: MessageRole::User, sender_id: context.initiator_id, content: Some(text.to_string()),
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let actor = channel.actor_by_id(ActorId::Agent(agent)).unwrap();

        actor.catch_up(vec![said("hello"), said("the secret is 42")]).await.unwrap();

        let contents = |messages: Vec<Message>| messages.into_iter().filter_map(|m| m.content).collect::<Vec<_>>();
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert_eq!(contents(state.history.messages()), ["hello", "the secret is 42"], "its history keeps all it missed");
        assert_eq!(contents(state.transcript.messages()), ["hello"], "its transcript what passed its processors");
    }

    #[tokio::test]
    async fn an_agent_keeps_all_it_was_offered_in_its_history_and_what_passes_its_processors_in_its_transcript() {
        let hi = Message { role: caelumora_model::message::MessageRole::Assistant, content: Some("Hi.".to_string()), ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) };
        let components = caelumora_model::component::ComponentConfig { component_nodes: vec![caelumora_model::component::ComponentDescriptor::new("filter", "test_no_secrets")] };
        let Stuck { channel, context, agent, helper, mut events, running } = with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([hi].into()))), components).await;
        answered(&mut events, agent).await;
        settled(&helper, ActorState::Idle).await;

        // Heard between turns; the turn it starts waits for a model that
        // does not answer any more.
        channel.user_actor(alice_of(&context)).unwrap().send_message("the secret is 42".to_string(), None).await.unwrap();
        settled(&helper, ActorState::Planning).await;

        let contents = |messages: Vec<Message>| messages.into_iter().filter_map(|m| m.content).collect::<Vec<_>>();
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert!(contents(state.history.messages()).contains(&"the secret is 42".to_string()), "its history keeps all it was offered");
        assert!(!contents(state.transcript.messages()).contains(&"the secret is 42".to_string()), "its transcript only what passed");
        assert!(contents(state.transcript.messages()).contains(&"hello".to_string()));
        running.abort();
    }

    #[tokio::test]
    async fn an_external_agent_that_joins_keeps_what_it_may_see_and_on_joining_again_what_it_missed() {
        use caelumora_model::message::{MessageId, MessageRole};
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::default());
        let (channel, context, agent, _) = external_channel(remote, external_components(), |_| None).await.unwrap();
        let message = |sender: ActorId, text: &str| Message {
            id: MessageId::new(), role: MessageRole::User, sender_id: sender, content: Some(text.to_string()),
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let said = |text: &str| message(context.initiator_id, text);
        let (one, two, three) = (said("one"), said("two"), said("three"));
        let own = message(ActorId::Agent(agent), "mine");
        let actor = channel.actor_by_id(ActorId::Agent(agent)).unwrap();
        let kept = || async {
            channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap()
                .history.messages().into_iter().map(|m| m.content.unwrap()).collect::<Vec<_>>()
        };

        actor.catch_up(vec![two.clone()]).await.unwrap();
        assert_eq!(kept().await, ["two"], "what it may see");
        actor.catch_up(vec![one, two, own, three]).await.unwrap();
        assert_eq!(kept().await, ["two", "three"], "what it missed, and nothing older nor its own");
    }

    #[tokio::test]
    async fn an_external_agent_does_not_answer_again() {
        use crate::external::ExternalEvent;
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Message("Hello, alice.".to_string())), Ok(ExternalEvent::Completed)],
        ]));
        let built = external_channel(Arc::clone(&remote), external_components(), |_| None).await.expect("the external agent can be run");
        let Stuck { channel, context, agent, mut events, running, .. } = start(built).await;
        let answer = answered(&mut events, agent).await.pop().expect("an answer");

        channel.user_actor(alice_of(&context)).unwrap().request_retry(answer.id).await.expect("asked");
        let ActorEvent::RetryRejected { target, reason } = shown_by(&mut events, agent, "RetryRejected").await else { unreachable!() };
        assert_eq!((target, reason), (RetryTarget::Message(answer.id), caelumora_model::agent::RetryRefused::Elsewhere), "the remote agent keeps what it was given");
        running.abort();
    }

    #[tokio::test]
    async fn an_external_agent_s_answer_under_way_is_not_asked_for_again() {
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::default());
        let Stuck { channel, context, agent, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;
        assert_eq!(channel.turn_under_way(ActorId::Agent(agent)), None, "it has no turn here to give up");
        let refused = channel.user_actor(alice_of(&context)).unwrap().request_retry_of_turn(ActorId::Agent(agent), TurnId::new()).await;
        assert!(matches!(refused.expect_err("refused").downcast_ref::<ChannelError>(), Some(ChannelError::NotUnderWay { .. })));
        running.abort();
    }

    #[tokio::test]
    async fn what_an_external_agents_perceive_processors_drop_is_kept_but_never_given() {
        use crate::external::ExternalEvent;

        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Message("Hello, alice.".to_string())), Ok(ExternalEvent::Completed)],
            vec![Ok(ExternalEvent::Completed)],
        ]));
        let mut components = external_components();
        components.component_nodes.push(caelumora_model::component::ComponentDescriptor::new("filter", "test_no_secrets"));
        let built = external_channel(Arc::clone(&remote), components, |_| None).await.expect("the external agent can be run");
        let Stuck { channel, context, agent, mut events, running, .. } = start(built).await;
        answered(&mut events, agent).await;

        let alice = channel.user_actor(alice_of(&context)).unwrap();
        alice.send_message("the secret is 42".to_string(), None).await.unwrap();
        assert!(answered(&mut events, agent).await.is_empty(), "nothing new reached it, so it is not asked");

        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        let contents = |messages: Vec<Message>| messages.into_iter().filter_map(|m| m.content).collect::<Vec<_>>();
        assert_eq!(contents(state.history.messages()), vec!["hello", "Hello, alice.", "the secret is 42"], "its history keeps all it was offered");
        assert_eq!(contents(state.transcript.all_entries().filter_map(|e| e.entry.message().cloned()).collect()), vec!["hello"], "its transcript what it was given");
        assert_eq!(remote.sent.lock().unwrap().len(), 1, "the secret was not given to the remote agent");

        alice.send_message("and now?".to_string(), None).await.unwrap();
        answered(&mut events, agent).await;
        let sent = remote.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1].1.said.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), vec!["and now?"], "what was dropped is not offered again");
        running.abort();
    }

    /// Runs `built` and has alice say "hello".
    async fn start((channel, context, agent, alice): (Arc<ChannelLoop>, SecurityContext, caelumora_model::agent::AgentId, UserId)) -> Stuck {
        let events = channel.observe();
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });
        channel.user_actor(alice).unwrap().send_message("hello".to_string(), None).await.unwrap();
        let helper = channel.actor_by_id(ActorId::Agent(agent)).unwrap();
        Stuck { channel, context, agent, helper, events, running }
    }

    /// What `agent` answered next.
    async fn answered(events: &mut tokio::sync::broadcast::Receiver<Envelope>, agent: caelumora_model::agent::AgentId) -> Vec<Message> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == ActorId::Agent(agent) {
                    if let ActorEvent::FullResponseGenerated(messages) = envelope.event { break messages }
                }
            }
        }).await.expect("the agent answers")
    }

    /// The user `context` acts for.
    fn alice_of(context: &SecurityContext) -> UserId {
        match context.initiator_id {
            ActorId::User(user) => user,
            other => panic!("{other} is no user"),
        }
    }

    fn task(id: &str) -> caelumora_model::external_agent::TaskId {
        caelumora_model::external_agent::TaskId(id.to_string())
    }

    /// What the remote agent was given in the call `answer` came of: the
    /// context recorded with it in the `state` of `agent` (issue 205).
    fn given_for(state: &caelumora_model::agent::AgentChannelState, agent: caelumora_model::agent::AgentId, answer: &Message) -> Vec<String> {
        let seen = crate::view::AgentContextView::of(ActorId::Agent(agent), state.clone());
        seen.context_of(answer.id).expect("recorded with the answer").items.into_iter()
            .filter_map(|item| match item {
                caelumora_model::context::ContextItem::Conversation { message } => message.content,
                caelumora_model::context::ContextItem::Prompt { .. } => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn an_external_agent_is_given_what_was_said_since_it_was_last_asked_and_answers_in_the_channel() {
        use crate::external::{ExternalEvent, Said};
        use caelumora_model::context::{ModelProfile, SenderFormat};
        use caelumora_model::external_agent::ConversationId;

        let conversation = ConversationId("c-1".to_string());
        // Told who said what, by the handle (issue 135).
        let marking = ModelProfile { sender_format: SenderFormat::Prefix, ..ModelProfile::default() };
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![
                Ok(ExternalEvent::Conversation(conversation.clone())),
                Ok(ExternalEvent::Started(task("t-1"))),
                Ok(ExternalEvent::Partial("Hel".to_string())),
                Ok(ExternalEvent::Message("Hello, alice.".to_string())),
                Ok(ExternalEvent::Completed),
            ],
            vec![Ok(ExternalEvent::Started(task("t-2"))), Ok(ExternalEvent::Message("Fine.".to_string())), Ok(ExternalEvent::Completed)],
        ]).marking(marking));
        let Stuck { channel, context, agent, mut events, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;

        let first = answered(&mut events, agent).await;
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].content.as_deref(), Some("Hello, alice."), "the whole message replaces its pieces");
        assert_eq!(first[0].sender_id, ActorId::Agent(agent));
        assert_eq!(channel.history().await.last().map(|m| m.id), Some(first[0].id), "in the channel's history");

        let alice = channel.user_actor(alice_of(&context)).unwrap();
        alice.send_message("How are you?".to_string(), None).await.unwrap();
        let second = answered(&mut events, agent).await;
        assert_eq!(second[0].content.as_deref(), Some("Fine."));

        let sent = remote.sent.lock().unwrap().clone();
        let said = |text: &str| Said { sender: "alice".to_string(), text: text.to_string() };
        assert_eq!(sent[0].0, None, "the remote agent opens the conversation");
        assert_eq!(sent[0].1.said, vec![said("@alice: hello")], "marked with its sender, as the component's profile says");
        assert_eq!(sent[1].0, Some(conversation.clone()), "and keeps it");
        assert_eq!(sent[1].1.said, vec![said("@alice: How are you?")], "only what it has not heard, not its own answer");

        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert_eq!(state.remote.conversation, Some(conversation));
        assert!(state.remote.open_tasks.is_empty(), "both tasks ended");
        let kept: Vec<_> = state.history.messages().into_iter().filter_map(|m| m.content).collect();
        assert_eq!(kept, vec!["hello", "Hello, alice.", "How are you?", "Fine."], "what it heard and what it answered");
        let given: Vec<_> = state.transcript.all_entries().filter_map(|e| e.entry.message()?.content.clone()).collect();
        assert_eq!(given, vec!["hello", "How are you?"], "what the remote agent was given, as received");
        assert!(state.transcript.messages().is_empty(), "all of it sent: nothing to give next");
        // The context of each answer is what was given in its call (issue 205).
        assert_eq!(given_for(&state, agent, &first[0]), ["@alice: hello"], "as it was given, marked");
        assert_eq!(given_for(&state, agent, &second[0]), ["@alice: How are you?"], "not what it was given before");
        running.abort();
    }

    /// A message the remote agent was given that is modified or taken back
    /// is told to it with its next call, at its place, in the name of its
    /// author (issue 236) - and is part of what that call's answer was given.
    #[tokio::test]
    async fn an_external_agent_tells_its_remote_agent_of_a_message_it_was_given_that_was_modified_or_taken_back() {
        use crate::external::{ExternalEvent, Said};
        use caelumora_model::context::{ModelProfile, SenderFormat};
        use caelumora_model::external_agent::ConversationId;

        let marking = ModelProfile { sender_format: SenderFormat::Prefix, ..ModelProfile::default() };
        let answer = |text: &str| vec![Ok(ExternalEvent::Message(text.to_string())), Ok(ExternalEvent::Completed)];
        let mut first = vec![Ok(ExternalEvent::Conversation(ConversationId("c-1".to_string())))];
        first.extend(answer("Hello."));
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![first, answer("Fine."), answer("Bye.")]).marking(marking));
        let Stuck { channel, context, agent, mut events, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;
        answered(&mut events, agent).await;
        let alice = channel.user_actor(alice_of(&context)).unwrap();
        let id_of = |history: Vec<Message>, text: &str| history.into_iter().find(|m| m.content.as_deref() == Some(text)).map(|m| m.id).expect("said");

        alice.modify_message(id_of(channel.history().await, "hello"), "hello, corrected".to_string(), None).await.unwrap();
        alice.send_message("And?".to_string(), None).await.unwrap();
        let second = answered(&mut events, agent).await;
        alice.revoke_message(id_of(channel.history().await, "And?")).await.unwrap();
        alice.send_message("Well?".to_string(), None).await.unwrap();
        answered(&mut events, agent).await;

        let sent = remote.sent.lock().unwrap().clone();
        let said = |text: &str| Said { sender: "alice".to_string(), text: text.to_string() };
        assert_eq!(sent[1].1.said, vec![said("@alice: [changed an earlier message \"hello\"; it now reads:] hello, corrected"), said("@alice: And?")], "told before what was said since");
        assert_eq!(sent[2].1.said, vec![said("@alice: [took back an earlier message \"And?\"; it no longer stands.]"), said("@alice: Well?")]);
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert_eq!(given_for(&state, agent, &second[0]), ["@alice: [changed an earlier message \"hello\"; it now reads:] hello, corrected", "@alice: And?"], "what its answer was given, word for word");
        assert!(state.history.messages().iter().all(|m| m.notice_of.is_none()), "a notice is never in the history");
        running.abort();
    }

    /// An external agent tells where it stands as an agent run here does
    /// (issue 243): working from before its answer starts until after it
    /// ends.
    #[tokio::test]
    async fn an_external_agent_tells_that_it_works_while_it_answers() {
        use crate::external::ExternalEvent;
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![vec![
            Ok(ExternalEvent::Started(task("t-1"))),
            Ok(ExternalEvent::Message("Hello, alice.".to_string())),
            Ok(ExternalEvent::Completed),
        ]]));
        let Stuck { mut events, agent, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;
        let told = tokio::time::timeout(Duration::from_secs(5), async {
            let mut told = Vec::new();
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id != ActorId::Agent(agent) { continue }
                match envelope.event {
                    // Where it starts, before anything was asked of it.
                    ActorEvent::StateChanged(ActorState::Idle) if told.is_empty() => {}
                    ActorEvent::StateChanged(ActorState::Idle) => { told.push("Idle"); break told }
                    ActorEvent::StateChanged(state) => told.push(if state == ActorState::Planning { "Planning" } else { "other" }),
                    ActorEvent::ResponseStarted { .. } => told.push("ResponseStarted"),
                    ActorEvent::FullResponseGenerated(_) => told.push("FullResponseGenerated"),
                    _ => {}
                }
            }
        }).await.expect("it answers and is idle again");
        assert_eq!(told, ["Planning", "ResponseStarted", "FullResponseGenerated", "Idle"]);
        running.abort();
    }

    #[tokio::test]
    async fn an_external_agent_s_answer_carries_the_turn_its_start_announced() {
        use crate::external::ExternalEvent;
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![vec![
            Ok(ExternalEvent::Started(task("t-1"))),
            Ok(ExternalEvent::Partial("Hel".to_string())),
            Ok(ExternalEvent::Message("Hello, alice.".to_string())),
            Ok(ExternalEvent::Completed),
        ]]));
        let Stuck { channel, agent, mut events, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;

        let (started, partial, answer) = tokio::time::timeout(Duration::from_secs(5), async {
            let (mut started, mut partial) = (None, None);
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id != ActorId::Agent(agent) { continue }
                match envelope.event {
                    ActorEvent::ResponseStarted { turn_id } => started = Some(turn_id),
                    ActorEvent::PartialMessageGenerated(m) => partial = m.turn_id,
                    ActorEvent::FullResponseGenerated(messages) => break (started, partial, messages),
                    _ => {}
                }
            }
        }).await.expect("it answers");
        let started = started.expect("its answer announced itself");
        assert_eq!(partial, Some(started), "what it streams is of the turn");
        assert_eq!(answer.iter().map(|m| m.turn_id).collect::<Vec<_>>(), [Some(started)], "and what it answers");
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert_eq!(state.history.messages().last().and_then(|m| m.turn_id), Some(started), "kept so in its history");
        running.abort();
    }

    #[tokio::test]
    async fn an_external_agent_asked_several_times_while_it_answers_is_called_once_after_it() {
        use crate::external::{ExternalEvent, Said};

        let answer = |id: &str, text: &str| vec![Ok(ExternalEvent::Started(task(id))), Ok(ExternalEvent::Message(text.to_string())), Ok(ExternalEvent::Completed)];
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            answer("t-1", "First."), answer("t-2", "Second."), answer("t-3", "Third."),
        ]).gated(Arc::clone(&gate)));
        let Stuck { channel, context, agent, mut events, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while remote.sent.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("the remote agent is asked about hello");

        // Three messages, each asking the agent, while the remote one works.
        let alice = alice_of(&context);
        let queued = ["one", "two", "three"];
        for text in queued {
            channel.user_actor(alice).unwrap().send_message(text.to_string(), None).await.expect("taken while it answers");
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut said = 0;
            while said < queued.len() {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == ActorId::User(alice) && matches!(envelope.event, ActorEvent::FullResponseGenerated(_)) {
                    said += 1;
                }
            }
        }).await.expect("the messages are said in the channel");
        gate.add_permits(3);

        let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
        assert_eq!(said(answered(&mut events, agent).await), ["First."]);
        assert_eq!(said(answered(&mut events, agent).await), ["Second."], "one call answers what was queued");

        // Nothing is left to answer: told something new, the agent asks the
        // remote one about it, not about something stale.
        channel.user_actor(alice).unwrap().send_message("four".to_string(), None).await.expect("sent");
        assert_eq!(said(answered(&mut events, agent).await), ["Third."]);
        let sent: Vec<Vec<Said>> = remote.sent.lock().unwrap().iter().map(|(_, message)| message.said.clone()).collect();
        let texts = |said: &[Said]| said.iter().map(|s| s.text.clone()).collect::<Vec<_>>();
        assert_eq!(sent.len(), 3, "one call per batch");
        assert_eq!(texts(&sent[1]), queued, "given all that was queued, at once");
        assert_eq!(texts(&sent[2]), ["four"]);
        running.abort();
    }

    #[tokio::test]
    async fn an_external_agent_that_fails_or_cannot_be_reached_fails_its_answer_and_goes_on() {
        use crate::external::{ExternalAgentError, ExternalEvent};

        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Started(task("t-1"))), Ok(ExternalEvent::Failed("out of coffee".to_string()))],
            vec![Err(ExternalAgentError::Connection { endpoint: "https://agent.example".to_string(), reason: "refused".to_string() })],
        ]));
        let Stuck { channel, context, agent, mut events, running, .. } = with_an_external_agent(remote).await;

        let failed = turn_failed(&mut events, agent).await;
        assert!(failed.contains("hermes could not answer: out of coffee"), "{failed}");

        let alice = channel.user_actor(alice_of(&context)).unwrap();
        alice.send_message("Still there?".to_string(), None).await.unwrap();
        let unreachable = turn_failed(&mut events, agent).await;
        assert!(unreachable.contains("cannot reach the external agent at https://agent.example"), "{unreachable}");
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert!(state.remote.open_tasks.is_empty(), "the failed task ended");
        running.abort();
    }

    #[tokio::test]
    async fn what_an_external_agent_could_not_send_is_sent_with_its_next_call() {
        use crate::external::ExternalEvent;

        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Message("Back again.".to_string())), Ok(ExternalEvent::Completed)],
        ]).unreachable_for(1));
        let Stuck { channel, context, agent, mut events, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;
        let unreachable = turn_failed(&mut events, agent).await;
        assert!(unreachable.contains("cannot reach the external agent"), "{unreachable}");

        let alice = channel.user_actor(alice_of(&context)).unwrap();
        alice.send_message("Still there?".to_string(), None).await.unwrap();
        answered(&mut events, agent).await;
        let sent = remote.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].1.said.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["hello", "Still there?"], "nothing lost, nothing twice");
        running.abort();
    }

    /// The components of an external personality with prompt parts (issue
    /// 204): a fixed one, and one that changes with what it hears.
    fn external_components_with_prompts() -> caelumora_model::component::ComponentConfig {
        let mut components = external_components();
        let mut frame = caelumora_model::component::ComponentDescriptor::new("frame", "static_system_prompt_provider");
        frame.params = serde_json::json!({ "prompt": "[Frame]\nBe brief.", "position": "before_turn_input" });
        components.component_nodes.push(frame);
        components.component_nodes.push(caelumora_model::component::ComponentDescriptor::new("heard", "test_heard"));
        components
    }

    #[tokio::test]
    async fn an_external_agent_is_given_its_prompt_parts_first_and_again_only_when_they_change() {
        use crate::external::ExternalEvent;

        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Message("Hello, alice.".to_string())), Ok(ExternalEvent::Completed)],
            vec![Ok(ExternalEvent::Message("Fine.".to_string())), Ok(ExternalEvent::Completed)],
        ]));
        let built = external_channel(Arc::clone(&remote), external_components_with_prompts(), |_| None).await.expect("the external agent can be run");
        let Stuck { channel, context, agent, mut events, running, .. } = start(built).await;
        let first = answered(&mut events, agent).await;
        channel.user_actor(alice_of(&context)).unwrap().send_message("How are you?".to_string(), None).await.unwrap();
        answered(&mut events, agent).await;

        let sent = remote.sent.lock().unwrap().clone();
        let prompts = &sent[0].1.prompts;
        assert_eq!(prompts, &["[Frame]\nBe brief.", "[Heard]\n1"], "first, in the order of the configuration, whatever their position");
        assert_eq!(sent[1].1.prompts, ["[Heard]\n3"], "again only what changed");

        // What it was given is the context of its answer (issue 205).
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        let seen = crate::view::AgentContextView::of(ActorId::Agent(agent), state);
        let items = seen.context_of(first[0].id).expect("recorded with the answer").items;
        let given_prompts: Vec<_> = items.iter().filter_map(|item| match item {
            caelumora_model::context::ContextItem::Prompt { text, .. } => Some(text.as_str()),
            _ => None,
        }).collect();
        assert_eq!(given_prompts, [prompts[0].as_str(), prompts[1].as_str()]);
        running.abort();
    }

    #[tokio::test]
    async fn after_a_restart_an_external_agent_is_given_only_the_prompt_parts_it_was_not_given() {
        use crate::external::ExternalEvent;
        use caelumora_model::external_agent::ConversationId;

        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Message("Hello again.".to_string())), Ok(ExternalEvent::Completed)],
        ]));
        // Before the restart it was given the frame, and how much it heard
        // then.
        let mut stored = caelumora_model::agent::AgentChannelState::default();
        stored.remote.converse_in(ConversationId("c-1".to_string()));
        stored.remote.give("frame".to_string(), caelumora_model::digest::prompt("[Frame]\nBe brief."));
        stored.remote.give("heard".to_string(), caelumora_model::digest::prompt("[Heard]\n0"));
        let built = external_channel(Arc::clone(&remote), external_components_with_prompts(), |_| Some(stored)).await.expect("the external agent can be run");
        let Stuck { agent, mut events, running, .. } = start(built).await;
        answered(&mut events, agent).await;

        let sent = remote.sent.lock().unwrap().clone();
        assert_eq!(sent[0].1.prompts, ["[Heard]\n1"], "what it has is not given again, what changed is");
        running.abort();
    }

    #[tokio::test]
    async fn the_prompt_parts_an_external_agent_could_not_send_are_sent_with_its_next_call() {
        use crate::external::ExternalEvent;

        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Message("Back again.".to_string())), Ok(ExternalEvent::Completed)],
        ]).unreachable_for(1));
        let built = external_channel(Arc::clone(&remote), external_components_with_prompts(), |_| None).await.expect("the external agent can be run");
        let Stuck { channel, context, agent, mut events, running, .. } = start(built).await;
        turn_failed(&mut events, agent).await;

        channel.user_actor(alice_of(&context)).unwrap().send_message("Still there?".to_string(), None).await.unwrap();
        answered(&mut events, agent).await;
        let sent = remote.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].1.prompts.len(), 2, "not lost with the send that failed");
        running.abort();
    }

    #[tokio::test]
    async fn after_a_restart_an_external_agent_sends_what_it_heard_but_had_not_sent() {
        use crate::external::ExternalEvent;
        use caelumora_model::agent::AgentChannelState;
        use caelumora_model::message::MessageRole;

        let said = |text: &str| Message { role: MessageRole::User, sender_name: "bob".to_string(), content: Some(text.to_string()), ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) };
        let mut stored = AgentChannelState::default();
        stored.perceive(vec![said("sent before")], vec![said("sent before")]);
        stored.transcript.start_anew();
        stored.perceive(vec![said("heard, then the crash")], vec![said("heard, then the crash")]);
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Message("Noted.".to_string())), Ok(ExternalEvent::Completed)],
        ]));
        let built = external_channel(Arc::clone(&remote), external_components(), |_| Some(stored)).await.unwrap();
        let Stuck { agent, mut events, running, .. } = start(built).await;

        answered(&mut events, agent).await;
        let sent = remote.sent.lock().unwrap().clone();
        assert_eq!(sent[0].1.said.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["heard, then the crash", "hello"]);
        running.abort();
    }

    #[tokio::test]
    async fn a_cancel_ends_the_answer_of_an_external_agent_and_its_remote_task() {
        use crate::external::ExternalEvent;

        // The remote agent takes the task on and then works on it for ever.
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Started(task("t-1"))), Ok(ExternalEvent::Partial("Let me think".to_string()))],
        ]));
        let Stuck { channel, context, agent, helper, mut events, running } = with_an_external_agent(Arc::clone(&remote)).await;
        settled(&helper, ActorState::Planning).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        channel.cancel(&context).await.expect("permitted");
        let ended = answered(&mut events, agent).await;
        assert_eq!(ended.first().and_then(|m| m.content.as_deref()), Some("Let me think"), "what came so far");
        settled(&helper, ActorState::Idle).await;
        assert_eq!(remote.cancelled.lock().unwrap().as_slice(), &[task("t-1")], "the remote task is cancelled too");
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert!(state.remote.open_tasks.is_empty());
        running.abort();
    }

    #[tokio::test]
    async fn an_external_agent_removed_while_it_answers_cancels_its_task_and_says_nothing() {
        use crate::external::ExternalEvent;

        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Started(task("t-1")))],
        ]));
        let Stuck { channel, context, agent, helper, mut events, running } = with_an_external_agent(Arc::clone(&remote)).await;
        settled(&helper, ActorState::Planning).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");
        tokio::time::timeout(Duration::from_secs(5), async {
            while remote.cancelled.lock().unwrap().is_empty() { tokio::time::sleep(Duration::from_millis(10)).await }
        }).await.expect("the remote task is cancelled");
        tokio::time::sleep(Duration::from_millis(50)).await;
        while let Ok(envelope) = events.try_recv() {
            assert!(
                !(envelope.context.sender_id == ActorId::Agent(agent) && matches!(envelope.event, ActorEvent::FullResponseGenerated(_) | ActorEvent::TurnFailed { .. })),
                "a stopped agent says nothing: {}", envelope.describe()
            );
        }
        running.abort();
    }

    /// What a deleted channel does with its actors before it aborts its
    /// task (`SessionLoop::delete_channel`, issue 187): an external agent
    /// has cancelled the remote task it followed by the time `stop_actors`
    /// returns, and every actor - the approver too - ended by itself.
    #[tokio::test]
    async fn a_stopped_channel_waits_for_its_external_agent_to_cancel_its_open_task() {
        use crate::external::ExternalEvent;

        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Started(task("t-1"))), Ok(ExternalEvent::InputRequired("Which city?".to_string()))],
        ]));
        let Stuck { channel, agent, mut events, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;
        answered(&mut events, agent).await;

        channel.stop_actors().await;
        assert_eq!(*remote.cancelled.lock().unwrap(), vec![task("t-1")], "the open task is cancelled before the channel is aborted");
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert!(state.remote.open_tasks.is_empty(), "and that is stored");
        tokio::time::timeout(Duration::from_secs(1), running).await
            .expect("every actor ended by itself, and with them the channel")
            .expect("the channel's task was not aborted")
            .expect("the channel ended without an error");
    }

    #[tokio::test]
    async fn an_external_agent_that_asked_back_is_answered_within_the_same_task() {
        use crate::external::ExternalEvent;
        use caelumora_model::external_agent::OpenTaskState;

        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Started(task("t-1"))), Ok(ExternalEvent::InputRequired("Which city?".to_string()))],
            vec![Ok(ExternalEvent::Message("Sunny in Bonn.".to_string())), Ok(ExternalEvent::Completed)],
        ]));
        let Stuck { channel, context, agent, mut events, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;

        let question = answered(&mut events, agent).await;
        assert_eq!(question[0].content.as_deref(), Some("Which city?"), "the question is the answer");
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert_eq!(state.remote.open_tasks.iter().map(|t| (t.task.clone(), t.state)).collect::<Vec<_>>(), vec![(task("t-1"), OpenTaskState::InputRequired)]);

        let alice = channel.user_actor(alice_of(&context)).unwrap();
        alice.send_message("Bonn".to_string(), None).await.unwrap();
        let answer = answered(&mut events, agent).await;
        assert_eq!(answer[0].content.as_deref(), Some("Sunny in Bonn."));
        assert_eq!(remote.sent.lock().unwrap()[1].1.answers, Some(task("t-1")), "the reply continues the task that asked");
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert!(state.remote.open_tasks.is_empty(), "and it ended");
        assert_eq!(given_for(&state, agent, &question[0]), ["hello"], "the question back came of the first call");
        assert_eq!(given_for(&state, agent, &answer[0]), ["Bonn"], "the answer of the call that answered it (issue 205)");
        running.abort();
    }

    #[tokio::test]
    async fn a_task_taken_up_again_is_done_as_the_call_that_took_it_up_though_the_remote_agent_does_not_tell_it() {
        use crate::external::{ExternalAgentError, ExternalEvent};

        // The answer to the question comes without the task told again,
        // and breaks off; re-attached, the task finishes (issue 213).
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Started(task("t-1"))), Ok(ExternalEvent::InputRequired("Which city?".to_string()))],
            vec![Ok(ExternalEvent::Partial("Sun".to_string())), Err(ExternalAgentError::Connection { endpoint: "https://agent.example".to_string(), reason: "reset".to_string() })],
        ]).resuming(vec![vec![Ok(ExternalEvent::Message("Sunny in Bonn.".to_string())), Ok(ExternalEvent::Completed)]]));
        let Stuck { channel, context, agent, mut events, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;
        answered(&mut events, agent).await;

        let alice = channel.user_actor(alice_of(&context)).unwrap();
        alice.send_message("Bonn".to_string(), None).await.unwrap();
        let (request, said) = tokio::time::timeout(Duration::from_secs(5), async {
            let mut answers = Vec::new();
            while answers.len() < 2 {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id != ActorId::Agent(agent) { continue }
                if let ActorEvent::FullResponseGenerated(messages) = envelope.event {
                    answers.push((envelope.context, messages));
                }
            }
            (answers.remove(0), answers.remove(0))
        }).await.expect("answered with nothing, and said unasked");
        assert!(request.1.is_empty(), "the answer broke off: the task goes on");
        assert_eq!(said.1[0].content.as_deref(), Some("Sunny in Bonn."));
        assert_eq!(said.0.span_id, request.0.span_id, "in the span of the request that took it up, not the first one");
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert_eq!(given_for(&state, agent, &said.1[0]), ["Bonn"], "with what the call that took it up was given");
        running.abort();
    }

    #[tokio::test]
    async fn an_external_personality_that_names_no_remote_agent_cannot_be_run() {
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::default());
        let refused = external_channel(remote, Default::default(), |_| None).await.expect_err("nothing to reach");
        let message = format!("{refused:#}");
        assert!(message.contains("no component provides ExternalAgent, and it has no default"), "{message}");
    }


    #[tokio::test]
    async fn an_open_task_whose_answer_broke_off_is_followed_and_its_end_said_unasked() {
        use crate::external::{ExternalAgentError, ExternalEvent};

        // The connection breaks while the remote agent works; re-attached,
        // it finishes.
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Started(task("t-1"))), Err(ExternalAgentError::Connection { endpoint: "https://agent.example".to_string(), reason: "reset".to_string() })],
        ]).resuming(vec![vec![Ok(ExternalEvent::Message("Here it is.".to_string())), Ok(ExternalEvent::Completed)]]));
        let Stuck { channel, agent, mut events, running, .. } = with_an_external_agent(remote).await;

        assert!(answered(&mut events, agent).await.is_empty(), "the request is answered with nothing: the task has not failed, it goes on (issue 195)");
        let mut woke = false;
        let said = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id != ActorId::Agent(agent) { continue }
                match envelope.event {
                    ActorEvent::WakeupRequested => woke = true,
                    ActorEvent::FullResponseGenerated(messages) => break messages,
                    _ => {}
                }
            }
        }).await.expect("said without being asked");
        assert!(!woke, "nobody is woken for it: sleeping agents hear it all the same (issue 199)");
        assert_eq!(said[0].content.as_deref(), Some("Here it is."));
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert!(state.remote.open_tasks.is_empty(), "the task ended");
        assert_eq!(state.history.messages().last().and_then(|m| m.content.clone()).as_deref(), Some("Here it is."), "kept as said");
        assert_eq!(given_for(&state, agent, &said[0]), ["hello"], "with what the call that began the task was given (issue 205)");
        running.abort();
    }

    #[tokio::test]
    async fn what_an_open_task_says_unasked_continues_the_answer_to_its_request() {
        use crate::external::{ExternalAgentError, ExternalEvent};

        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Started(task("t-1"))), Err(ExternalAgentError::Connection { endpoint: "https://agent.example".to_string(), reason: "reset".to_string() })],
        ]).resuming(vec![vec![Ok(ExternalEvent::Message("Here it is.".to_string())), Ok(ExternalEvent::Completed)]]));
        let Stuck { agent, mut events, running, .. } = with_an_external_agent(remote).await;

        // The answer to the request, with nothing, and later what the task
        // says unasked.
        let contexts = tokio::time::timeout(Duration::from_secs(5), async {
            let mut contexts = Vec::new();
            while contexts.len() < 2 {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == ActorId::Agent(agent) && matches!(envelope.event, ActorEvent::FullResponseGenerated(_)) {
                    contexts.push(envelope.context);
                }
            }
            contexts
        }).await.expect("answered, and said unasked");
        assert_eq!(contexts[1].span_id, contexts[0].span_id, "in the span of the request");
        assert_eq!(contexts[1].agent_hops, contexts[0].agent_hops, "as far from the last user's message as the answer (issue 45)");
        running.abort();
    }

    #[tokio::test]
    async fn an_answer_that_broke_off_is_said_once_in_full_when_its_task_ends() {
        use crate::external::{ExternalAgentError, ExternalEvent};

        let broken = || vec![
            Ok(ExternalEvent::Started(task("t-1"))),
            Ok(ExternalEvent::Partial("Here ".to_string())),
            Ok(ExternalEvent::Partial("it".to_string())),
            Err(ExternalAgentError::Connection { endpoint: "https://agent.example".to_string(), reason: "reset".to_string() }),
        ];
        // Re-attached, one remote agent gives the whole result again (as
        // A2A's GetTask does), the other only what is new.
        for (resumed, expected) in [
            (vec![Ok(ExternalEvent::Message("Here it is.".to_string())), Ok(ExternalEvent::Completed)], "Here it is."),
            (vec![Ok(ExternalEvent::Message("is.".to_string())), Ok(ExternalEvent::Completed)], "Here it\n\nis."),
        ] {
            let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![broken()]).resuming(vec![resumed]));
            let Stuck { channel, agent, mut events, running, .. } = with_an_external_agent(remote).await;

            assert!(answered(&mut events, agent).await.is_empty(), "nothing is said cut off");
            let said = answered(&mut events, agent).await;
            assert_eq!(said.len(), 1);
            assert_eq!(said[0].content.as_deref(), Some(expected), "said once, in full");
            let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
            let own: Vec<_> = state.history.messages().into_iter().filter(|m| m.sender_id == ActorId::Agent(agent)).filter_map(|m| m.content).collect();
            assert_eq!(own, vec![expected.to_string()], "and kept once");
            running.abort();
        }
    }

    #[tokio::test]
    async fn an_external_agent_answers_as_what_it_was_given_whatever_it_declares() {
        use crate::external::ExternalEvent;
        use caelumora_model::confidentiality::Classification;

        // It declares to bring in nothing that is not public; it was told
        // what is internal.
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Partial("He".to_string())), Ok(ExternalEvent::Message("Hello, alice.".to_string())), Ok(ExternalEvent::Completed)],
        ]).declaring(Classification::PUBLIC));
        let Stuck { mut events, agent, running, .. } = with_an_external_agent(remote).await;

        let mut partials = Vec::new();
        let answer = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id != ActorId::Agent(agent) {
                    continue;
                }
                match envelope.event {
                    ActorEvent::PartialMessageGenerated(m) => partials.push(m.classification),
                    ActorEvent::FullResponseGenerated(messages) => break messages,
                    _ => {}
                }
            }
        }).await.expect("the agent answers");
        assert_eq!(answer[0].classification, Classification::CHANNEL_DEFAULT, "what it was given, not what it declares");
        assert_eq!(partials, [Classification::CHANNEL_DEFAULT], "its pieces too");
        running.abort();
    }

    #[tokio::test]
    async fn a_task_is_stored_as_soon_as_it_is_begun() {
        use crate::external::ExternalEvent;
        use caelumora_model::external_agent::OpenTaskState;

        // The remote agent takes the request on, and is still at work.
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![vec![Ok(ExternalEvent::Started(task("t-1")))]]));
        let Stuck { channel, agent, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;

        let state = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
                if !state.remote.open_tasks.is_empty() { break state }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.expect("stored before the task ends, as a crash would find it");
        assert_eq!(state.remote.open_tasks.iter().map(|t| (t.task.clone(), t.state)).collect::<Vec<_>>(), vec![(task("t-1"), OpenTaskState::Working)]);
        assert!(state.transcript.messages().is_empty(), "what it was given for it is sent: a restart gives nothing twice");
        let work = state.remote.work_of(&task("t-1")).expect("with the work it is");
        let given = state.transcript.prefix(&work.context.transcript.expect("with what it was given for it")).unwrap();
        assert_eq!(given.iter().filter_map(|e| e.entry.message()?.content.clone()).collect::<Vec<_>>(), ["hello"]);
        let sent_as = remote.called_as.lock().unwrap()[0].1.clone();
        assert_eq!(work.security_context, sent_as, "the authority of the request it was sent with");
        assert_eq!(work.event_context.sender_id, ActorId::Agent(agent), "the request's envelope, continued in the agent's name");
        assert_eq!(work.classification, caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT, "bound by the channel as it began");
        running.abort();
    }

    #[tokio::test]
    async fn after_a_restart_an_open_task_is_followed_again_and_one_that_asked_back_waits() {
        use crate::external::ExternalEvent;
        use caelumora_model::agent::AgentChannelState;
        use caelumora_model::external_agent::{OpenTask, OpenTaskState, RemoteState};

        let stored = AgentChannelState {
            remote: RemoteState {
                open_tasks: vec![
                    OpenTask { task: task("t-working"), since: chrono::Utc::now(), state: OpenTaskState::Working, unsaid: None, work: None },
                    OpenTask { task: task("t-asking"), since: chrono::Utc::now(), state: OpenTaskState::InputRequired, unsaid: None, work: None },
                ],
                ..RemoteState::default()
            },
            ..AgentChannelState::default()
        };
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::default()
            .resuming(vec![vec![Ok(ExternalEvent::Message("Finished while you were away.".to_string())), Ok(ExternalEvent::Completed)]]));
        let (channel, _, agent, owner) = external_channel(Arc::clone(&remote), external_components(), |_| Some(stored)).await.unwrap();
        let mut events = channel.observe();
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

        let said = answered(&mut events, agent).await;
        assert_eq!(said[0].content.as_deref(), Some("Finished while you were away."), "what it did meanwhile, unasked");
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert_eq!(state.remote.open_tasks.iter().map(|t| t.task.clone()).collect::<Vec<_>>(), vec![task("t-asking")], "the one that asked back waits for the next request");
        assert!(remote.sent.lock().unwrap().is_empty(), "nothing was sent to it");
        let called_as: Vec<_> = remote.called_as.lock().unwrap().iter().map(|(call, security)| (*call, security.initiator_id)).collect();
        assert_eq!(called_as, [("resume", ActorId::User(owner))], "a task stored without its contexts is followed as the agent itself, on its owner's behalf");
        running.abort();
    }

    #[tokio::test]
    async fn an_open_task_is_followed_and_told_of_in_the_contexts_of_the_request_that_began_it() {
        use crate::external::ExternalEvent;
        use caelumora_model::agent::AgentChannelState;
        use caelumora_model::external_agent::{OpenTask, OpenTaskState, RemoteState};

        let bob = ActorId::User(UserId::new());
        // Bob's request, answered by the agent - continued in its name.
        let request = EventContext::new(SessionId::default(), ChannelId::default(), bob, "bob".to_string());
        let begun = request.clone();
        let stored = move |agent| { let hermes = ActorId::Agent(agent); Some(AgentChannelState {
            remote: RemoteState {
                open_tasks: vec![OpenTask {
                    task: task("t-1"),
                    since: chrono::Utc::now(),
                    state: OpenTaskState::Working,
                    unsaid: None,
                    work: Some(caelumora_model::work::Work::new(
                        begun.next(hermes, "hermes".to_string()),
                        SecurityContext::new(bob, SessionId::default(), ChannelId::default()).next(hermes),
                        caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT,
                    )),
                }],
                ..RemoteState::default()
            },
            ..AgentChannelState::default()
        }) };
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::default()
            .resuming(vec![vec![Ok(ExternalEvent::Message("Done for bob.".to_string())), Ok(ExternalEvent::Completed)]]));
        let (channel, _, agent, _) = external_channel(Arc::clone(&remote), external_components(), stored).await.unwrap();
        let mut events = channel.observe();
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

        let told = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel runs");
                if envelope.context.sender_id == ActorId::Agent(agent) && matches!(envelope.event, ActorEvent::CompleteMessageGenerated(_)) {
                    return envelope;
                }
            }
        }).await.expect("what came of the task is said");
        assert_eq!(told.security.initiator_id, bob, "said with the authority of bob's request");
        assert_eq!(told.context.span_id, request.span_id, "in its span");
        let called_as: Vec<_> = remote.called_as.lock().unwrap().iter().map(|(call, security)| (*call, security.initiator_id)).collect();
        assert_eq!(called_as, [("resume", bob)], "followed in bob's name, as it was begun");
        running.abort();
    }

    /// The state of external agent `agent` with one task `standing` so,
    /// begun while its channel was confidential - above the channel as it is
    /// now - in its own name.
    fn begun_above(agent: caelumora_model::agent::AgentId, open: &str, standing: caelumora_model::external_agent::OpenTaskState) -> caelumora_model::agent::AgentChannelState {
        use caelumora_model::agent::AgentChannelState;
        use caelumora_model::confidentiality::{Classification, Confidentiality};
        use caelumora_model::external_agent::{OpenTask, RemoteState};
        use caelumora_model::work::Work;
        let own = ActorId::Agent(agent);
        let work = Work::new(EventContext::default().next(own, "hermes".to_string()), SecurityContext::default().next(own), Classification::general(Confidentiality::Confidential));
        AgentChannelState {
            remote: RemoteState {
                open_tasks: vec![OpenTask { task: task(open), since: chrono::Utc::now(), state: standing, unsaid: None, work: Some(work) }],
                ..RemoteState::default()
            },
            ..AgentChannelState::default()
        }
    }

    #[tokio::test]
    async fn lowering_the_channel_gives_up_an_external_agents_answer_under_way() {
        use crate::external::ExternalEvent;
        use caelumora_model::confidentiality::{Classification, Confidentiality};

        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Started(task("t-1"))), Ok(ExternalEvent::Partial("Let me think".to_string()))],
        ]));
        let Stuck { channel, context, agent, helper, mut events, running } = with_an_external_agent(Arc::clone(&remote)).await;
        settled(&helper, ActorState::Planning).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        channel.reclassify(&context, Classification::general(Confidentiality::Public)).await.expect("permitted");
        let told = turn_failed(&mut events, agent).await;
        assert!(told.contains("lowered"), "{told}");
        settled(&helper, ActorState::Idle).await;
        assert_eq!(remote.cancelled.lock().unwrap().as_slice(), &[task("t-1")], "the remote task is cancelled");
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert!(state.remote.open_tasks.is_empty(), "and forgotten");
        assert!(state.history.messages().iter().all(|m| m.sender_id != ActorId::Agent(agent)), "nothing of the answer is kept");
        running.abort();
    }

    #[tokio::test]
    async fn a_task_begun_before_its_channel_was_lowered_is_given_up_instead_of_followed() {
        use caelumora_model::external_agent::OpenTaskState;

        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::default());
        let (channel, _, agent, _) = external_channel(Arc::clone(&remote), external_components(), |agent| Some(begun_above(agent, "t-1", OpenTaskState::Working))).await.unwrap();
        let mut events = channel.observe();
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

        let told = turn_failed(&mut events, agent).await;
        assert!(told.contains("lowered"), "{told}");
        let calls: Vec<&str> = remote.called_as.lock().unwrap().iter().map(|(call, _)| *call).collect();
        assert_eq!(calls, ["cancel"], "cancelled, not followed");
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert!(state.remote.open_tasks.is_empty(), "and forgotten");
        running.abort();
    }

    #[tokio::test]
    async fn a_task_that_asked_back_before_its_channel_was_lowered_is_not_taken_up() {
        use crate::external::ExternalEvent;
        use caelumora_model::external_agent::OpenTaskState;

        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Message("Hello.".to_string())), Ok(ExternalEvent::Completed)],
        ]));
        let built = external_channel(Arc::clone(&remote), external_components(), |agent| Some(begun_above(agent, "t-1", OpenTaskState::InputRequired))).await.unwrap();
        let Stuck { mut events, agent, running, .. } = start(built).await;

        answered(&mut events, agent).await;
        assert_eq!(remote.cancelled.lock().unwrap().as_slice(), &[task("t-1")], "the task that asked back is cancelled");
        assert_eq!(remote.sent.lock().unwrap()[0].1.answers, None, "and the request begins a new one");
        running.abort();
    }

    /// The state of an external agent with one task working, as a restart
    /// finds it.
    fn working_on(open: &str) -> caelumora_model::agent::AgentChannelState {
        use caelumora_model::agent::AgentChannelState;
        use caelumora_model::external_agent::{OpenTask, OpenTaskState, RemoteState};
        AgentChannelState {
            remote: RemoteState {
                open_tasks: vec![OpenTask { task: task(open), since: chrono::Utc::now(), state: OpenTaskState::Working, unsaid: None, work: None }],
                ..RemoteState::default()
            },
            ..AgentChannelState::default()
        }
    }

    /// Whether `agent` said anything - a message or an answer - among the
    /// events published so far.
    fn said_anything(events: &mut tokio::sync::broadcast::Receiver<Envelope>, agent: caelumora_model::agent::AgentId) -> bool {
        std::iter::from_fn(|| events.try_recv().ok()).any(|envelope| {
            envelope.context.sender_id == ActorId::Agent(agent)
                && matches!(envelope.event, ActorEvent::CompleteMessageGenerated(_) | ActorEvent::FullResponseGenerated(_))
        })
    }

    #[tokio::test]
    async fn a_followed_task_that_fails_says_so_and_keeps_nothing_of_what_came_before() {
        use crate::external::ExternalEvent;

        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::default()
            .resuming(vec![vec![Ok(ExternalEvent::Message("Half of it".to_string())), Ok(ExternalEvent::Failed("out of coffee".to_string()))]]));
        let (channel, _, agent, _) = external_channel(remote, external_components(), |_| Some(working_on("t-1"))).await.unwrap();
        let mut events = channel.observe();
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

        let failed = turn_failed(&mut events, agent).await;
        assert!(failed.contains("hermes could not finish: out of coffee"), "{failed}");
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert!(state.remote.open_tasks.is_empty(), "the failed task ended");
        assert!(state.history.messages().is_empty(), "nothing the channel never saw is kept as said");
        running.abort();
    }

    #[tokio::test]
    async fn a_followed_task_whose_stream_breaks_says_nothing_and_stays_open() {
        use crate::external::{ExternalAgentError, ExternalEvent};
        use caelumora_model::external_agent::OpenTaskState;

        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::default()
            .resuming(vec![vec![
                Ok(ExternalEvent::Message("Half of it".to_string())),
                Err(ExternalAgentError::Connection { endpoint: "https://agent.example".to_string(), reason: "reset".to_string() }),
            ]]));
        let (channel, _, agent, _) = external_channel(remote, external_components(), |_| Some(working_on("t-1"))).await.unwrap();
        let mut events = channel.observe();
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(!said_anything(&mut events, agent), "no half answer is published");
        let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert_eq!(state.remote.open_tasks.iter().map(|t| (t.task.clone(), t.state)).collect::<Vec<_>>(), vec![(task("t-1"), OpenTaskState::Working)], "left open, to be followed again");
        assert!(state.history.messages().is_empty(), "nothing is kept as said");
        running.abort();
    }
}
