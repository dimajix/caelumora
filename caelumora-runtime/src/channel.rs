use caelumora_model::name::Name;
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
use crate::transport_impl::{FilteringEventSender, InMemoryEventReceiver, InMemoryEventSender};
use anyhow::{anyhow, Result};
use arc_swap::ArcSwap;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_model::channel::{Channel, ChannelId, ChannelState, HistoryAccess, HistoryStart, Participant};
use crate::controller::{AgentController, ChannelController, UserController};
use caelumora_model::agent::{Personality, PersonalityKind};
use caelumora_model::approval::Approver;
use caelumora_model::event::GlobalEvent;
use caelumora_model::confidentiality::{user_roles, Classification};
use caelumora_model::mention::{handle_of, MentionDirectory};
use caelumora_model::message::{Message, MessageId};
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, Reclassified, SecurityAttributes, SecurityContext};
use caelumora_model::session::SessionId;
use caelumora_model::store::{EntityStore, StateStore, StoreError};
use caelumora_model::user::UserId;
use tracing::{debug, info, trace, warn, Instrument};
use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use crate::session::SessionLoop;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::mpsc::Sender;
use tokio::sync::{mpsc, Mutex};
use tokio::task::{AbortHandle, JoinSet};

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
struct ActorHandle {
    id: ActorId,
    abort_handle: AbortHandle,
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
    /// A participant not cleared for the channel's classification - to
    /// join it, or for the classification it would be raised to (issue
    /// 133).
    #[error("{participant} is not cleared for channel {channel} as it is, or would be, classified")]
    NotCleared { participant: ActorId, channel: ChannelId },
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
    /// its personality, read once, as the agent may read it, for its actor
    /// to be built from - or `None` if its user or agent no longer exists.
    ///
    /// # Errors
    /// Whatever else the store reports when loading it - also that an
    /// agent's personality is gone or not its to read: the agent exists, but
    /// cannot be run.
    async fn of(participant: &Participant, channel: &Channel, runtime: &RuntimeManager) -> Result<Option<Self>> {
        let controllers = runtime.controller_manager();
        let agent = match participant {
            Participant::User(id) => return gone_is_none(controllers.user_controller(*id).await.map(Member::User)),
            Participant::Approver(approver) => return Ok(Some(Member::Approver(approver.clone()))),
            Participant::Agent(id) => match gone_is_none(controllers.agent_controller(*id).await)? {
                Some(agent) => agent,
                None => return Ok(None),
            },
        };
        let (security, id) = { let loaded = agent.load(); (loaded.security_context(channel.session_id, channel.id), loaded.personality_id) };
        let personality = runtime.entity_store().get_personality(&security, id).await?;
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

    /// Nothing if the member is cleared for channel `channel`, classified
    /// `classification` (`JoinChannel`), by every set of roles that binds
    /// it - to join it, to take part in it when it is created, or for what
    /// it would be reclassified to.
    ///
    /// # Errors
    /// `ChannelError::NotCleared` if it is not; whatever reading its roles
    /// reports.
    async fn require_cleared(&self, runtime: &RuntimeManager, context: &SecurityContext, classification: &Classification, channel: ChannelId) -> Result<()> {
        let participant = self.actor_id();
        for roles in self.clearances(runtime.entity_store().as_ref()).await? {
            let action = ActionType::JoinChannel { participant, roles: &roles, channel: classification };
            runtime.security_manager().require(context, action, || ChannelError::NotCleared { participant, channel }).await?;
        }
        Ok(())
    }
}

/// Nothing if every participant of `channel` that still exists is cleared
/// for its classification (`Member::require_cleared`) - asked before
/// anything of a new channel or session is stored.
///
/// # Errors
/// `ChannelError::NotCleared` for the first who is not; whatever reading a
/// participant reports.
pub(crate) async fn require_cleared(runtime: &RuntimeManager, context: &SecurityContext, channel: &Channel) -> Result<()> {
    for participant in &channel.participants {
        if let Some(member) = Member::of(participant, channel, runtime).await? {
            member.require_cleared(runtime, context, &channel.classification, channel.id).await?;
        }
    }
    Ok(())
}

/// The actors of a channel's participants, each with what it was built
/// from.
type Actors = HashMap<ActorId, (Arc<dyn Actor>, Member)>;

/// What was loaded, or `None` if it no longer exists.
fn gone_is_none<T>(found: Result<T, StoreError>) -> Result<Option<T>> {
    match found {
        Ok(found) => Ok(Some(found)),
        Err(StoreError::NotFound { .. }) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// How long a stopped actor gets to end its task by itself before it is
/// aborted (issue 75): a tool that hangs is the case this is for.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// A change to the actors a running channel runs (issue 46), carried out by
/// `run`, which owns their tasks.
enum ActorChange {
    Start(Arc<dyn Actor>),
    /// The actor to stop, and on whose behalf it was removed - in whose name
    /// `run` announces it (`ParticipantRemoved`, issue 162).
    Stop(ActorId, SecurityContext),
}

#[derive(Debug)]
pub struct ChannelLoop {
    id: ChannelId,
    session_id: SessionId,
    channel: Arc<ChannelController>,
    runtime: ChannelRuntime,
    /// The actors of the participants, each with what it was built from -
    /// what a reclassification asks its clearances of; replaced as a whole
    /// when someone joins or leaves (issue 46), so that every reader sees
    /// one set.
    actors: ArcSwap<Actors>,
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
    event_tx: broadcast::Sender<Envelope>,
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
        let state_store = runtime.state_store();
        // Its components (issue 174) act on behalf of the channel's owner,
        // as an agent's act on behalf of its owner.
        let security_context = SecurityContext::new(channel.security_attributes.owner_id, session_id, id);
        let components = ComponentContainer::from_config(
            &channel.components,
            &runtime.component_registry(),
            &ComponentContext::new(security_context, runtime.security_manager(), runtime.secret_manager()),
        )
        .await
        .map_err(SessionError::InvalidModerator)?;
        let moderator = components.one::<dyn Moderator>().await.map_err(SessionError::InvalidModerator)?;
        
        let state = match state_store.get_channel_state(id).await {
            Ok(s) => s,
            Err(StoreError::NotFound { entity: _, id: _ }) => {
                let state = ChannelState::new(id);
                state_store.put_channel_state(id, &state).await?;
                state
            }
            Err(e) => return Err(e.into()),
        };
        let state = Mutex::new(state);

        let recorder = runtime.new_recorder()?;
        let (event_tx, _) = broadcast::channel(100);

        let channel_controller = runtime.controller_manager().new_channel_controller(channel);

        let (changes_tx, changes_rx) = mpsc::unbounded_channel();
        let channel_loop = Arc::new(Self {
            id,
            session_id,
            channel: channel_controller,
            runtime,
            actors: ArcSwap::from_pointee(HashMap::new()),
            moderator,
            mentions: ArcSwap::from_pointee(MentionDirectory::default()),
            changes_tx,
            changes_rx: Mutex::new(changes_rx),
            recorder: Mutex::new(recorder),
            state,
            state_store,
            quiet: AtomicBool::new(false),
            event_tx,
            max_route_decisions: 100,
            run_lock: Mutex::new(()),
            session: OnceLock::new(),
        });
        channel_loop.runtime.attach(&channel_loop);
        // The participants the channel has; `run` starts their tasks.
        let participants = channel_loop.channel.load().participants.clone();
        for participant in &participants {
            // A user or agent deleted since it joined: the channel runs
            // without it rather than not at all (issue 120).
            let Some(member) = Member::of(participant, &channel_loop.channel.load(), channel_loop.runtime.runtime_manager()).await? else {
                warn!("{} of channel {} no longer exists and is left out", participant.actor_id(), channel_loop.id);
                continue;
            };
            let actor = Self::create_actor(channel_loop.channel.clone(), &member, &channel_loop.runtime).await?;
            channel_loop.adopt(Arc::from(actor), member).await;
        }
        Ok(channel_loop)
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
    async fn adopt(&self, actor: Arc<dyn Actor>, member: Member) {
        self.moderator.add_actor(actor.clone());
        let id = actor.id();
        self.actors.rcu(|actors| {
            let mut actors = HashMap::clone(actors);
            actors.insert(id, (actor.clone(), member.clone()));
            actors
        });
        self.rebuild_mentions();
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.event_tx.subscribe()
    }

    /// The whole history, whoever may see it; outside the channel, what a
    /// participant sees is `visible_history`.
    pub(crate) async fn history(&self) -> Vec<Message> {
        self.state.lock().await.history.messages()
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
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// channel, or whatever else storing it reports.
    pub async fn update_name(&self, context: &SecurityContext, new_name: Name) -> Result<()> {
        Ok(self.channel.update_name(context, new_name).await?)
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
        for (_, member) in self.actors.load().values() {
            member.require_cleared(self.runtime.runtime_manager(), context, &classification, self.id).await?;
        }
        self.channel.update_classification(context, classification.clone()).await?;
        let lowered = !self.runtime.security_manager().may_publish(context, context.caller_id, &channel, &classification).await;
        if lowered {
            self.cancel_actors();
        }
        let mut state = self.state.lock().await;
        let fitted = state.history.fit_into(&classification);
        if fitted > 0 {
            self.state_store.put_channel_state(self.id, &state).await?;
        }
        info!(channel = %self.id, %classification, fitted, lowered, by = %context.caller_id, "Channel reclassified");
        Ok(())
    }

    /// Reclassifies message `message` as `classification` on behalf of
    /// `context` (issue 133): asked as `ActionType::Reclassify` - its author
    /// or the channel's owner, never above the channel - then changed in the
    /// history, stored and announced (`GlobalEvent::MessageReclassified`).
    ///
    /// # Errors
    /// `ChannelError::MessageNotFound` if the history has no such message,
    /// `ChannelError::Forbidden` if `context` may not reclassify it as
    /// asked; whatever storing it reports.
    pub async fn reclassify_message(&self, context: &SecurityContext, message: MessageId, classification: Classification) -> Result<()> {
        let channel = self.classification();
        let attributes = self.security_attributes();
        let mut state = self.state.lock().await;
        let not_found = || ChannelError::MessageNotFound { message, channel: self.id };
        let (author, current) = state.history.all_entries()
            .find(|e| e.message.id == message)
            .map(|e| (e.message.sender_id, e.message.classification.clone()))
            .ok_or_else(not_found)?;
        let action = ActionType::Reclassify {
            target: Reclassified::Message { author, attributes: &attributes },
            current: &current,
            new: &classification,
            channel: &channel,
        };
        self.require(context, action, "reclassifying a message of").await?;
        state.history.reclassify(message, classification.clone());
        self.state_store.put_channel_state(self.id, &state).await?;
        drop(state);
        self.runtime.event_bus().publish(GlobalEvent::MessageReclassified(self.session_id, self.id, message, classification), attributes);
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
    ///
    /// # Errors
    /// `ChannelError::NotInSession`, `AlreadyParticipant` or `Forbidden`; a
    /// `StoreError` if `context` may not change the channel.
    pub async fn add_participant(&self, context: &SecurityContext, participant: ActorId, history: HistoryAccess) -> Result<()> {
        if self.actors.load().contains_key(&participant) {
            return Err(ChannelError::AlreadyParticipant { participant, channel: self.id }.into());
        }
        let found = self.session_loop()
            .and_then(|s| s.session().participants.iter().find(|p| p.actor_id() == participant).cloned())
            .ok_or(ChannelError::NotInSession { participant, channel: self.id })?;
        let member = Member::of(&found, &self.channel.load(), self.runtime.runtime_manager()).await?
            .ok_or(ChannelError::ParticipantGone { participant, channel: self.id })?;
        member.require_cleared(self.runtime.runtime_manager(), context, &self.classification(), self.id).await?;

        let messages = self.history().await;
        let start = history_start(history, &messages, chrono::Utc::now());
        self.channel.add_participant(context, found.clone(), start).await?;

        let started = async {
            let actor = Self::create_actor(self.channel.clone(), &member, &self.runtime).await?;
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
        self.adopt(actor.clone(), member).await;
        let _ = self.changes_tx.send(ActorChange::Start(actor));
        info!("{} joined channel {}", participant, self.id);
        Ok(())
    }

    /// Removes `participant` from the channel on behalf of `context` (issue
    /// 46): stored first (`UpdateChannel`), then its actor stopped - an agent
    /// at once, without finishing a turn; the approver discards its pending
    /// approvals. The leave is announced by the `ChannelController` on the
    /// event bus (issue 73), and in the channel by `run` with a
    /// `ParticipantRemoved` in the name of `context`'s caller, after which
    /// nothing of the participant enters the channel any more (issue 162).
    /// Its state is kept, for the record and for when it joins again.
    ///
    /// # Errors
    /// `ChannelError::ApproverStays` or `NotAParticipant`; a `StoreError` if
    /// `context` may not change the channel.
    pub async fn remove_participant(&self, context: &SecurityContext, participant: ActorId) -> Result<()> {
        if matches!(participant, ActorId::Approver(_)) {
            return Err(ChannelError::ApproverStays { channel: self.id }.into());
        }
        let Some(actor) = self.actor_by_id(participant) else {
            return Err(ChannelError::NotAParticipant { participant, channel: self.id }.into());
        };
        self.channel.remove_participant(context, participant).await?;
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
        for (actor, _) in self.actors.load().values() {
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
            .map(|(a, _)| a)
            .filter(|a| a.state() == ActorState::Sleeping)
            .map(|a| a.name())
            .collect();
        self.wake_for_delivery();
        Ok(asleep)
    }

    /// Wakes one actor deliberately, like `wakeup` and under the same
    /// permission. Returns whether it was asleep.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `SecurityManager` refuses
    /// `WakeChannel`; an error if `actor` is not in this channel.
    pub async fn wakeup_actor(&self, context: &SecurityContext, actor: ActorId) -> Result<bool> {
        let channel = self.channel.load();
        self.require(context, ActionType::WakeChannel(&channel.security_attributes), "waking the agents of").await?;
        let actor = self.actor_by_id(actor)
            .ok_or_else(|| anyhow!("actor {} is not in channel {}", actor, self.id))?;
        let was_asleep = actor.state() == ActorState::Sleeping;
        self.quiet.store(false, Ordering::SeqCst);
        actor.wakeup();
        Ok(was_asleep)
    }

    /// Tells every actor of the channel to stop for good (`Actor::stop`) -
    /// for a channel that is deleted: nothing of it is resumed, and whoever
    /// follows it through their actor is told it ended, whether or not the
    /// actors' tasks ever ran (issue 104).
    pub(crate) fn stop_actors(&self) {
        for (actor, _) in self.actors.load().values() {
            actor.stop();
        }
    }

    /// Wakes the channel's actors because something is on its way to them:
    /// a `WakeupRequested` - which a user's actor sends with every message -
    /// or a mention. That is the consequence of an action already permitted,
    /// so it carries no permission of its own - otherwise a user allowed to
    /// write but not to wake could not write.
    fn wake_for_delivery(&self) {
        self.quiet.store(false, Ordering::SeqCst);
        for (actor, _) in self.actors.load().values() {
            actor.wakeup();
        }
    }

    /// `action`, refused as a `ChannelError::Forbidden` that names what was
    /// refused, not why.
    async fn require(&self, context: &SecurityContext, action: ActionType<'_>, what: &'static str) -> Result<()> {
        let channel = self.id;
        Ok(self.runtime.security_manager().require(context, action, || ChannelError::Forbidden { action: what, channel }).await?)
    }

    pub fn actor_ids(&self) -> Vec<ActorId> {
        self.actors.load().keys().copied().collect()
    }

    /// All participants of the channel, in no particular order.
    pub fn members(&self) -> Vec<ChannelMember> {
        self.actors.load().values()
            .map(|(a, _)| {
                let name = a.name();
                let handle = Some(handle_of(&name))
                    .filter(|h| Self::is_mentionable(a.id()) && !h.is_empty());
                ChannelMember { id: a.id(), name, handle, kind: a.kind() }
            })
            .collect()
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
            .map(|(a, _)| a)
            .filter(|a| Self::is_mentionable(a.id()))
            .map(|a| (a.id(), a.name()))
            .collect();
        MentionDirectory::new(names.iter().map(|(id, name)| (*id, name.as_str())))
    }

    pub fn actor_by_id(&self, actor_id: ActorId) -> Option<Arc<dyn Actor>> {
        self.actors.load().get(&actor_id).map(|(actor, _)| actor.clone())
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
            Member::User(user) => Box::new(UserActor::new(&channel.load(), user, actor_runtime).await?),
            Member::Agent(agent, personality) => Box::new(AgentActor::new(&channel.load(), agent, personality, actor_runtime).await?),
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
    async fn apply_change(&self, change: ActorChange, running: &mut HashMap<ActorId, ActorHandle>, event_tx: &Sender<Envelope>, join_set: &mut JoinSet<ActorId>) {
        match change {
            ActorChange::Start(actor) => {
                // Joined before `run` started: already running.
                if !running.contains_key(&actor.id()) {
                    let handle = self.spawn_actor(actor, event_tx.clone(), join_set);
                    running.insert(handle.id, handle);
                }
            }
            ActorChange::Stop(id, remover) => {
                // Dropping the handle closes the actor's inbox; told to stop
                // (`Actor::stop`), it ends its task itself once its state is
                // stored consistent. Only if it does not - a tool that hangs -
                // is it aborted.
                if let Some(handle) = running.remove(&id) {
                    let abort_handle = handle.abort_handle.clone();
                    let (channel, actor) = (self.id, id);
                    tokio::spawn(async move {
                        tokio::time::sleep(STOP_GRACE).await;
                        if !abort_handle.is_finished() {
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
                let name = self.runtime.controller_manager().actor_name(remover.caller_id).await.unwrap_or_else(|e| {
                    warn!("Cannot name {} for channel {}: {}", remover.caller_id, self.id, e);
                    remover.caller_id.to_string()
                });
                let sender = EventContext::new(self.session_id, self.id, remover.caller_id, name);
                let removed = Envelope::new(remover, sender, ActorEvent::ParticipantRemoved { participant: id });
                self.process_envelope(removed, running).await;
            }
        }
    }

    async fn handle_actor_event(&self, maybe_event: Option<Envelope>, running: &HashMap<ActorId,ActorHandle>) -> bool {
        if let Some(event) = maybe_event {
            // An actor that was removed may still finish the step it was in;
            // what it sends then no longer enters the channel (issue 162).
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

    async fn process_envelope(&self, event: Envelope, running: &HashMap<ActorId,ActorHandle>) {
        match event.event {
            // One per streamed chunk: far finer than per message.
            ActorEvent::PartialMessageGenerated(_) | ActorEvent::IntermediatePartialGenerated(_) => {
                trace!("Received {}", event.describe())
            }
            _ => debug!("Received {}", event.describe()),
        }
        // The full envelope carries message content: trace level only.
        trace!("Envelope: {:?}", event);
        if !self.admits(&event).await {
            return;
        }
        let event = self.resolve_mentions(event);
        self.record_receive_event(&event).await;
        self.append_history(&event).await;
        self.route_event(running, event).await;
    }

    /// Starts the tasks of the channel's actors as `run` begins.
    fn spawn_actors(&self, actor_output:Sender<Envelope>, join_set: &mut JoinSet<ActorId>) -> HashMap<ActorId,ActorHandle> {
        self.actors.load().iter()
            .map(|(id, (actor, _))| (*id, self.spawn_actor(actor.clone(), actor_output.clone(), join_set)))
            .collect()
    }

    fn spawn_actor(&self, actor: Arc<dyn Actor>, actor_output: Sender<Envelope>, join_set: &mut JoinSet<ActorId>) -> ActorHandle {
        let id = actor.id();
        let filter = actor.event_filter();
        let (tx, rx) = mpsc::channel(100);

        let actor_rx = Box::new(InMemoryEventReceiver::new(rx));
        let actor_tx = Box::new(InMemoryEventSender::new(actor_output));

        let abort_handle = join_set.spawn(async move {
            let result = actor.run(actor_rx, actor_tx).await;
            if let Err(e) = result {
                tracing::error!("Actor {} stopped with an error: {:#}", actor.id(), e)
            }
            id
        });

        // Wrap the actor's input sender with its declared filter predicate.
        let filtered_sender = Box::new(FilteringEventSender::new(
            Box::new(InMemoryEventSender::new(tx)),
            filter,
        ));

        ActorHandle { id, abort_handle, sender: filtered_sender }
    }

    /// Whether every message `event` carries may enter the channel as its
    /// sender classified it, and every tool result in them as it is
    /// classified (`AccessQuestions::may_publish`, issue 133): the gate for every
    /// actor, whatever it checked itself. An event with a part that may not
    /// is dropped whole, and logged: its sender misclassified it.
    async fn admits(&self, event: &Envelope) -> bool {
        let messages: Vec<&Message> = match &event.event {
            ActorEvent::PartialMessageGenerated(m)
            | ActorEvent::CompleteMessageGenerated(m)
            | ActorEvent::IntermediatePartialGenerated(m)
            | ActorEvent::IntermediateMessageGenerated(m) => vec![m],
            ActorEvent::FullResponseGenerated(messages) => messages.iter().collect(),
            _ => return true,
        };
        let channel = self.classification();
        let security = self.runtime.security_manager();
        for message in messages {
            let parts = std::iter::once(&message.classification).chain(message.tool_results.iter().map(|r| &r.classification));
            for classification in parts {
                if !security.may_publish(&event.security, event.context.sender_id, classification, &channel).await {
                    warn!("Dropped {}: a part of its message {} is classified {}, which channel {} does not admit", event.describe(), message.id, classification, self.id);
                    return false;
                }
            }
        }
        true
    }

    /// Resolves the `@`-mentions in every complete message carried by `event`
    /// and stores them in `Message::mentions`. Done here, once, as the message
    /// enters the channel, so that every frontend, the history and the
    /// moderator all see the same resolution (issue 29).
    fn resolve_mentions(&self, mut event: Envelope) -> Envelope {
        let resolve = |m: &mut Message| {
            if let Some(content) = &m.content {
                m.mentions = self.mentions.load().resolve(content);
            }
        };
        match &mut event.event {
            ActorEvent::CompleteMessageGenerated(m) | ActorEvent::IntermediateMessageGenerated(m) => resolve(m),
            ActorEvent::FullResponseGenerated(msgs) => msgs.iter_mut().for_each(resolve),
            _ => {}
        }
        event
    }

    /// Wakes every sleeping agent a user explicitly mentioned, so that the
    /// moderator's `ResponseRequested` reaches an agent that acts on it.
    /// Mentions in agent responses wake nobody, matching the moderator, which
    /// does not route them either.
    async fn wake_mentioned(&self, event: &Envelope) {
        if !matches!(event.context.sender_id, ActorId::User(_)) {
            return;
        }
        if let ActorEvent::FullResponseGenerated(messages) = &event.event {
            for id in messages.iter().flat_map(|m| m.mentions.iter()) {
                if let Some(actor) = self.actor_by_id(*id) {
                    if matches!(id, ActorId::Agent(_)) && actor.state() == ActorState::Sleeping {
                        actor.wakeup();
                    }
                }
            }
        }
    }

    async fn route_event(&self, running: &HashMap<ActorId,ActorHandle>, event: Envelope) {
        // Asked for by an actor - a user's actor asks with every message it
        // sends (issue 90); the channel does not decide by itself.
        if matches!(event.event, ActorEvent::WakeupRequested) {
            self.wake_for_delivery();
        }
        self.wake_mentioned(&event).await;

        let broadcast = self.route_event_impl(running, &event).await;

        if broadcast {
            let _ = self.event_tx.send(event);
        }
    }

    async fn route_event_impl(&self, running: &HashMap<ActorId,ActorHandle>, event: &Envelope) -> bool {
        if event.context.span_depth >= self.max_route_decisions {
            warn!("Too many routings, stopping further propagation");
            return true
        }

        let span_id = event.context.span_id;
        let span_depth = event.context.span_depth + 1;
        let decision = self.moderator.route(event).await;
        let quiet = self.quiet.load(Ordering::SeqCst);

        // First, route all messages to all agents...
        for route in decision.forwards {
            let target_id = route.0;

            // Do not propagate message to sender again
            if target_id == event.context.sender_id {
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

        decision.broadcast
    }

    async fn append_history(&self, envelope: &Envelope) {
        let mut state = self.state.lock().await;
        let mut changed = false;
        match &envelope.event {
            ActorEvent::FullResponseGenerated(msgs) => {
                state.history.extend(msgs.clone());
                changed = true;
            }
            // Handle other event types as needed
            _ => {}
        }

        if changed {
            if let Err(e) = self.state_store.put_channel_state(self.id, &state).await {
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
    use caelumora_model::name::{Name, ScopedName};
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
            username: username.to_string(),
            email: String::new(),
            roles: Vec::new(),
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
        let registry = crate::component::ComponentRegistry::built_in(store.clone());
        channel_built(store, participants, security, registry, caelumora_model::channel::moderation_components(None, Some("broadcast_policy"))).await
    }

    /// A channel of `participants` with `components`, built by `registry`.
    async fn channel_built(store: Arc<MemoryStore>, participants: Vec<Participant>, security: Arc<dyn SecurityManager>, registry: crate::component::ComponentRegistry, components: caelumora_model::component::ComponentConfig) -> Arc<ChannelLoop> {
        let secrets = Arc::new(SecretManager::new(vec![], security.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store.clone(), secrets, security, default_commands(), Default::default(), registry).unwrap());
        let channel = Channel {
            id: ChannelId::new(),
            session_id: SessionId::new(),
            name: Name::new("main").unwrap(),
            components,
            participants,
            security_attributes: SecurityAttributes::default(),
            classification: caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT,
            history_limits: Vec::new(),
        };
        let builtin = |id: &str, facet: Arc<dyn crate::command::CommandProvider>| ComponentImpl { id: id.to_string(), fail_policy: ComponentFailPolicy::FailOpen, facet };
        let executor = CommandExecutor::new(
            runtime.security_manager(),
            vec![builtin("help", Arc::new(HelpCommandProvider)), builtin("wakeup", Arc::new(WakeupCommandProvider))],
        ).unwrap();
        ChannelLoop::new(channel, ChannelRuntime::new(runtime, Arc::new(executor))).await.unwrap()
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
            assert_eq!(actor.name(), participant.username, "the actor writes in this user's name");
        }
        assert!(channel.user_actor(UserId::new()).is_none(), "someone who is not a participant has no actor");
    }

    #[tokio::test]
    async fn cancelling_and_waking_deliberately_ask_the_security_manager() {
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let refused = channel_with(&[&alice], Arc::new(DenyAll)).await;
        for error in [
            refused.cancel(&context).await.expect_err("cancelling is refused"),
            refused.wakeup(&context).await.expect_err("waking is refused"),
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
        let mut events = channel.subscribe();
        let running = tokio::spawn({ let channel = channel.clone(); async move { channel.run().await } });

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
        let mut events = channel.subscribe();
        let running = tokio::spawn({ let channel = channel.clone(); async move { channel.run().await } });

        let reply = actor.execute_command("wakeup", "").await.expect("/wakeup runs");
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
        async fn route(&self, event: &Envelope) -> crate::moderator::RouteDecision {
            vec![(self.0, event.clone()), (self.0, event.forward(ActorEvent::ResponseRequested))].into()
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
        let mut registry = crate::component::ComponentRegistry::built_in(store.clone());
        registry.register(ComponentKind::new("test_moderator").provides::<dyn crate::moderator::Moderator>(), move |_, _| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn crate::moderator::Moderator>(moderator.clone());
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
        let actors = HashMap::from([(bob, ActorHandle { id: bob, abort_handle, sender: Box::new(InMemoryEventSender::new(tx)) })]);
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
        let call = caelumora_model::tool::ToolCall { id: None, function_name: "f".to_string(), arguments: serde_json::json!({}), visibility: Default::default() };
        let result = caelumora_model::tool::ToolCallResult::of(&call, Default::default(), "secret".to_string(), Classification::general(Confidentiality::Confidential));
        let mut with_result = from(alice_id, alice_id, internal);
        if let ActorEvent::FullResponseGenerated(messages) = &mut with_result.event {
            messages[0].tool_results.push(result);
        }
        channel.process_envelope(with_result, &actors).await;
        assert_eq!(channel.history().await.len(), 1, "nor with a result above the channel");
    }

    #[tokio::test]
    async fn a_stopped_channel_refuses_what_a_user_sends() {
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(DummySecurityManager::new())).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let mut said = channel.subscribe();
        let running = tokio::spawn({ let channel = channel.clone(); async move { channel.run().await } });
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
        async fn chat_completion(&self, _: &[Message], _: &[caelumora_model::tool::ToolDefinition])
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
        use caelumora_model::agent::{Agent, Personality, PersonalityId};
        use caelumora_model::scope::Scope;
        use caelumora_model::store::EntityStore;

        let store = Arc::new(MemoryStore::new());
        let personality = Personality {
            description: String::new(),
            introduction: None,
            reasoning_visibility: caelumora_model::confidentiality::Visibility::Channel,
            id: PersonalityId::new(),
            name: ScopedName::new("plain", Default::default()).unwrap(),
            kind: Default::default(),
            roles: None,
            components,
            security_attributes: Default::default(),
        };
        store.create_personality(&SecurityContext::system(), &personality).await.unwrap();
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let agent = Agent::new(ScopedName::new("helper", Scope::GlobalScope).unwrap(), &personality, context.initiator_id);
        let agent_id = agent.id;
        store.create_user(&SecurityContext::system(), &alice).await.unwrap();
        store.create_agent(&SecurityContext::system(), &agent).await.unwrap();

        let secrets = Arc::new(SecretManager::new(vec![], security.clone()));
        // The model is a component (issue 130): the stand-in is registered as
        // a kind of its own and made every agent's default.
        let mut registry = crate::component::ComponentRegistry::built_in(store.clone());
        registry.register(
            crate::component::ComponentKind::new("test_model").provides::<dyn crate::llm::LLMClient>(),
            move |_descriptor, _context| {
                let mut facets = crate::component::ComponentFacets::empty();
                facets.insert::<dyn crate::llm::LLMClient>(model.clone());
                Ok(facets)
            },
        );
        register_no_secrets(&mut registry);
        let defaults = caelumora_model::component::ComponentConfig {
            component_nodes: vec![caelumora_model::component::ComponentDescriptor {
                id: "model".to_string(),
                kind: "test_model".to_string(),
                params: serde_json::Value::Null,
                tags: Vec::new(),
                fail_policy: Default::default(),
                security_attributes: Default::default(),
            }],
        };
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store, secrets, security, default_commands(), defaults, registry).unwrap());
        let channel = Channel {
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
            history_limits: Vec::new(),
        };
        // Stored, so that leaving it can be stored too.
        runtime.entity_store().create_channel(&SecurityContext::system(), &channel).await.unwrap();
        let executor = CommandExecutor::empty(runtime.security_manager());
        let channel = ChannelLoop::new(channel, ChannelRuntime::new(runtime, Arc::new(executor))).await.unwrap();
        let events = channel.subscribe();
        let running = tokio::spawn({ let channel = channel.clone(); async move { channel.run().await } });

        // The message makes the agent start a turn.
        channel.user_actor(alice.id).unwrap().send_message("hello".to_string(), None).await.unwrap();
        let helper = channel.actor_by_id(ActorId::Agent(agent_id)).unwrap();
        Stuck { channel, context, agent: agent_id, helper, events, running }
    }

    /// A model that gives the answers it was given, one per call, and then
    /// none.
    #[derive(Debug)]
    struct Scripted(std::sync::Mutex<std::collections::VecDeque<Message>>);

    #[async_trait::async_trait]
    impl crate::llm::LLMClient for Scripted {
        async fn chat_completion(&self, _: &[Message], _: &[caelumora_model::tool::ToolDefinition])
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

    #[tokio::test]
    async fn an_agent_that_calls_stop_ends_its_turn_with_the_call_and_sleeps() {
        use caelumora_model::approval::ApprovalResponse;
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        // One answer only: a second call of the model would never return.
        let bye = Message {
            role: MessageRole::Assistant,
            content: Some("Good bye.".to_string()),
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "stop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN }],
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "stop".to_string(),
            kind: crate::tool_impl::stop_tool::STOP_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
            security_attributes: Default::default(),
        }] };
        let Stuck { channel, context, agent, helper, mut events, running } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([bye].into()))), components).await;

        // `stop` needs approval; alice gives it.
        let request = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let ActorEvent::ApprovalRequested(request) = events.recv().await.expect("the channel is running").event {
                    break request;
                }
            }
        }).await.expect("the call is put up for approval");
        let ActorId::User(alice) = context.initiator_id else { panic!("alice acts") };
        channel.user_actor(alice).unwrap().send_approval(ApprovalResponse { id: request.id, agent_id: agent }, true).await.unwrap();

        // The turn ends with the call - the model is not asked again - and
        // the agent sleeps, as the tool asked; no turn is left behind.
        settled(&helper, ActorState::Sleeping).await;
        let stored = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
        assert!(stored.turn.is_none(), "the turn was taken over");
        assert!(stored.history.messages().iter().any(|m| m.content.as_deref() == Some("Good bye.")), "and its answer recorded");
        running.abort();
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
            security_attributes: Default::default(),
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
        channel.user_actor(alice).unwrap().send_approval(ApprovalResponse { id: request.id, agent_id: agent }, true).await.unwrap();

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
    async fn a_turn_is_stored_with_the_step_that_comes_next() {
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        // The model calls a tool that needs no approval, and then never
        // answers again: the agent waits in `Planning` with the result.
        let calculate = Message {
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "calculate".to_string(), arguments: serde_json::json!({ "expression": "1+1" }), visibility: ToolVisibility::SHOWN }],
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "calculator".to_string(),
            kind: crate::tool_impl::calculator_tool::CALCULATOR_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
            security_attributes: Default::default(),
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
        async fn chat_completion(&self, _: &[Message], _: &[caelumora_model::tool::ToolDefinition])
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
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "calculate".to_string(), arguments: serde_json::json!({ "expression": "1+1" }), visibility: ToolVisibility::SHOWN }],
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "calculator".to_string(),
            kind: crate::tool_impl::calculator_tool::CALCULATOR_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
            security_attributes: Default::default(),
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
        let seen = crate::view::InternalContext::of(ActorId::Agent(agent), stored.clone());
        use crate::view::AgentContextView;
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
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "calculate".to_string(), arguments: serde_json::json!({ "expression": "1+1" }), visibility: ToolVisibility::SHOWN }],
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "calculator".to_string(),
            kind: crate::tool_impl::calculator_tool::CALCULATOR_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
            security_attributes: Default::default(),
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
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "calculate".to_string(), arguments: serde_json::json!({ "expression": "1+1" }), visibility: ToolVisibility::SHOWN }],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "calculator".to_string(),
            kind: crate::tool_impl::calculator_tool::CALCULATOR_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
            security_attributes: Default::default(),
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
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::confidentiality::Confidentiality;
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        // `stop` needs approval, which nobody gives.
        let bye = Message {
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "stop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN }],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "stop".to_string(),
            kind: crate::tool_impl::stop_tool::STOP_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
            security_attributes: Default::default(),
        }] };
        let Stuck { channel, context, agent, helper, mut events, running } =
            with_an_agent_under(Arc::new(FlowRule), Arc::new(Scripted(std::sync::Mutex::new([bye].into()))), components).await;
        settled(&helper, ActorState::WaitingForApproval).await;

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
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        let bye = Message {
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "stop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN }],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "stop".to_string(),
            kind: crate::tool_impl::stop_tool::STOP_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
            security_attributes: Default::default(),
        }] };
        let Stuck { channel, context, agent, helper, mut events, running } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([bye].into()))), components).await;
        settled(&helper, ActorState::WaitingForApproval).await;

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
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        let bye = Message {
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "stop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN }],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "stop".to_string(),
            kind: crate::tool_impl::stop_tool::STOP_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
            security_attributes: Default::default(),
        }] };
        let Stuck { channel, agent, helper, mut events, running, .. } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([bye].into()))), components).await;
        settled(&helper, ActorState::WaitingForApproval).await;
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
        let manager = channel.runtime.new_actor_runtime(ActorId::System, document.session_id, document.id).runtime_manager().clone();
        let executor = CommandExecutor::empty(manager.security_manager());
        let restarted = ChannelLoop::new(document.clone(), ChannelRuntime::new(manager, Arc::new(executor))).await.unwrap();
        let mut events = restarted.subscribe();
        let running = tokio::spawn({ let restarted = restarted.clone(); async move { restarted.run().await } });
        let helper = restarted.actor_by_id(ActorId::Agent(agent)).unwrap();

        // The agent asks again, with the same id, and the users can decide.
        let asked_again = approval_requested(&mut events).await;
        assert_eq!(asked_again.id, asked.id);
        let alice = document.participants.iter().find_map(|p| match p { Participant::User(id) => Some(*id), _ => None }).expect("alice");
        let response = caelumora_model::approval::ApprovalResponse { id: asked.id, agent_id: agent };
        restarted.user_actor(alice).unwrap().send_approval(response, true).await.unwrap();
        settled(&helper, ActorState::Sleeping).await;
        running.abort();
    }

    #[tokio::test]
    async fn a_cancel_while_a_tool_runs_ends_the_turn_after_the_tool() {
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};

        let call = Message {
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "sleep".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN }],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "sleep".to_string(),
            kind: crate::tool_impl::sleep_tool::SLEEP_TOOL_KIND.to_string(),
            params: serde_json::json!({ "seconds": 0.5 }),
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
            security_attributes: Default::default(),
        }] };
        let unasked = Message { role: MessageRole::Assistant, content: Some("never given".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let model = Arc::new(Scripted(std::sync::Mutex::new([call, unasked].into())));
        let Stuck { channel, context, helper, running, .. } = with_an_agent(model.clone(), components).await;
        settled(&helper, ActorState::ExecutingTools).await;

        channel.cancel(&context).await.expect("permitted");

        // The tool step returns `Planning`; the cancel wins over it, and the
        // model is not asked again.
        settled(&helper, ActorState::Idle).await;
        assert_eq!(model.0.lock().unwrap().len(), 1, "the second answer was never asked for");
        running.abort();
    }

    #[tokio::test]
    async fn an_agent_removed_while_waiting_for_approval_gives_up_its_turn() {
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolVisibility};
        use caelumora_model::transcript::GivenUpReason;

        let bye = Message {
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "stop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN }],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "stop".to_string(),
            kind: crate::tool_impl::stop_tool::STOP_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
            security_attributes: Default::default(),
        }] };
        let Stuck { channel, context, agent, helper, running, .. } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([bye].into()))), components).await;
        settled(&helper, ActorState::WaitingForApproval).await;

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
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "create_plan".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN }],
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let answer = Message { role: MessageRole::Assistant, content: Some("Let me answer directly.".to_string()), ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "plan".to_string(),
            kind: crate::component_impl::create_plan_tool::CREATE_PLAN_TOOL_KIND.to_string(),
            params: serde_json::Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
            security_attributes: Default::default(),
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
        let mut events = channel.subscribe();
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
        let channel = external_channel(remote, external_components(), None).await.expect("the external agent can be run");
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
    async fn external_channel(remote: Arc<crate::external::testing::ScriptedExternalAgent>, components: caelumora_model::component::ComponentConfig, stored: Option<caelumora_model::external_agent::ExternalAgentChannelState>) -> Result<(Arc<ChannelLoop>, SecurityContext, caelumora_model::agent::AgentId, UserId)> {
        use caelumora_model::agent::{Agent, Personality, PersonalityId, PersonalityKind};
        use caelumora_model::scope::Scope;

        let store = Arc::new(MemoryStore::new());
        let personality = Personality {
            description: String::new(),
            introduction: None,
            reasoning_visibility: caelumora_model::confidentiality::Visibility::Channel,
            id: PersonalityId::new(),
            name: ScopedName::new("remote", Default::default()).unwrap(),
            kind: PersonalityKind::External,
            roles: None,
            components,
            security_attributes: Default::default(),
        };
        store.create_personality(&SecurityContext::system(), &personality).await.unwrap();
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let agent = Agent::new(ScopedName::new("hermes", Scope::GlobalScope).unwrap(), &personality, context.initiator_id);
        let agent_id = agent.id;
        store.create_user(&SecurityContext::system(), &alice).await.unwrap();
        store.create_agent(&SecurityContext::system(), &agent).await.unwrap();

        let security: Arc<dyn SecurityManager> = Arc::new(DummySecurityManager::new());
        let secrets = Arc::new(SecretManager::new(vec![], security.clone()));
        let mut registry = crate::component::ComponentRegistry::built_in(store.clone());
        registry.register(
            crate::component::ComponentKind::new("test_remote").provides::<dyn crate::external::ExternalAgent>(),
            move |_descriptor, _context| {
                let mut facets = crate::component::ComponentFacets::empty();
                facets.insert::<dyn crate::external::ExternalAgent>(remote.clone());
                Ok(facets)
            },
        );
        register_no_secrets(&mut registry);
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store, secrets, security, default_commands(), Default::default(), registry).unwrap());
        let channel = Channel {
            id: ChannelId::new(),
            session_id: SessionId::new(),
            name: Name::new("main").unwrap(),
            components: caelumora_model::channel::moderation_components(None, Some("broadcast_policy")),
            participants: vec![Participant::User(alice.id), Participant::Agent(agent_id)],
            security_attributes: SecurityAttributes::default(),
            classification: caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT,
            history_limits: Vec::new(),
        };
        runtime.entity_store().create_channel(&SecurityContext::system(), &channel).await.unwrap();
        if let Some(stored) = stored {
            runtime.state_store().put_external_agent_channel_state(agent_id, channel.id, &stored).await.unwrap();
        }
        let executor = CommandExecutor::empty(runtime.security_manager());
        let channel = ChannelLoop::new(channel, ChannelRuntime::new(runtime, Arc::new(executor))).await?;
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

    /// Registers `NoSecrets` as the kind `test_no_secrets`.
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
        let (channel, context, agent, _) = external_channel(remote, external_components(), None).await.unwrap();
        let message = |sender: ActorId, text: &str| Message {
            id: MessageId::new(), role: MessageRole::User, sender_id: sender, content: Some(text.to_string()),
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
        };
        let said = |text: &str| message(context.initiator_id, text);
        let (one, two, three) = (said("one"), said("two"), said("three"));
        let own = message(ActorId::Agent(agent), "mine");
        let actor = channel.actor_by_id(ActorId::Agent(agent)).unwrap();
        let kept = || async {
            channel.state_store.get_external_agent_channel_state(agent, channel.id).await.unwrap()
                .history.messages().into_iter().map(|m| m.content.unwrap()).collect::<Vec<_>>()
        };

        actor.catch_up(vec![two.clone()]).await.unwrap();
        assert_eq!(kept().await, ["two"], "what it may see");
        actor.catch_up(vec![one, two, own, three]).await.unwrap();
        assert_eq!(kept().await, ["two", "mine", "three"], "what it missed, and nothing older");
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
        let built = external_channel(remote.clone(), components, None).await.expect("the external agent can be run");
        let Stuck { channel, context, agent, mut events, running, .. } = start(built).await;
        answered(&mut events, agent).await;

        let alice = channel.user_actor(alice_of(&context)).unwrap();
        alice.send_message("the secret is 42".to_string(), None).await.unwrap();
        assert!(answered(&mut events, agent).await.is_empty(), "nothing new reached it, so it is not asked");

        let state = channel.state_store.get_external_agent_channel_state(agent, channel.id).await.unwrap();
        let contents = |messages: Vec<Message>| messages.into_iter().filter_map(|m| m.content).collect::<Vec<_>>();
        assert_eq!(contents(state.history.messages()), vec!["hello", "Hello, alice.", "the secret is 42"], "its history keeps all it was offered");
        assert_eq!(contents(state.transcript.messages()), vec!["hello"], "its transcript what it was given");
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
        let events = channel.subscribe();
        let running = tokio::spawn({ let channel = channel.clone(); async move { channel.run().await } });
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

    #[tokio::test]
    async fn an_external_agent_is_given_what_was_said_since_it_was_last_asked_and_answers_in_the_channel() {
        use crate::external::{ExternalEvent, Said};
        use caelumora_model::external_agent::ConversationId;

        let conversation = ConversationId("c-1".to_string());
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![
                Ok(ExternalEvent::Conversation(conversation.clone())),
                Ok(ExternalEvent::Started(task("t-1"))),
                Ok(ExternalEvent::Partial("Hel".to_string())),
                Ok(ExternalEvent::Message("Hello, alice.".to_string())),
                Ok(ExternalEvent::Completed),
            ],
            vec![Ok(ExternalEvent::Started(task("t-2"))), Ok(ExternalEvent::Message("Fine.".to_string())), Ok(ExternalEvent::Completed)],
        ]));
        let Stuck { channel, context, agent, mut events, running, .. } = with_an_external_agent(remote.clone()).await;

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
        assert_eq!(sent[0].1.said, vec![said("hello")]);
        assert_eq!(sent[1].0, Some(conversation.clone()), "and keeps it");
        assert_eq!(sent[1].1.said, vec![said("How are you?")], "only what it has not heard, not its own answer");

        let state = channel.state_store.get_external_agent_channel_state(agent, channel.id).await.unwrap();
        assert_eq!(state.conversation, Some(conversation));
        assert!(state.open_tasks.is_empty(), "both tasks ended");
        let kept: Vec<_> = state.history.messages().into_iter().filter_map(|m| m.content).collect();
        assert_eq!(kept, vec!["hello", "Hello, alice.", "How are you?", "Fine."], "what it heard and what it answered");
        let given: Vec<_> = state.transcript.messages().into_iter().filter_map(|m| m.content).collect();
        assert_eq!(given, vec!["hello", "How are you?"], "what the remote agent was given");
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
        let state = channel.state_store.get_external_agent_channel_state(agent, channel.id).await.unwrap();
        assert!(state.open_tasks.is_empty(), "the failed task ended");
        running.abort();
    }

    #[tokio::test]
    async fn a_cancel_ends_the_answer_of_an_external_agent_and_its_remote_task() {
        use crate::external::ExternalEvent;

        // The remote agent takes the task on and then works on it for ever.
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Started(task("t-1"))), Ok(ExternalEvent::Partial("Let me think".to_string()))],
        ]));
        let Stuck { channel, context, agent, helper, mut events, running } = with_an_external_agent(remote.clone()).await;
        settled(&helper, ActorState::Planning).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        channel.cancel(&context).await.expect("permitted");
        let ended = answered(&mut events, agent).await;
        assert_eq!(ended.first().and_then(|m| m.content.as_deref()), Some("Let me think"), "what came so far");
        settled(&helper, ActorState::Idle).await;
        assert_eq!(remote.cancelled.lock().unwrap().as_slice(), &[task("t-1")], "the remote task is cancelled too");
        let state = channel.state_store.get_external_agent_channel_state(agent, channel.id).await.unwrap();
        assert!(state.open_tasks.is_empty());
        running.abort();
    }

    #[tokio::test]
    async fn an_external_agent_removed_while_it_answers_cancels_its_task_and_says_nothing() {
        use crate::external::ExternalEvent;

        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Started(task("t-1")))],
        ]));
        let Stuck { channel, context, agent, helper, mut events, running } = with_an_external_agent(remote.clone()).await;
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

    #[tokio::test]
    async fn an_external_agent_that_asked_back_is_answered_within_the_same_task() {
        use crate::external::ExternalEvent;
        use caelumora_model::external_agent::OpenTaskState;

        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::answering(vec![
            vec![Ok(ExternalEvent::Started(task("t-1"))), Ok(ExternalEvent::InputRequired("Which city?".to_string()))],
            vec![Ok(ExternalEvent::Message("Sunny in Bonn.".to_string())), Ok(ExternalEvent::Completed)],
        ]));
        let Stuck { channel, context, agent, mut events, running, .. } = with_an_external_agent(remote.clone()).await;

        let question = answered(&mut events, agent).await;
        assert_eq!(question[0].content.as_deref(), Some("Which city?"), "the question is the answer");
        let state = channel.state_store.get_external_agent_channel_state(agent, channel.id).await.unwrap();
        assert_eq!(state.open_tasks.iter().map(|t| (t.task.clone(), t.state)).collect::<Vec<_>>(), vec![(task("t-1"), OpenTaskState::InputRequired)]);

        let alice = channel.user_actor(alice_of(&context)).unwrap();
        alice.send_message("Bonn".to_string(), None).await.unwrap();
        assert_eq!(answered(&mut events, agent).await[0].content.as_deref(), Some("Sunny in Bonn."));
        assert_eq!(remote.sent.lock().unwrap()[1].1.answers, Some(task("t-1")), "the reply continues the task that asked");
        let state = channel.state_store.get_external_agent_channel_state(agent, channel.id).await.unwrap();
        assert!(state.open_tasks.is_empty(), "and it ended");
        running.abort();
    }

    #[tokio::test]
    async fn an_external_personality_that_names_no_remote_agent_cannot_be_run() {
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::default());
        let refused = external_channel(remote, Default::default(), None).await.expect_err("nothing to reach");
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
        assert!(woke, "the channel's agents are woken for it, as for a user's message");
        assert_eq!(said[0].content.as_deref(), Some("Here it is."));
        let state = channel.state_store.get_external_agent_channel_state(agent, channel.id).await.unwrap();
        assert!(state.open_tasks.is_empty(), "the task ended");
        assert_eq!(state.history.messages().last().and_then(|m| m.content.clone()).as_deref(), Some("Here it is."), "kept as said");
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
            let state = channel.state_store.get_external_agent_channel_state(agent, channel.id).await.unwrap();
            let own: Vec<_> = state.history.messages().into_iter().filter(|m| m.sender_id == ActorId::Agent(agent)).filter_map(|m| m.content).collect();
            assert_eq!(own, vec![expected.to_string()], "and kept once");
            running.abort();
        }
    }

    #[tokio::test]
    async fn after_a_restart_an_open_task_is_followed_again_and_one_that_asked_back_waits() {
        use crate::external::ExternalEvent;
        use caelumora_model::external_agent::{ExternalAgentChannelState, OpenTask, OpenTaskState};

        let stored = ExternalAgentChannelState {
            open_tasks: vec![
                OpenTask { task: task("t-working"), since: chrono::Utc::now(), state: OpenTaskState::Working, unsaid: None },
                OpenTask { task: task("t-asking"), since: chrono::Utc::now(), state: OpenTaskState::InputRequired, unsaid: None },
            ],
            ..ExternalAgentChannelState::default()
        };
        let remote = Arc::new(crate::external::testing::ScriptedExternalAgent::default()
            .resuming(vec![vec![Ok(ExternalEvent::Message("Finished while you were away.".to_string())), Ok(ExternalEvent::Completed)]]));
        let (channel, _, agent, _) = external_channel(remote.clone(), external_components(), Some(stored)).await.unwrap();
        let mut events = channel.subscribe();
        let running = tokio::spawn({ let channel = channel.clone(); async move { channel.run().await } });

        let said = answered(&mut events, agent).await;
        assert_eq!(said[0].content.as_deref(), Some("Finished while you were away."), "what it did meanwhile, unasked");
        let state = channel.state_store.get_external_agent_channel_state(agent, channel.id).await.unwrap();
        assert_eq!(state.open_tasks.iter().map(|t| t.task.clone()).collect::<Vec<_>>(), vec![task("t-asking")], "the one that asked back waits for the next request");
        assert!(remote.sent.lock().unwrap().is_empty(), "nothing was sent to it");
        running.abort();
    }

    /// The state of an external agent with one task working, as a restart
    /// finds it.
    fn working_on(open: &str) -> caelumora_model::external_agent::ExternalAgentChannelState {
        use caelumora_model::external_agent::{ExternalAgentChannelState, OpenTask, OpenTaskState};
        ExternalAgentChannelState {
            open_tasks: vec![OpenTask { task: task(open), since: chrono::Utc::now(), state: OpenTaskState::Working, unsaid: None }],
            ..ExternalAgentChannelState::default()
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
        let (channel, _, agent, _) = external_channel(remote, external_components(), Some(working_on("t-1"))).await.unwrap();
        let mut events = channel.subscribe();
        let running = tokio::spawn({ let channel = channel.clone(); async move { channel.run().await } });

        let failed = turn_failed(&mut events, agent).await;
        assert!(failed.contains("hermes could not finish: out of coffee"), "{failed}");
        let state = channel.state_store.get_external_agent_channel_state(agent, channel.id).await.unwrap();
        assert!(state.open_tasks.is_empty(), "the failed task ended");
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
        let (channel, _, agent, _) = external_channel(remote, external_components(), Some(working_on("t-1"))).await.unwrap();
        let mut events = channel.subscribe();
        let running = tokio::spawn({ let channel = channel.clone(); async move { channel.run().await } });
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(!said_anything(&mut events, agent), "no half answer is published");
        let state = channel.state_store.get_external_agent_channel_state(agent, channel.id).await.unwrap();
        assert_eq!(state.open_tasks.iter().map(|t| (t.task.clone(), t.state)).collect::<Vec<_>>(), vec![(task("t-1"), OpenTaskState::Working)], "left open, to be followed again");
        assert!(state.history.messages().is_empty(), "nothing is kept as said");
        running.abort();
    }
}
