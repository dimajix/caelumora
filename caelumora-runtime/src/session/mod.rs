//! A running session: `SessionLoop`, how it is built and runs its channels.

use caelumora_model::value::name::Name;
use crate::channel::{ChannelLoop, Participation};
use crate::command::{CommandExecutor, ChannelCommandProvider};
use crate::runtime::{ChannelRuntime, RuntimeManager};
use crate::scoped::{purge_for_good, ScopeNotEmpty, ScopedEntities};
use crate::event::{EventBus, SubscriptionTopic};
use anyhow::{anyhow, Result};
use caelumora_model::actor::ActorId;
use caelumora_model::channel::{self, Channel, Participant};
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy, ConfidentialityQuestions};
use caelumora_model::event::{Announcing, GlobalEvent};
use caelumora_model::security::{ActionType, Enforcement, SecurityContext, AccessPolicy};
use crate::view::{Viewer, Views};
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::store::{EntityStores, StoreError, StoreResult, StoreResultExt, Stored};
use caelumora_model::scope::Scope;
use caelumora_model::session::{ChannelId, Session, SessionId, SessionUpdateEvent};
use caelumora_model::template::SessionTemplateId;
use caelumora_model::group::GroupId;
use crate::group::{keep_participants, GroupManager};
use caelumora_model::group::Group;
use caelumora_model::project::{Project, ProjectId};
use caelumora_model::workspace::{Workspace, WorkspaceId};
use crate::workspace::WorkspaceManager;
use crate::controller::{ControllerManager, SessionController, WorkspaceController};
use arc_swap::{ArcSwapOption, Guard};
use dashmap::DashMap;
use std::sync::Arc;
use tracing::warn;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc, Mutex};
use tokio::task::{AbortHandle, JoinSet};
use caelumora_model::agent::{Agent, AgentId};
use caelumora_model::event::EntityChange;
use crate::component::ComponentError;

mod channels;
mod manager;
mod participants;
mod settings;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use channels::ChannelSpec;
pub use manager::SessionManager;
pub(crate) use manager::Found;

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
    ChannelUnavailable { session: SessionId, channel: ChannelId, #[source] cause: Arc<ComponentError> },
    /// No classification that can be given (`require_classifiable`, issue
    /// 304): it names a compartment that is not configured.
    #[error("A channel cannot be classified {0}: a compartment is not configured or may not be given")]
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
    #[error("The session lies in the project {0}, which is deleted: it comes back with the project")]
    ProjectDeleted(ProjectId),
    /// What lies in the scope of a session or channel being deleted is not
    /// all the caller's to delete (issue 320): what was is deleted, the
    /// session or channel is kept.
    #[error(transparent)]
    ScopeNotEmpty(#[from] ScopeNotEmpty),
    /// The sessions are shut down as the process ends (issue 297): none is
    /// started any more.
    #[error("Caelumora is shutting down: no session is started")]
    ShuttingDown,
}

/// Nothing if a channel may be given `classification` at all - it names
/// only configured compartments (issues 133, 304) - checked before anything
/// is stored.
fn require_classification(runtime_manager: &RuntimeManager, classification: &Classification) -> Result<()> {
    runtime_manager.confidentiality().require_classifiable(classification, || SessionError::ClassificationRefused(classification.clone()))?;
    Ok(())
}

/// A channel the session lists, as the running session holds it.
#[derive(Debug)]
enum ChannelEntry {
    /// Built, and once `run` started it, with its task.
    Running { channel: Arc<ChannelLoop>, task: Option<AbortHandle> },
    /// Left out as the session opened (issue 463): its components cannot be read. Held with its
    /// document as it was then - a channel that does not run is not changed.
    Unavailable { channel: Arc<Channel>, cause: Arc<ComponentError> },
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
            Self::Running { channel, .. } => (Arc::clone(&channel.channel()), ChannelRun::Running(Arc::clone(channel))),
            Self::Unavailable { channel, cause } => (Arc::clone(channel), ChannelRun::Unavailable(Arc::clone(cause))),
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
    runtime_manager: Arc<RuntimeManager>,
    /// The services it works with, taken from `runtime_manager` once (issue
    /// 229).
    access_policy: Arc<dyn AccessPolicy>,
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    stores: EntityStores,
    controllers: Arc<ControllerManager>,
    /// Who it may take in, and who may take part in its channels (issue
    /// 356).
    participation: Arc<Participation>,
    events: Arc<EventBus>,
    /// What goes with a channel deleted or restored (issue 178).
    scoped: Arc<ScopedEntities>,
    /// What deletes and restores the group of a channel's participants with
    /// it (issue 348).
    groups: Arc<GroupManager>,
    /// The user commands of every channel of the session.
    commands: Arc<CommandExecutor<dyn ChannelCommandProvider>>,
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
    /// Held while the session is put into another project (issue 467), so
    /// that two such changes do not share and unshare projects across each
    /// other.
    moving: Mutex<()>,
}

/// Removes the document of channel `channel_id` for good, in the runtime's
/// name, with the group of its participants: marked deleted first if it is
/// not - a channel of a session being purged, which is hidden with the
/// session, or one whose creation failed - since only what was deleted is
/// purged (issue 164).
async fn purge_channel_document(stores: &EntityStores, channel_id: ChannelId) -> StoreResult<bool> {
    purge_for_good(stores.groups.as_ref(), GroupId::of_channel(channel_id)).await?;
    purge_for_good(stores.channels.as_ref(), channel_id).await
}

/// The own project of session `id`, if it is there: the one in its scope, found by its name
/// (`Project::own_name`) in the runtime's name - none once it was lifted (issue 467).
///
/// # Errors
/// Whatever the store reports.
async fn load_own(stores: &EntityStores, id: SessionId) -> StoreResult<Option<Project>> {
    stores.projects.find_by_name(&SecurityContext::system(), &Project::own_name(id)).await.optional()
}

/// Makes `own`, a session's own project, with the group of its members, in the runtime's name
/// (issue 467) - both, or, if the group cannot be made, neither: as the session is made, or taken
/// out of a project once its own was lifted. Announcing it is the caller's.
///
/// # Errors
/// Whatever the store reports.
async fn make_own(stores: &EntityStores, own: &Project) -> StoreResult<()> {
    stores.projects.create(&SecurityContext::system(), own).await?;
    if let Err(error) = keep_participants(stores.groups.as_ref(), Group::of_project(own)).await {
        if let Err(undo) = purge_for_good(stores.projects.as_ref(), own.id).await {
            warn!(project = %own.id, "A session's own project whose group could not be made is left: {undo}");
        }
        return Err(error);
    }
    Ok(())
}

/// Asks whether `context` may attach the workspace `id` to the session
/// `session_id` (issue 253): reached as the views reach it (issue 312), it
/// must lie where the session may use it, not in another session's scope,
/// and be theirs to share (`UpdateWorkspace` - its owner, issue 251,
/// decision 12). Whether they may change the session is the caller's to
/// ask - or not, for a session they are making (issue 266).
///
/// # Errors
/// `WorkspaceError::NotFound` if there is no workspace they may read or
/// are cleared for, `SessionError::WorkspaceOutOfScope`,
/// `StoreError::PermissionDenied` if they may not share it.
async fn require_attachable(workspaces: &WorkspaceManager, access_policy: &dyn AccessPolicy, context: &SecurityContext, id: WorkspaceId, session_id: SessionId) -> Result<()> {
    let shared = workspaces.reached(context, id).await?;
    if !shared.name.scope.admits(&Scope::SessionScope { session_id }) {
        return Err(SessionError::WorkspaceOutOfScope(id).into());
    }
    access_policy.require(context, ActionType::update(Workspace::KIND, &shared.security_attributes), StoreError::PermissionDenied).await?;
    Ok(())
}

/// The controller of the workspace `id` attached to session `session_id`,
/// to hold while it runs (issue 344); none if none is attached, or it is
/// gone - the session reads it as absent.
///
/// # Errors
/// Whatever else the store reports.
async fn held_workspace(controllers: &ControllerManager, session_id: SessionId, id: Option<WorkspaceId>) -> StoreResult<Option<Arc<WorkspaceController>>> {
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
        ChannelRuntime::new(Arc::clone(&self.runtime_manager), Arc::clone(&self.commands), Arc::clone(&self.session_controller), channel, Arc::downgrade(self))
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
    pub(crate) async fn new(session: Session, channels: Vec<Channel>, runtime_manager: Arc<RuntimeManager>) -> Result<Arc<Self>> {
        let id = session.id;
        let commands = Arc::new(runtime_manager.new_command_executor(&session).await?);
        let workspace = held_workspace(runtime_manager.controller_manager(), id, session.workspace).await?;
        let session_controller = runtime_manager.controller_manager().new_session_controller(session);
        let (start_tx, start_rx) = mpsc::unbounded_channel();

        let session_loop = Arc::new(Self {
            id,
            session_controller,
            workspace: ArcSwapOption::new(workspace),
            access_policy: Arc::clone(runtime_manager.access_policy()),
            confidentiality: Arc::clone(runtime_manager.confidentiality()),
            stores: runtime_manager.entity_stores().clone(),
            controllers: Arc::clone(runtime_manager.controller_manager()),
            participation: Arc::clone(runtime_manager.participation()),
            events: Arc::clone(runtime_manager.event_bus()),
            scoped: Arc::clone(runtime_manager.scoped_entities()),
            groups: Arc::clone(runtime_manager.group_manager()),
            runtime_manager,
            commands,
            channels: DashMap::new(),
            start_tx,
            start_rx: Mutex::new(start_rx),
            moving: Mutex::new(()),
        });
        for channel in channels {
            session_loop.add_listed_channel(channel).await?;
        }
        Ok(session_loop)
    }

    /// The views of an agent in this session, as `viewer` (issue 163):
    /// this session alone, reached through its loop.
    pub(crate) fn views(self: &Arc<Self>, viewer: Viewer) -> Views {
        Views::of_own_session(&self.runtime_manager, Arc::clone(self), viewer)
    }

    pub fn id(&self) -> SessionId {
        self.session_controller.id()
    }

    /// The session as it is now.
    pub fn session(&self) -> Guard<Arc<Session>> {
        self.session_controller.load()
    }

    /// The session as privacy sees it now (issue 134).
    pub fn privacy(&self) -> SessionPrivacy {
        SessionPrivacy::of(&self.session())
    }

    pub fn name(&self) -> Name {
        self.session_controller.name()
    }

    /// The workspace attached to the session, as it holds it live (issue
    /// 344); `None` if none is, or it is gone.
    pub(crate) fn workspace(&self) -> Option<Arc<WorkspaceController>> {
        self.workspace.load_full()
    }

    /// Runs the session's channels, those it has and those created while it
    /// runs, until the session is stopped - which aborts this, and with it
    /// every channel.
    ///
    /// Its channels follow whether its agents exist (issue 314): told of an
    /// agent deleted or restored, and of every agent as it starts and after
    /// it lost events - so that nothing that happened meanwhile is missed.
    pub async fn run(&self) -> Result<()> {
        let mut start_rx = self.start_rx.try_lock()
            .map_err(|_| anyhow!("Already running"))?;

        let mut join_set = JoinSet::new();
        // The session's own events, and those about its agents' lifecycle;
        // the rest about its users and agents is the `ControllerManager`'s to
        // take in (issue 121). An agent taken in later is followed once that
        // is announced (issue 221).
        let mut topics = vec![SubscriptionTopic::Session(self.id())];
        topics.extend(self.session().participants.iter().filter_map(|p| match p {
            Participant::Agent(id) => Some(SubscriptionTopic::Agent(*id)),
            _ => None,
        }));
        let mut subscription = self.events.subscribe(topics);
        let mut following = true;
        self.follow_agents(None).await;

        loop {
            tokio::select! {
                Some(channel_id) = start_rx.recv() => {
                    self.start_channel(channel_id, &mut join_set);
                }
                Some(res) = join_set.join_next(), if !join_set.is_empty() => {
                    match res {
                        Ok(channel_id) => {
                            let stopped = self.channels.get_mut(&channel_id)
                                .and_then(|mut entry| match &mut *entry {
                                    ChannelEntry::Running { channel, task } => {
                                        *task = None;
                                        Some(Arc::clone(&channel.channel()))
                                    }
                                    ChannelEntry::Unavailable { .. } => None,
                                });
                            // Gone: a deleted channel, whose actors all
                            // stopped in time; `delete_channel` announces it.
                            if let Some(stopped) = stopped {
                                self.events.announce(GlobalEvent::custom::<Channel>(&(self.id, channel_id), channel::STOPPED, &()), &*stopped);
                            }
                        }
                        // A deleted channel; `delete_channel` announces it.
                        Err(e) if e.is_cancelled() => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                received = subscription.receiver().recv(), if following => {
                    match received {
                        Ok(global_event) => {
                            if let Some(agent) = self.agent_taken_in(&global_event) {
                                self.events.follow(&subscription, SubscriptionTopic::Agent(agent));
                            }
                            self.handle_global_event(&global_event).await
                        }
                        Err(RecvError::Lagged(lost)) => {
                            tracing::warn!("Session {} lost {} events and looks at its agents again", self.id, lost);
                            self.follow_agents(None).await;
                        }
                        // Not while the session holds the bus.
                        Err(RecvError::Closed) => following = false,
                    }
                }
            }
        }
    }

    /// The agent taken into this session, if `global_event` announces one
    /// (issue 221).
    fn agent_taken_in(&self, global_event: &GlobalEvent) -> Option<AgentId> {
        let (id, change) = global_event.about::<Session>()?;
        match change.update::<SessionUpdateEvent>() {
            Some(SessionUpdateEvent::ParticipantJoined(Participant::Agent(agent))) if id == self.id => Some(agent),
            _ => None,
        }
    }

    async fn handle_global_event(&self, global_event: &GlobalEvent) {
        match global_event.about::<Agent>() {
            Some((id, EntityChange::Deleted | EntityChange::Restored)) => self.follow_agents(Some(id)).await,
            _ => self.session_controller.process_global_event(global_event),
        }
    }

    /// Starts the channel `id` in `join_set` - unless it was deleted while
    /// it waited to be started. The entry stays locked while its task is
    /// set, so that `delete_channel` either finds the task or no channel.
    fn start_channel(&self, id: ChannelId, join_set: &mut JoinSet<ChannelId>) {
        let Some(mut entry) = self.channels.get_mut(&id) else { return };
        let ChannelEntry::Running { channel, task } = &mut *entry else { return };
        let channel = Arc::clone(channel);
        self.events.announce(GlobalEvent::custom::<Channel>(&(self.id, id), channel::STARTED, &()), &**channel.channel());
        *task = Some(join_set.spawn(async move {
            let _ = channel.run().await;
            channel.id()
        }));
    }
}

#[cfg(test)]
mod tests {

    

    
    use caelumora_model::event::GlobalEvent;
    use caelumora_model::security::SecurityContext;

    
    use std::sync::Arc;

    

    

    use crate::session::SessionManager;
    use crate::session::testing::*;
    use caelumora_model::user::User;
    use std::time::Duration;
    use tokio::time;

    #[tokio::test]
    async fn a_renamed_user_is_renamed_in_every_channel_of_the_session() {
        use crate::actor::Actor;
        use caelumora_model::actor::ActorId;
        use caelumora_model::user::UserUpdateEvent;

        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let ActorId::User(owner) = ctx.initiator_id else { unreachable!() };
        let first = session.create_channel(&ctx, spec("first", vec![other])).await.unwrap();
        let second = session.create_channel(&ctx, spec("second", vec![other])).await.unwrap();

        // Announced about the user, as whoever changes users will: the
        // session takes it into the one controller both actors share.
        let renamed = async {
            loop {
                runtime.event_bus().publish(GlobalEvent::updated::<User>(&owner, &UserUpdateEvent::NameChanged("renamed".to_string())), Default::default());
                let names: Vec<String> = [&first, &second].iter().map(|c| c.user_actor(owner).expect("takes part").name().to_string()).collect();
                if names.iter().all(|n| n == "renamed") {
                    break;
                }
                time::sleep(Duration::from_millis(10)).await;
            }
        };
        time::timeout(Duration::from_secs(2), renamed).await.expect("both channels show the new name");
        assert_eq!(first.user_actor(other.as_user_id().unwrap()).unwrap().name(), "other", "nobody else is renamed");
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

}
