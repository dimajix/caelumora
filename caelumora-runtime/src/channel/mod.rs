//! A running channel: `ChannelLoop`, how it is built, and what it tells of itself and its actors.

mod control;
mod history;
mod inbox;
mod participants;
mod participation;
mod run;
mod settings;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
#[cfg(test)]
mod tests;

pub(crate) use participation::{Participation, ResolvedKind, ResolvedParticipant};

use crate::actor::approval::ApproverActor;
use crate::actor::user::UserActor;
use crate::actor::Actor;
use crate::actor::AgentKinds;
use crate::attachment::AttachmentManager;
use crate::component::{ComponentContainer, ComponentContext, ComponentError, ComponentImpl};
use crate::controller::{ChannelController, ControllerManager};
use crate::moderator::Moderator;
use crate::runtime::ChannelRuntime;
use crate::session::SessionError;
use crate::session::SessionLoop;
use anyhow::{Error, Result};
use arc_swap::ArcSwap;
use caelumora_model::actor::ActorId;
#[cfg(any(test, feature = "test-support"))]
use caelumora_model::actor::Envelope;
use caelumora_model::chain::Position;
use caelumora_model::channel::{Channel, ChannelId, ChannelState, Participant};
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy};
use caelumora_model::mention::MentionDirectory;
use caelumora_model::message::MessageId;
use caelumora_model::question::{QuestionError, QuestionId};
use caelumora_model::security::{
    AccessPolicy, ActionType, Enforcement, Refused, SecurityAttributes, SecurityContext,
};
use caelumora_model::session::SessionId;
use caelumora_model::store::history::HistoryStore;
use caelumora_model::store::{StateStore, StoreError};
use caelumora_model::type_map::TypeKey;
use caelumora_model::user::UserId;
use caelumora_model::value::name::Name;
use caelumora_model::work::TurnId;
use futures::future;
use std::any::Any;
use std::collections::HashMap;
use std::sync;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
#[cfg(any(test, feature = "test-support"))]
use tokio::sync::broadcast;
use tokio::sync::{mpsc, Mutex};
use tokio::time;
use tokio_util::task::TaskTracker;
use tracing::{info, warn};

/// A participant of a channel, with the handle it is `@`-mentioned by.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelParticipant {
    pub id: ActorId,
    pub name: Name,
    /// The handle the participant is mentioned by (see `caelumora_model::mention`),
    /// or `None` if it cannot be mentioned - approvers, and names that yield
    /// no handle.
    pub handle: Option<String>,
}

/// Something that may not be done in a channel. Raised by `ChannelLoop`;
/// the messages are meant for users (see `crate::error::user_message`).
#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    /// The `AccessPolicy` refused the action. Deliberately silent about
    /// which rule refused it and about what the caller would have to be
    /// instead - like a refused tool call (issue 15).
    #[error("{action} channel {channel} is not permitted")]
    Forbidden {
        action: &'static str,
        channel: ChannelId,
        #[source]
        refused: Refused,
    },
    /// Only participants of the session can be added to its channels
    /// (issue 46).
    #[error("{participant} is not a participant of the session of channel {channel}")]
    NotInSession { participant: ActorId, channel: ChannelId },
    /// An agent of another session's or channel's scope, which takes part
    /// only there (issue 178).
    #[error(
        "{participant} lies in the scope of another session or channel than channel {channel}"
    )]
    OutOfScope { participant: ActorId, channel: ChannelId },
    #[error("{participant} already takes part in channel {channel}")]
    AlreadyParticipant { participant: ActorId, channel: ChannelId },
    #[error("{participant} takes no part in channel {channel}")]
    NotAParticipant { participant: ActorId, channel: ChannelId },
    /// An agent nudged while it is in a turn (issue 278): asked again, it
    /// would answer twice.
    #[error("{participant} is responding in channel {channel} already")]
    AlreadyResponding { participant: ActorId, channel: ChannelId },
    /// A user nudging themselves (issue 278): nobody would be asked.
    #[error("{participant} cannot nudge themselves in channel {channel}")]
    NudgesItself { participant: ActorId, channel: ChannelId },
    /// A participant of the session whose user or agent was deleted
    /// (issue 120).
    #[error("{participant} of the session of channel {channel} no longer exists")]
    ParticipantGone { participant: ActorId, channel: ChannelId },
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
    /// A turn is asked for again only while it is under way (issue 235),
    /// or as the agent's last turn, given up for a failure or a lowering,
    /// with nothing after it (issue 259).
    #[error("turn {turn} can be asked for again in channel {channel} neither as under way nor as the last one given up")]
    TurnNotRetryable { turn: TurnId, channel: ChannelId },
    /// A tool call is interrupted on its own only while it runs or waits to
    /// run in the turn under way (issue 276).
    #[error("tool call {call} of turn {turn} does not run in channel {channel}")]
    ToolCallNotRunning { call: String, turn: TurnId, channel: ChannelId },
    /// A participant not cleared for the channel's classification - to
    /// join it, or for the classification it would be raised to (issue
    /// 133).
    #[error(
        "{participant} is not cleared for channel {channel} as it is, or would be, classified"
    )]
    NotCleared { participant: ActorId, channel: ChannelId },
    /// An agent whose personality is of a kind the program runs none of
    /// (`AgentKinds`, issue 370): its actor cannot be built.
    #[error("{participant} is of a kind of agent that cannot take part in channel {channel} here")]
    UnknownAgentKind { participant: ActorId, channel: ChannelId },
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

/// Whether `error` of `Participation::resolve` or of building its actor says
/// that the participant cannot be run - its agent's personality, or the
/// component bundles it includes (issue 270), are gone or not the
/// agent's to read - rather than that the store failed.
fn cannot_run(error: &Error) -> bool {
    error.downcast_ref::<StoreError>().is_some_and(StoreError::is_absent_or_refused)
        || error
            .chain()
            .any(|e| e.downcast_ref::<ComponentError>().is_some_and(ComponentError::is_unavailable))
}

/// The actor of a channel's participant, with the participant it was built
/// from, resolved, and what tells when its task has ended.
#[derive(Debug, Clone)]
struct ChannelActor {
    actor: Arc<dyn Actor>,
    participant: ResolvedParticipant,
    /// Tracks the actor's task while `run` runs it, so that whoever stops
    /// the actor can wait for the task to end without going through `run`
    /// (issue 187) - `run`'s `JoinSet` alone owns the task.
    task: TaskTracker,
}

/// The actors of a channel's participants.
type Actors = HashMap<ActorId, ChannelActor>;

/// How long an actor that stops or leaves gets to end its task by itself
/// before it is aborted (issues 75, 377). An agent's tool under way is
/// interrupted rather than waited for (issue 161); this is for whatever
/// else hangs.
pub const STOP_GRACE: Duration = Duration::from_secs(5);

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
    time::timeout(STOP_GRACE, future::join_all(tasks.iter().map(TaskTracker::wait))).await.is_ok()
}

/// A change to the actors a running channel runs (issue 46), carried out by
/// `run`, which owns their tasks.
enum ActorChange {
    /// The actor to start - every actor of the channel is started so, as
    /// the channel is built and as a participant joins -, from which
    /// position on it is given the history - `None`: what is said from its
    /// start on, a founder's (issue 420) or an actor's that starts again with
    /// its channel -, and on whose behalf it joined, in whose name `run`
    /// announces it as it starts it (`ParticipantJoined`, issue 408), with
    /// what it was given (`HistoryShared`, issue 417) - `None` for an actor
    /// that starts again with its channel, or an internal one, which join
    /// nothing.
    Start(ChannelActor, Option<Position>, Option<SecurityContext>),
    /// The actor that leaves, what it was called as it left, and on whose
    /// behalf it was removed - in whose name `run` announces it once its
    /// task has ended (`ParticipantLeft`, issues 162, 262, 408).
    Leave(ActorId, Name, SecurityContext),
}

#[derive(Debug)]
pub struct ChannelLoop {
    id: ChannelId,
    session_id: SessionId,
    channel: Arc<ChannelController>,
    /// What builds the runtimes of its actors.
    runtime: ChannelRuntime,
    /// The services it works with, taken from `runtime` once (issue 229).
    access_policy: Arc<dyn AccessPolicy>,
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    controllers: Arc<ControllerManager>,
    participation: Arc<Participation>,
    /// What the attachments of a message are claimed from (issue 282).
    attachments: Arc<AttachmentManager>,
    /// What builds the actors of its agents, as their kind says (issue 370).
    agent_kinds: Arc<AgentKinds>,
    /// The actors of the participants and of the channel's internal actors
    /// (issue 420), each with what it was built from -
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
    leaving: sync::Mutex<HashMap<ActorId, TaskTracker>>,
    moderator: ComponentImpl<dyn Moderator>,
    mentions: ArcSwap<MentionDirectory>,
    /// Actors for `run` to start or stop; `run` takes the receiver.
    changes_tx: mpsc::UnboundedSender<ActorChange>,
    changes_rx: Mutex<mpsc::UnboundedReceiver<ActorChange>>,
    state: Mutex<ChannelState>,
    state_store: Arc<dyn StateStore>,
    /// The read side of the histories (issue 379): the channel's own, as it
    /// is stored with its state after every entry.
    history_store: Arc<dyn HistoryStore>,
    /// Set by `cancel`, cleared when the channel is woken: while set, what is
    /// said is still delivered, but nobody is asked to respond to it
    /// (`route_event_impl`). The turns themselves are cancelled on the actors.
    quiet: AtomicBool,
    /// Every event the channel took in, for tests that watch what happens
    /// in it beyond what a user is told; nobody follows a channel this way
    /// otherwise - a user follows their actor (issue 220).
    #[cfg(any(test, feature = "test-support"))]
    observed: broadcast::Sender<Envelope>,
    run_lock: Mutex<()>,
}

impl ChannelLoop {
    /// The facets it asks its components for (issue 337).
    pub const COMPONENT_FACETS: &[TypeKey] = &[TypeKey::of::<dyn Moderator>()];

    /// Builds the channel `runtime` holds the controller of, and its actors,
    /// whose runtimes `runtime` creates and which reach the channel through
    /// it once it exists.
    pub async fn new(runtime: ChannelRuntime) -> Result<Arc<Self>> {
        let channel_controller = Arc::clone(runtime.channel_controller());
        let channel = Arc::clone(&channel_controller.load());
        let id = channel.id;
        let session_id = channel.session_id;
        let state_store = Arc::clone(runtime.state_store());
        let history_store = Arc::clone(runtime.history_store());
        // Its components (issue 174) are built in its owner's name, as an
        // agent's are in the agent's, with the roles of now (issue 321):
        // what they ask is asked as they are built; routing, they act in
        // the context of the event they route.
        let security_context = runtime
            .controller_manager()
            .owner_context(channel.security_attributes.owner_id, session_id, id)
            .await?;
        let components = ComponentContainer::from_config(
            &channel.components,
            Self::COMPONENT_FACETS,
            runtime.component_registry(),
            &ComponentContext::new(security_context, runtime.services().clone()),
        )
        .await
        .map_err(SessionError::InvalidComponents)?;
        let moderator =
            components.one::<dyn Moderator>().await.map_err(SessionError::InvalidComponents)?;

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

        let (changes_tx, changes_rx) = mpsc::unbounded_channel();
        let channel_loop = Arc::new(Self {
            id,
            session_id,
            channel: channel_controller,
            access_policy: Arc::clone(runtime.access_policy()),
            confidentiality: Arc::clone(runtime.confidentiality()),
            controllers: Arc::clone(runtime.controller_manager()),
            participation: Arc::clone(runtime.participation()),
            attachments: Arc::clone(runtime.attachment_manager()),
            agent_kinds: Arc::clone(runtime.agent_kinds()),
            runtime,
            actors: ArcSwap::from_pointee(HashMap::new()),
            leaving: sync::Mutex::new(HashMap::new()),
            moderator,
            mentions: ArcSwap::from_pointee(MentionDirectory::default()),
            changes_tx,
            changes_rx: Mutex::new(changes_rx),
            state,
            state_store,
            history_store,
            quiet: AtomicBool::new(false),
            #[cfg(any(test, feature = "test-support"))]
            observed: broadcast::channel(128).0,
            run_lock: Mutex::new(()),
        });
        channel_loop.runtime.attach(&channel_loop);
        // Its internal actors (issue 420), which are its own: nothing to
        // resolve, nothing to leave out. `run` starts them with the
        // participants'.
        let internal_actors = channel_loop.channel.load().internal_actors.clone();
        for internal in &internal_actors {
            let resolved = ResolvedParticipant::internal(internal, session_id, id);
            channel_loop.start_participant(resolved, None, None).await?;
        }
        // The participants the channel has, which start again with it:
        // nothing to take in, no join to announce.
        let participants = channel_loop.channel.load().participants.clone();
        for participant in &participants {
            // A user or agent deleted since it joined - an agent rests in
            // the channel until it is restored (issue 314) -, or an agent
            // that may not run - its personality, or what it includes (issue
            // 270), withdrawn or gone: the channel runs without it rather
            // than not at all (issues 120, 198). Its owner's loss must not
            // keep everyone else out, nor can a third party - the
            // personality's owner, or the components' - stop someone's
            // channel so.
            let found = channel_loop.resolve(participant).await;
            let resolved = match found {
                Ok(Some(resolved)) => resolved,
                Ok(None) => {
                    info!(
                        "{} of channel {} no longer exists and is left out",
                        participant.actor_id(),
                        channel_loop.id
                    );
                    continue;
                }
                Err(e) if cannot_run(&e) => {
                    warn!(
                        "{} of channel {} cannot be run and is left out: {:#}",
                        participant.actor_id(),
                        channel_loop.id,
                        e
                    );
                    continue;
                }
                Err(e) => return Err(e),
            };
            match channel_loop.start_participant(resolved, None, None).await {
                Ok(()) => {}
                Err(e) if cannot_run(&e) => warn!(
                    "{} of channel {} cannot be run and is left out: {:#}",
                    participant.actor_id(),
                    channel_loop.id,
                    e
                ),
                Err(e) => return Err(e),
            }
        }
        Ok(channel_loop)
    }

    /// `participant` with what its actor is built from
    /// (`Participation::resolve`).
    async fn resolve(&self, participant: &Participant) -> Result<Option<ResolvedParticipant>> {
        self.participation.resolve(participant, self.session_id, Some(self.id)).await
    }

    /// The session the channel belongs to (issues 123, 361), given it with
    /// its runtime; `None` once it is gone, or for a channel of none, in
    /// tests.
    pub(crate) fn session_loop(&self) -> Option<Arc<SessionLoop>> {
        self.runtime.session_loop()
    }

    /// Takes `actor` into the channel and hands it to `run` to start
    /// (`ActorChange::Start`, given `from` and `joined` as it says): the
    /// moderator routes to it, it is among the participants' actors, and it
    /// can be mentioned. The one way an actor enters the channel - when the
    /// channel is built and when a participant joins (issue 91).
    async fn adopt(
        &self,
        actor: Arc<dyn Actor>,
        resolved: ResolvedParticipant,
        from: Option<Position>,
        joined: Option<SecurityContext>,
    ) {
        self.moderator.add_actor(Arc::clone(&actor));
        let id = actor.id();
        let adopted = ChannelActor { actor, participant: resolved, task: TaskTracker::new() };
        self.actors.rcu(|actors| {
            let mut actors = HashMap::clone(actors);
            actors.insert(id, adopted.clone());
            actors
        });
        self.rebuild_mentions();
        let _ = self.changes_tx.send(ActorChange::Start(adopted, from, joined));
    }

    /// Every event the channel takes in from now on (see `observed`).
    #[cfg(any(test, feature = "test-support"))]
    pub fn observe(&self) -> broadcast::Receiver<Envelope> {
        self.observed.subscribe()
    }

    pub fn id(&self) -> ChannelId {
        self.id
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub fn name(&self) -> Name {
        self.channel.name()
    }

    /// Who the channel belongs to and who contributes to it (issue 11).
    pub fn security_attributes(&self) -> SecurityAttributes {
        self.channel.load().security_attributes.clone()
    }

    /// The channel as it is now: a snapshot, to be read or handed on to an operation - not to be
    /// kept, since it does not follow later changes.
    pub fn channel(&self) -> Arc<Channel> {
        Arc::clone(&self.channel.load())
    }

    /// `action` on the channel, as its attributes are now, refused as a
    /// `ChannelError::Forbidden` that names what was refused, not why.
    async fn require(
        &self,
        context: &SecurityContext,
        action: impl FnOnce(&SecurityAttributes) -> ActionType<'_>,
        what: &'static str,
    ) -> Result<()> {
        let attributes = self.security_attributes();
        let channel = self.id;
        Ok(self
            .access_policy
            .require(context, action(&attributes), |refused| ChannelError::Forbidden {
                action: what,
                channel,
                refused,
            })
            .await?)
    }

    /// The classification of the channel's contents (issue 133).
    pub fn classification(&self) -> Classification {
        self.channel.load().classification.clone()
    }

    pub(crate) fn actor(&self, actor_id: ActorId) -> Option<Arc<dyn Actor>> {
        self.actors.load().get(&actor_id).map(|a| Arc::clone(&a.actor))
    }

    /// The actor of `actor_id` if it is an agent of this channel - whom a
    /// request to answer again or to carry out a command may name.
    pub(crate) fn agent_actor(&self, actor_id: ActorId) -> Option<Arc<dyn Actor>> {
        actor_id.as_agent_id().and_then(|_| self.actor(actor_id))
    }

    /// The turn under way of the participant `actor` (issue 235), if it is
    /// in one that can still be ended - what a request to answer it again
    /// names.
    pub(crate) fn turn_under_way(&self, actor: ActorId) -> Option<TurnId> {
        self.actor(actor)?.turn_under_way()
    }

    /// Whether the turn `turn` of `author` can be asked for again: under
    /// way (issue 235), or `author`'s last turn, given up for a reason that
    /// may be answered again and with nothing after it in the channel's
    /// history (issue 259).
    pub(crate) async fn retryable_turn(&self, author: ActorId, turn: TurnId) -> bool {
        self.turn_under_way(author) == Some(turn) || self.given_up_last(author, turn).await
    }

    /// Whether `turn` is `author`'s last turn given up, which may be
    /// answered anew after (issue 259).
    async fn given_up_last(&self, author: ActorId, turn: TurnId) -> bool {
        let state = self.state.lock().await;
        state
            .history
            .turn_given_up_last(author)
            .is_some_and(|(last, reason)| last == turn && reason.may_be_answered_again())
    }

    /// The actor `user` acts through in this channel, or `None` if they are
    /// not a participant of it.
    ///
    /// Every envelope is built by the actor itself, so this is what decides
    /// whose name a message or an approval carries: there is deliberately no
    /// way to reach "some" user actor (issue 30).
    pub fn user_actor(&self, user: UserId) -> Option<Arc<UserActor>> {
        let actor = self.actor(ActorId::User(user))?;
        let any_actor: Arc<dyn Any + Send + Sync> = actor;
        Arc::downcast::<UserActor>(any_actor).ok()
    }

    /// The actor of `resolved`, built but not yet taken in (`adopt`).
    async fn create_actor(&self, resolved: &ResolvedParticipant) -> Result<Box<dyn Actor>> {
        let actor_runtime = self.runtime.new_actor_runtime(resolved.actor_id());
        Ok(match resolved.kind().clone() {
            ResolvedKind::User(user) => Box::new(UserActor::new(user, actor_runtime).await?),
            ResolvedKind::Agent(agent, personality) => {
                self.agent_kinds.build(agent, personality, actor_runtime).await?
            }
            ResolvedKind::Approver(approver) => {
                Box::new(ApproverActor::new(approver, actor_runtime))
            }
        })
    }
}
