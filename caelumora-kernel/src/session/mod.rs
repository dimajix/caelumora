//! A running session: `SessionLoop`, how it is built, runs its channels and holds its workspace.

mod channels;
mod manager;
mod spec;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub(crate) use channels::unavailable_components;
pub(crate) use manager::Found;
pub use manager::SessionManager;
pub use spec::{ChannelSpec, SessionSpec};

use crate::channel::ChannelLoop;
use crate::command::CommandExecutor;
use crate::controller::{ControllerManager, SessionController, WorkspaceController};
use crate::event::{EventBus, EventSubscription, Following, SubscriptionTopic};
use crate::kernel::{ChannelRuntime, Kernel};
use crate::procedure::{ProcedureError, ProcedureExecutor};
use crate::scoped::ScopeNotEmpty;
use crate::view::{Viewer, Views};
use crate::workspace::WorkspaceManager;
use anyhow::{anyhow, Error, Result};
use arc_swap::ArcSwapOption;
use caelumora_api::component::ComponentError;
use caelumora_api::workspace::{WorkspaceError, WorkspaceResult};
use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::{Agent, AgentUpdateEvent};
use caelumora_domain::channel::{self, Channel, ChannelId, Participant};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::event::{Announcing, GlobalEvent};
use caelumora_domain::group::GroupId;
use caelumora_domain::privacy::SessionPrivacy;
use caelumora_domain::project::ProjectId;
use caelumora_domain::scope::Scope;
use caelumora_domain::security::{Grant, Subject};
use caelumora_domain::session::{Session, SessionId};
use caelumora_domain::store::{StoreError, StoreResult, StoreResultExt, Stored};
use caelumora_domain::template::SessionTemplateId;
use caelumora_domain::user::{User, UserUpdateEvent};
use caelumora_domain::value::name::Name;
use caelumora_domain::workspace::WorkspaceId;
use caelumora_security::confidentiality::{ConfidentialityPolicy, ConfidentialityQuestions};
use caelumora_security::{ActionType, Enforcement, Policies, SecurityContext};
use dashmap::DashMap;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc, Mutex};
use tokio::task::{AbortHandle, JoinSet};
use tracing::info;

/// A request for a channel that cannot be met as asked (issue 09). The
/// messages are meant for users (see `crate::error::user_message`).
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The channel's components cannot be built (issue 174): a kind that
    /// does not exist, two moderators, two policies, a component nobody asks
    /// for (issue 337), component bundles whoever creates the channel may
    /// not read (issue 321) - found before it is created; or whatever
    /// fails as they are built, after which a channel created so is taken
    /// back out.
    #[error("The channel's components cannot be built: {0}")]
    InvalidComponents(#[source] ComponentError),
    #[error("{participant} is not a participant of session {session}")]
    NotAParticipant { session: SessionId, participant: ActorId },
    /// Taken in already, and no channel named to take them into (issue 221).
    #[error("{participant} is a participant of session {session} already")]
    AlreadyParticipant { session: SessionId, participant: ActorId },
    /// Only a user or an agent is taken into a session after it was built
    /// (issue 221); an approver is made with it.
    #[error("{0} cannot be taken into a session: only a user or an agent can")]
    CannotTakeIn(ActorId),
    /// Only a user owns a session (issue 88), and the project of its own with it (issue 467).
    #[error("{0} cannot own a session: only a user can")]
    OwnerNotAUser(ActorId),
    /// There is no such session, or none the caller may see (issue 323).
    #[error("There is no session {0}")]
    NotFound(SessionId),
    /// There is no such session template, or none the caller may see.
    #[error("There is no session template {0}")]
    TemplateNotFound(SessionTemplateId),
    #[error("Session {session} has no channel {channel}")]
    ChannelNotFound { session: SessionId, channel: ChannelId },
    /// The session lists the channel, but it does not run (issue 463): its components cannot be
    /// read - a bundle it names is gone, or no longer its owner's to read.
    #[error("Channel {channel} of session {session} does not run: {cause}")]
    ChannelUnavailable {
        session: SessionId,
        channel: ChannelId,
        #[source]
        cause: Arc<ComponentError>,
    },
    /// No classification that can be given (`require_classifiable`, issue
    /// 304): it names a compartment that is not configured.
    #[error(
        "A channel cannot be classified {0}: a compartment is not configured or may not be given"
    )]
    ClassificationRefused(Classification),
    /// The workspace lies in another session's scope, and is used within
    /// that session alone (issue 251, `Scope::admits`).
    #[error("The workspace {0} lies in another session's scope")]
    WorkspaceOutOfScope(WorkspaceId),
    /// The project is another session's own, which takes no other session in (issue 467).
    #[error("The project {0} is another session's own")]
    ProjectOutOfScope(ProjectId),
    /// The shared project a deleted session lies in is deleted too: the session comes back with
    /// it, not alone (issue 467).
    #[error(
        "The session lies in the project {0}, which is deleted: it comes back with the project"
    )]
    ProjectDeleted(ProjectId),
    /// What lies in the scope of a session or channel being deleted is not
    /// all the caller's to delete (issue 320): what was is deleted, the
    /// session or channel is kept.
    #[error(transparent)]
    ScopeNotEmpty(#[from] ScopeNotEmpty),
    /// What the executor of procedures failed with: the sessions are shut down as the process
    /// ends (issue 297), and none is started any more, among it (issue 593).
    #[error(transparent)]
    Procedure(#[from] ProcedureError),
    /// What a store reported.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// What anything else a procedure of a session asked failed with - a template, a workspace,
    /// a channel -, judged further along its chain.
    #[error("{0}")]
    Failed(#[source] anyhow::Error),
    /// The session runs in another process that shares the stores (issue 554): it is not started,
    /// deleted, restored or purged here meanwhile.
    #[error("The session {0} runs in another process")]
    RunsElsewhere(SessionId),
}

impl SessionError {
    /// The error as the building block a procedure carried out failed, for whoever looks into it
    /// as it did before the procedure answered it (issue 593): what a store reported and what
    /// anything else failed with as they are - a `ChannelError` among them -, the session's own as
    /// it is.
    pub fn untyped(self) -> anyhow::Error {
        match self {
            SessionError::Store(error) => error.into(),
            SessionError::Failed(error) => error,
            error => error.into(),
        }
    }
}

/// What a building block of a session, which answers `anyhow`, failed with, as a procedure of the
/// session answers it (issue 593): the session's own error or the store's as they are, anything
/// else as `Failed`.
impl From<anyhow::Error> for SessionError {
    fn from(error: anyhow::Error) -> Self {
        let error = match error.downcast::<SessionError>() {
            Ok(own) => return own,
            Err(error) => error,
        };
        match error.downcast::<StoreError>() {
            Ok(store) => SessionError::Store(store),
            Err(error) => SessionError::Failed(error),
        }
    }
}

/// Nothing if a channel may be given `classification` at all - it names
/// only configured compartments (issues 133, 304) - checked before anything
/// is stored.
pub(crate) fn require_classification(
    policy: &dyn ConfidentialityPolicy,
    classification: &Classification,
) -> Result<()> {
    policy.require_classifiable(classification, || {
        SessionError::ClassificationRefused(classification.clone())
    })?;
    Ok(())
}

/// A channel the session lists, as the running session holds it.
#[derive(Debug)]
enum ChannelEntry {
    /// Built, and once `run` started it, with its task.
    Running { channel: Arc<ChannelLoop>, task: Option<AbortHandle> },
    /// Left out as the session opened (issue 463): its components cannot be read. Held with its
    /// document as it was then; a channel that does not run is changed only as someone is let go
    /// from the session (issue 590), and its copy renewed with it
    /// (`procedure::participant::let_go_from_session`).
    Unavailable { channel: Arc<Channel>, cause: Arc<ComponentError> },
}

/// How the actors of a session end as it stops (`SessionManager::stop_running`): stopped, to be
/// resumed when it runs again, or leaving for good, as it is deleted (issue 377).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ending {
    /// Stopped, each keeping what it is in the middle of.
    Stop,
    /// Leaving, as from a deleted channel.
    Leave,
}

/// Whether a channel the running session lists runs (issue 463).
#[derive(Debug, Clone)]
pub(crate) enum ChannelRun {
    /// It runs: its loop is where an operation goes.
    Running(Arc<ChannelLoop>),
    /// It was left out as the session opened: its components cannot be read.
    Unavailable(Arc<ComponentError>),
}

/// A channel the running session lists: its document, and whether it runs.
pub(crate) type Listed = (Arc<Channel>, ChannelRun);

impl ChannelEntry {
    fn listed(&self) -> Listed {
        match self {
            Self::Running { channel, .. } => {
                (channel.channel(), ChannelRun::Running(Arc::clone(channel)))
            }
            Self::Unavailable { channel, cause } => {
                (Arc::clone(channel), ChannelRun::Unavailable(Arc::clone(cause)))
            }
        }
    }

    fn running(&self) -> Option<&Arc<ChannelLoop>> {
        match self {
            Self::Running { channel, .. } => Some(channel),
            Self::Unavailable { .. } => None,
        }
    }
}

#[derive(Debug)]
pub struct SessionLoop {
    id: SessionId,
    session_controller: Arc<SessionController>,
    /// The workspace attached to the session, held live while it runs
    /// (issue 344); none if none is, or it is gone.
    workspace: ArcSwapOption<WorkspaceController>,
    /// What builds its channels' runtimes and the views of it.
    kernel: Arc<Kernel>,
    /// The services it works with, taken from `kernel` once (issue
    /// 229).
    policies: Policies,
    controllers: Arc<ControllerManager>,
    /// What a workspace attached to it is reached by.
    workspaces: Arc<WorkspaceManager>,
    events: Arc<EventBus>,
    /// The user commands of every channel of the session.
    commands: Arc<CommandExecutor>,
    /// The session's channels, and which of them run (issues 116, 463). A
    /// channel deleted while it waits to be started has no entry any more, so
    /// `run` never starts it.
    channels: DashMap<ChannelId, ChannelEntry>,
    /// Channels to start. `run` starts them, those of the session as well as
    /// those created later, so that all of them run in its `JoinSet` - and
    /// stop with it when the session is stopped.
    start_tx: mpsc::UnboundedSender<ChannelId>,
    /// Taken by `run`, which is how it knows it is not already running.
    start_rx: Mutex<mpsc::UnboundedReceiver<ChannelId>>,
    /// The events about its participants' names, subscribed to as the session is made, so that
    /// nothing of whom it takes in before it runs is missed; taken by `run`, which follows them.
    /// Whether its agents exist the follower of the agents follows (`procedure::follower`, issue
    /// 593); taking them into their controllers is the `ControllerManager`'s (issue 121).
    subscription: Mutex<EventSubscription>,
    /// What the participants it takes in are followed by on `subscription` (issue 590), from
    /// wherever it takes them in.
    following: Following,
}

/// `error` of the store about session `id`: that there is none - or none
/// the caller may see - is the session's own error, what the request named
/// and did not find (issue 323).
pub(crate) fn store_error(id: SessionId) -> impl FnOnce(StoreError) -> Error {
    move |error| match error {
        StoreError::NotFound { .. } => SessionError::NotFound(id).into(),
        error => error.into(),
    }
}

/// Asks whether `context` may attach the workspace `id` to the session `session_id` (issue 253):
/// theirs to attach as `WorkspaceManager::reached_to_attach` reaches it, it must lie where the
/// session may use it, not in another session's scope. Whether they may change the session is the
/// caller's to ask - or not, for a session they are making (issue 266).
///
/// # Errors
/// What `WorkspaceManager::reached_to_attach` reports; `SessionError::WorkspaceOutOfScope`.
pub(crate) async fn require_attachable(
    workspaces: &WorkspaceManager,
    context: &SecurityContext,
    id: WorkspaceId,
    session_id: SessionId,
) -> Result<()> {
    let shared = workspaces.reached_to_attach(context, id).await?;
    if !shared.name.scope.admits(&Scope::SessionScope { session_id }) {
        return Err(SessionError::WorkspaceOutOfScope(id).into());
    }
    Ok(())
}

/// The controller of the workspace `id` attached to session `session_id`,
/// to hold while it runs (issue 344); none if none is attached, or it is
/// gone - the session reads it as absent.
///
/// # Errors
/// Whatever else the store reports.
async fn held_workspace(
    controllers: &ControllerManager,
    session_id: SessionId,
    id: Option<WorkspaceId>,
) -> StoreResult<Option<Arc<WorkspaceController>>> {
    let Some(id) = id else { return Ok(None) };
    let held = controllers.workspace_controller(id).await.optional()?;
    if held.is_none() {
        tracing::warn!(session = %session_id, workspace = %id, "The workspace attached to a session is gone");
    }
    Ok(held)
}

impl SessionLoop {
    /// What `channel` runs with: the one place a `ChannelRuntime` is made, so that a channel is
    /// given everything of its session, and its own controller, as it is built (issues 361, 374).
    fn channel_runtime(self: &Arc<Self>, channel: Channel) -> ChannelRuntime {
        let channel = self.controllers.new_channel_controller(channel);
        ChannelRuntime::new(
            Arc::clone(&self.kernel),
            Arc::clone(&self.commands),
            Arc::clone(&self.session_controller),
            channel,
            Arc::downgrade(self),
        )
    }

    /// The loop of `session`, with its `channels` - documents of their own
    /// (issue 70), in the order the session lists them - each built when
    /// the session exists and given it, so that a channel reaches the
    /// others of its session (issues 17, 123, 361). A channel whose
    /// components cannot be read is left out (`add_listed_channel`, issue
    /// 463).
    ///
    /// # Errors
    /// Whatever building a channel reports otherwise - a store that failed
    /// among it -, and whatever making the session's commands or reading its
    /// workspace reports.
    pub(crate) async fn new(
        session: Session,
        channels: Vec<Channel>,
        kernel: Arc<Kernel>,
    ) -> Result<Arc<Self>> {
        let id = session.id;
        let commands = Arc::new(kernel.new_command_executor(&session).await?);
        let workspace = held_workspace(kernel.controller_manager(), id, session.workspace).await?;
        let topics = session.participants.iter().map(SubscriptionTopic::of_participant).collect();
        let subscription = kernel.event_bus().subscribe(SecurityContext::system(), topics);
        let following = subscription.following();
        let session_controller = kernel.controller_manager().new_session_controller(session);
        let (start_tx, start_rx) = mpsc::unbounded_channel();

        let session_loop = Arc::new(Self {
            id,
            session_controller,
            workspace: ArcSwapOption::new(workspace),
            policies: kernel.policies().clone(),
            controllers: Arc::clone(kernel.controller_manager()),
            workspaces: Arc::clone(kernel.workspace_manager()),
            events: Arc::clone(kernel.event_bus()),
            kernel,
            commands,
            channels: DashMap::new(),
            start_tx,
            start_rx: Mutex::new(start_rx),
            subscription: Mutex::new(subscription),
            following,
        });
        for channel in channels {
            session_loop.add_listed_channel(channel).await?;
        }
        Ok(session_loop)
    }

    /// The views of this session alone, as `viewer`, reached through its loop: an agent's in it
    /// (issue 163), or those of whoever issued a command in one of its channels (issue 555).
    pub(crate) fn views(self: &Arc<Self>, viewer: Viewer) -> Views {
        Views::of_own_session(&self.kernel, Arc::clone(self), viewer)
    }

    pub fn id(&self) -> SessionId {
        self.session_controller.id()
    }

    /// The session's controller: its document live, through which it is changed and stored, each
    /// change asked by the store as it is stored - its name, language, time zone, privacy, rules of
    /// approval and description; what changes its channels, its participants, its project or its
    /// workspace is the crate's, written by the procedures and the session.
    pub fn controller(&self) -> &Arc<SessionController> {
        &self.session_controller
    }

    /// The session as it is now: a snapshot, to be read or handed on to an operation - not to be
    /// kept, since it does not follow later changes.
    pub fn session(&self) -> Arc<Session> {
        Arc::clone(&self.session_controller.load())
    }

    /// The session as privacy sees it now (issue 134).
    pub fn privacy(&self) -> SessionPrivacy {
        SessionPrivacy::of(&self.session())
    }

    /// The executor of the kernel, for an entry point that reaches the session from within it - a
    /// command run in one of its channels, a plugin's writer bound to it - to carry out the
    /// procedures of its participants (issue 593); never for a building block of one.
    pub fn procedures(&self) -> &Arc<ProcedureExecutor> {
        self.kernel.procedures()
    }

    pub fn name(&self) -> Name {
        self.session_controller.name()
    }

    /// The workspace attached to the session, as it holds it live (issue 344).
    ///
    /// # Errors
    /// `NotUsableInChannel` if none is, or it is gone: nothing of a workspace is usable in the
    /// session's channels then.
    pub fn workspace(&self) -> WorkspaceResult<Arc<WorkspaceController>> {
        self.workspace.load_full().ok_or(WorkspaceError::NotUsableInChannel)
    }

    /// Follows `participant` from now on, on the session's subscription (issue 590) - its names,
    /// and its deletion, which its actor follows once it is taken in (`participant::Follow`) -, so
    /// that nothing of it is missed until it is listed: for a procedure that takes them in, as it
    /// reads them (`procedure::participant::take_into_session`). A topic followed already is not
    /// followed twice.
    pub(crate) fn follow(&self, participant: &Participant) {
        self.events.follow(&self.following, SubscriptionTopic::of_participant(participant));
    }

    /// Runs the session's channels, those it has and those created while it runs, until the session
    /// is stopped - which aborts this, and with it every channel (issue 593).
    ///
    /// Its channels follow the names of its participants: told of a participant renamed, and after
    /// it lost events (issue 585). Whether its agents exist they follow by the follower of the
    /// agents (`procedure::follower::AgentFollower`, issue 593), which changes no lifecycle of
    /// theirs from here.
    ///
    /// # Errors
    /// If it runs already, or a channel panicked, which ends it with every channel - logged by
    /// whoever spawned it. Nothing else follows: the session stays registered as running, its
    /// channels listed, none of them running, until it is stopped, which announces them stopped
    /// only then.
    pub async fn run(&self) -> Result<Infallible> {
        let mut start_rx = self.start_rx.try_lock().map_err(|_| anyhow!("Already running"))?;
        let mut subscription =
            self.subscription.try_lock().map_err(|_| anyhow!("Already running"))?;

        let mut join_set = JoinSet::new();
        let mut following = true;

        loop {
            tokio::select! {
                Some(channel_id) = start_rx.recv() => {
                    self.start_channel(channel_id, &mut join_set);
                }
                // A channel's task ends as it is aborted - deleted, or abandoned as its founders
                // could not join -, which is let be; a `run` that failed was logged by
                // `start_channel` and ends its task without an error; a channel that panicked
                // ends the session, and with it every channel.
                Some(Err(e)) = join_set.join_next(), if !join_set.is_empty() => {
                    if !e.is_cancelled() {
                        return Err(e.into());
                    }
                }
                received = subscription.receiver().recv(), if following => {
                    match received {
                        Ok(global_event) => self.handle_global_event(&global_event),
                        Err(RecvError::Lagged(lost)) => {
                            tracing::warn!("Session {} lost {} events and looks at its participants' names again", self.id, lost);
                            // One renamed meanwhile is mentioned by its name (issue 585). Whom it
                            // takes in it follows as it takes them in, which loses nothing.
                            self.rebuild_mentions();
                        }
                        // Not while the session holds the bus.
                        Err(RecvError::Closed) => following = false,
                    }
                }
            }
        }
    }

    fn handle_global_event(&self, global_event: &GlobalEvent) {
        if let Some((participant, name)) = renamed(global_event) {
            self.participant_renamed(participant, &name);
        }
    }

    /// Starts the channel `id` in `join_set` - unless it was deleted while
    /// it waited to be started. The entry stays locked while its task is
    /// set, so that `remove_channel` either finds the task or no channel.
    fn start_channel(&self, id: ChannelId, join_set: &mut JoinSet<()>) {
        let Some(mut entry) = self.channels.get_mut(&id) else { return };
        let ChannelEntry::Running { channel, task } = &mut *entry else { return };
        let channel = Arc::clone(channel);
        self.events.announce(
            GlobalEvent::custom::<Channel>(&(self.id, id), channel::STARTED, &()),
            &*channel.channel(),
        );
        let session = self.id;
        *task = Some(join_set.spawn(async move {
            let Err(error) = channel.run().await;
            tracing::warn!("Channel {id} of session {session} could not run: {error:#}");
        }));
    }

    /// Announces every channel it started stopped (`channel::STOPPED`), as `channel::STARTED`
    /// announced it, for a session whose task has ended - and with it every channel's aborted -,
    /// and takes their tasks: a channel deleted meanwhile is not announced stopped again.
    fn announce_channels_stopped(&self) {
        for mut entry in self.channels.iter_mut() {
            let id = *entry.key();
            if let ChannelEntry::Running { channel, task } = entry.value_mut() {
                if task.take().is_some() {
                    self.events.announce(
                        GlobalEvent::custom::<Channel>(&(self.id, id), channel::STOPPED, &()),
                        &*channel.channel(),
                    );
                }
            }
        }
    }

    /// Attaches `workspace` to the session, or with `None` takes the one
    /// attached away (issue 253), on behalf of `context`. Whoever may change
    /// the session asks first (`UpdateSession`); to attach one, it must be
    /// theirs to share (`GetWorkspace`, `UpdateWorkspace` - its owner,
    /// issue 251, decision 12) and lie where this session may use it - not
    /// in another session's scope. Taking it away shares nothing and asks
    /// no more. Its channels use it as their classifications and switches
    /// let them, from their next use - and as its access list lets each
    /// participant (issue 23): attaching shares nothing by itself, unless `share`
    /// says how far the session's participants may use it, which adds an entry
    /// for them. Taking it away leaves the list as it is: it is the
    /// workspace's, and its owner's to change.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if `context` may not change the session; what
    /// `require_attachable` reports of the workspace - `WorkspaceError::NotFound` if there is none
    /// they may read or are cleared for, `WorkspaceError::Store(StoreError::PermissionDenied)` if
    /// it is not theirs to attach, `SessionError::WorkspaceOutOfScope` -; or whatever else storing
    /// reports.
    pub async fn update_workspace(
        &self,
        context: &SecurityContext,
        workspace: Option<WorkspaceId>,
        share: Option<Grant>,
    ) -> Result<()> {
        let session = self.session();
        self.policies
            .access_policy()
            .require(
                context,
                ActionType::update(Session::KIND, &session.security_attributes),
                StoreError::PermissionDenied,
            )
            .await?;
        if let Some(id) = workspace {
            require_attachable(&self.workspaces, context, id, self.id).await?;
        }
        let held = self.attach_workspace(context, workspace).await?;
        if let (Some(controller), Some(grant)) = (held, share) {
            controller.share(context, Subject::Group(GroupId::of_session(self.id)), grant).await?;
        }
        info!(session = %self.id, workspace = ?workspace, by = %context.caller_id, "Session's workspace changed");
        Ok(())
    }

    /// Attaches `workspace` to the session on behalf of `context`, or with
    /// `None` takes the one attached away, and holds it live (issue 344):
    /// stored as changing the session, without asking whether it may be
    /// attached - its caller asked that (`update_workspace`), or found or made
    /// it for the session as it made the session (`SessionManager`).
    /// Returns the workspace as it is held.
    ///
    /// # Errors
    /// Whatever storing the session or reading the workspace reports.
    pub(crate) async fn attach_workspace(
        &self,
        context: &SecurityContext,
        workspace: Option<WorkspaceId>,
    ) -> Result<Option<Arc<WorkspaceController>>> {
        self.session_controller.update_workspace(context, workspace).await?;
        let held = held_workspace(&self.controllers, self.id, workspace).await?;
        self.workspace.store(held.clone());
        Ok(held)
    }
}

/// The participant `global_event` announces renamed, with its new name (issue 585); `None` if it
/// announces no rename, or one to a name that is none.
fn renamed(global_event: &GlobalEvent) -> Option<(ActorId, Name)> {
    let (participant, name) = if let Some((id, change)) = global_event.about::<Agent>() {
        let AgentUpdateEvent::NameChanged(name) = change.update::<AgentUpdateEvent>()?;
        (ActorId::Agent(id), name)
    } else {
        let (id, change) = global_event.about::<User>()?;
        let Some(UserUpdateEvent::NameChanged(name)) = change.update::<UserUpdateEvent>() else {
            return None;
        };
        (ActorId::User(id), name)
    };
    Some((participant, Name::new(name).ok()?))
}

#[cfg(test)]
mod tests {
    use crate::kernel::Kernel;
    use crate::session::testing::*;
    use crate::session::SessionManager;
    use crate::view::Viewer;
    use caelumora_api::actor::Actor;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::testing::make_personality;
    use caelumora_domain::agent::{Agent, AgentUpdateEvent};
    use caelumora_domain::channel::HistoryAccess;
    use caelumora_domain::event::GlobalEvent;
    use caelumora_domain::security::SecurityAttributes;
    use caelumora_domain::security::{Grant, Subject};
    use caelumora_domain::session::testing::make_session;
    use caelumora_domain::session::Session;
    use caelumora_domain::store::StoreError;
    use caelumora_domain::user::testing::make_user;
    use caelumora_domain::user::{User, UserUpdateEvent};
    use caelumora_security::SecurityContext;
    use caelumora_store::testing::record_users;
    use caelumora_store::testing::stored_agent;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::time;

    /// Announces `event` in `kernel`, as whoever renames a participant does, until `done`; whether
    /// it is done within two seconds.
    async fn done_once_renamed(
        kernel: &Kernel,
        event: GlobalEvent,
        done: impl Fn() -> bool,
    ) -> bool {
        let renamed = async {
            loop {
                kernel.event_bus().publish(event.clone(), SecurityAttributes::systems());
                if done() {
                    break;
                }
                time::sleep(Duration::from_millis(10)).await;
            }
        };
        time::timeout(Duration::from_secs(2), renamed).await.is_ok()
    }

    #[tokio::test]
    async fn a_renamed_user_is_renamed_in_every_channel_of_the_session() {
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let ActorId::User(owner) = ctx.initiator_id else { unreachable!() };
        let first = session.create_channel(&ctx, spec("first", vec![other])).await.unwrap();
        let second = session.create_channel(&ctx, spec("second", vec![other])).await.unwrap();

        // Announced about the user, as whoever changes users will: the `ControllerManager` takes it
        // into the one controller both actors share.
        let event = UserUpdateEvent::NameChanged("renamed".to_string());
        let event = GlobalEvent::updated::<User>(&owner, &event);
        let renamed = || {
            [&first, &second]
                .iter()
                .all(|c| c.user_actor(owner).expect("takes part").name() == "renamed")
        };
        assert!(
            done_once_renamed(&kernel, event, renamed).await,
            "both channels show the new name"
        );
        assert_eq!(
            first.user_actor(other.as_user_id().unwrap()).unwrap().name(),
            "other",
            "nobody else is renamed"
        );
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    /// Issue 585: a participant renamed while it takes part is mentioned by the handle of its new
    /// name, and no longer by its old one, in every channel of the session it takes part in - an
    /// agent and a user alike, without anyone joining or leaving.
    #[tokio::test]
    async fn a_renamed_participant_is_mentioned_by_its_new_name_in_every_channel() {
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let ActorId::User(other_id) = other else { unreachable!() };
        let stores = kernel.entity_stores();
        let helper = stored_agent(stores, "helper", make_personality("plain"), ctx.caller_id).await;
        let agent = ActorId::Agent(helper.id);
        let main = Arc::clone(&session.channels()[0]);
        let into = Some((main.id(), HistoryAccess::All));
        session.add_participant(&ctx, agent, into).await.expect("taken in");
        let side = session.create_channel(&ctx, spec("side", vec![other, agent])).await.unwrap();
        let channels = [&*main, &*side];
        assert!(channels.iter().all(|c| c.mentions().resolve("@helper") == [agent]), "as named");

        let mentioned =
            |handle: &str, actor| channels.iter().all(|c| c.mentions().resolve(handle) == [actor]);

        let event = AgentUpdateEvent::NameChanged("Assistant".to_string());
        let event = GlobalEvent::updated::<Agent>(&helper.id, &event);
        let by_new_name = || mentioned("@assistant", agent);
        assert!(done_once_renamed(&kernel, event, by_new_name).await, "the agent by its new name");
        let event = UserUpdateEvent::NameChanged("Bea".to_string());
        let event = GlobalEvent::updated::<User>(&other_id, &event);
        let by_new_name = || mentioned("@bea", other);
        assert!(done_once_renamed(&kernel, event, by_new_name).await, "the user by their new name");
        for old in ["@helper", "@other"] {
            assert!(channels.iter().all(|c| c.mentions().resolve(old).is_empty()), "{old}");
        }
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    /// Issue 585: a participant renamed among more events than the session's subscription holds is
    /// mentioned by its new name all the same, its announcement lost.
    #[tokio::test]
    async fn a_participant_renamed_while_events_were_lost_is_mentioned_by_its_new_name() {
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let stores = kernel.entity_stores();
        let helper = stored_agent(stores, "helper", make_personality("plain"), ctx.caller_id).await;
        let agent = ActorId::Agent(helper.id);
        let main = Arc::clone(&session.channels()[0]);
        let into = Some((main.id(), HistoryAccess::All));
        session.add_participant(&ctx, agent, into).await.expect("taken in");
        time::sleep(Duration::from_millis(50)).await;

        let controller = kernel.controller_manager().agent_controller(helper.id).await.unwrap();
        controller.update_name(&ctx, "Assistant").await.expect("renamed");
        // More than a subscription holds, on a topic the session follows and published before it
        // runs again: the announcement of the rename is lost.
        kernel.event_bus().publish_past_capacity(&GlobalEvent::created::<Agent>(&helper.id));
        let mentioned = async {
            while main.mentions().resolve("@assistant") != [agent] {
                time::sleep(Duration::from_millis(10)).await;
            }
        };
        assert!(time::timeout(Duration::from_secs(2), mentioned).await.is_ok(), "by its new name");
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn only_the_owner_changes_a_sessions_privacy_and_everyone_in_it_sees_it() {
        use caelumora_domain::privacy::PrivacyConfig;
        use caelumora_domain::session::SessionUpdateEvent;
        use caelumora_domain::user::UserId;

        let (_, manager) = owned_kernel(Default::default());
        let kernel = Arc::clone(manager.kernel());
        let (alice, bob) = (make_user("alice"), make_user("bob"));
        let as_user = |id: UserId| {
            SecurityContext::new(ActorId::User(id), Default::default(), Default::default())
        };
        let mut session = make_session("hers", alice.id);
        session.security_attributes = session
            .security_attributes
            .clone()
            .granting(Subject::Actor(ActorId::User(bob.id)), Grant::Read);
        session.participants =
            record_users(manager.kernel().entity_stores(), &[&alice, &bob]).await;
        let session = create(&manager, &as_user(alice.id), session).await.unwrap();
        let mut events = kernel.event_bus().subscribe_all();

        let refused = session
            .controller()
            .update_privacy(&as_user(bob.id), PrivacyConfig::SharedAudience)
            .await
            .expect_err("a participant may not");
        assert!(matches!(refused, StoreError::PermissionDenied(_)), "{refused}");
        assert_eq!(session.privacy().policy, PrivacyConfig::Private, "unchanged");

        session
            .controller()
            .update_privacy(&as_user(alice.id), PrivacyConfig::SharedAudience)
            .await
            .expect("the owner may");
        assert_eq!(session.privacy().policy, PrivacyConfig::SharedAudience);
        let stored = kernel
            .entity_stores()
            .sessions
            .get(&SecurityContext::system(), session.id())
            .await
            .unwrap();
        assert_eq!(stored.privacy, PrivacyConfig::SharedAudience, "stored");
        let shared = |event: &GlobalEvent| {
            let about = event.about::<Session>().filter(|(id, _)| *id == session.id());
            let update = about.and_then(|(_, change)| change.update::<SessionUpdateEvent>());
            matches!(
                update,
                Some(SessionUpdateEvent::PrivacyChanged(PrivacyConfig::SharedAudience))
            )
        };
        assert!(events.drain().await.iter().any(shared), "announced");
        let for_bob =
            manager.views(Viewer::new(as_user(bob.id))).session(session.id()).await.unwrap();
        assert_eq!(
            for_bob.privacy().policy,
            PrivacyConfig::SharedAudience,
            "shown to everyone who takes part"
        );
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }
}
