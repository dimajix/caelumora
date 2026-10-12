//! A running session: `SessionLoop`, how it is built and runs its channels.

use caelumora_model::value::name::Name;
use crate::channel::ChannelLoop;
use crate::command::{CommandExecutor, ChannelCommandProvider};
use crate::runtime::{ChannelRuntime, RuntimeManager};
use crate::scoped::ScopedEntities;
use crate::event::{EventBus, SubscriptionTopic};
use anyhow::{anyhow, Result};
use caelumora_model::actor::ActorId;
use caelumora_model::channel::{Channel, Participant};
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy, ConfidentialityQuestions};
use caelumora_model::event::GlobalEvent;
use caelumora_model::security::{ActionType, Enforcement, SecurityContext, SecurityManager};
use crate::view::{Viewer, Views};
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::store::{EntityKind, EntityStore, StoreError, StoreResult};
use caelumora_model::scope::Scope;
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::group::GroupId;
use crate::group::purge_participants;
use caelumora_model::security::Grant;
use caelumora_model::workspace::{Workspace, WorkspaceId};
use caelumora_model::removal::Stored;
use crate::workspace::WorkspaceManager;
use crate::controller::SessionController;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc, Mutex};
use tokio::task::{AbortHandle, JoinSet};

mod channels;
mod manager;
mod settings;
#[cfg(test)]
pub(crate) mod testing;

pub use channels::ChannelSpec;
pub use manager::SessionManager;
pub(crate) use manager::Found;

/// A request for a channel that cannot be met as asked (issue 09). The
/// messages are meant for users (see `crate::error::user_message`).
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The channel's components do not make a moderator (issue 174): a kind
    /// that does not exist, two moderators, two policies. Found when the
    /// channel is built; a channel created so is taken back out.
    #[error("The channel's moderator cannot be built: {0}")]
    InvalidModerator(#[source] crate::component::ComponentError),
    #[error("{participant} is not a participant of session {session}")]
    NotAParticipant { session: SessionId, participant: ActorId },
    /// Only a user owns a session made from a template (issue 88).
    #[error("{0} cannot own a session: only a user can")]
    OwnerNotAUser(ActorId),
    #[error("Session {session} has no channel {channel}")]
    ChannelNotFound { session: SessionId, channel: ChannelId },
    /// No classification that can be given (`require_classifiable`, issue
    /// 304): it names a compartment that is not configured.
    #[error("A channel cannot be classified {0}: a compartment is not configured or may not be given")]
    ClassificationRefused(Classification),
    /// The workspace lies in another session's scope, and is used within
    /// that session alone (issue 251, `Scope::admits`).
    #[error("The workspace {0} lies in another session's scope")]
    WorkspaceOutOfScope(caelumora_model::workspace::WorkspaceId),
    /// What lies in the scope of a session or channel being deleted is not
    /// all the caller's to delete (issue 320): what was is deleted, the
    /// session or channel is kept. `left` says how many of each kind are
    /// left - not which, since they need not be the caller's to read.
    #[error("The {} is not deleted: {} of what lies in its scope may not be deleted by you", shown_holder(.holder), shown_left(.left))]
    ScopeNotEmpty { holder: Scope, left: Vec<(EntityKind, usize)> },
}

/// The session or channel whose scope `holder` is, as an error names it.
fn shown_holder(holder: &Scope) -> String {
    match holder {
        Scope::SessionScope { session_id } => format!("session {session_id}"),
        Scope::ChannelScope { session_id, channel_id } => format!("channel {channel_id} of session {session_id}"),
        // Nothing else holds a scope that is deleted with it.
        _ => "holder of the scope".to_string(),
    }
}

/// How many of each kind are left, as an error says it: "1 agent, 2 artifact".
fn shown_left(left: &[(EntityKind, usize)]) -> String {
    left.iter().map(|(kind, count)| format!("{count} {kind}")).collect::<Vec<_>>().join(", ")
}

/// Nothing if a channel may be given `classification` at all - it names
/// only configured compartments (issues 133, 304) - checked before anything
/// is stored.
fn require_classification(runtime_manager: &RuntimeManager, classification: &Classification) -> Result<()> {
    runtime_manager.confidentiality().require_classifiable(classification, || SessionError::ClassificationRefused(classification.clone()))?;
    Ok(())
}

/// A channel of the session and, once `run` started it, its task.
#[derive(Debug)]
struct ChannelEntry {
    channel: Arc<ChannelLoop>,
    task: Option<AbortHandle>,
}

#[derive(Debug)]
pub struct SessionLoop {
    id: SessionId,
    session_controller: Arc<SessionController>,
    /// What builds its channels' runtimes and the views of it.
    runtime_manager: Arc<RuntimeManager>,
    /// The services it works with, taken from `runtime_manager` once (issue
    /// 229).
    security: Arc<dyn SecurityManager>,
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    entities: Arc<dyn EntityStore>,
    events: Arc<EventBus>,
    /// What goes with a channel deleted or restored (issue 178).
    scoped: Arc<ScopedEntities>,
    /// The user commands of every channel of the session.
    commands: Arc<CommandExecutor<dyn ChannelCommandProvider>>,
    /// The session's channels, and which of them run (issue 116). A channel
    /// deleted while it waits to be started has no entry any more, so `run`
    /// never starts it.
    channels: DashMap<ChannelId, ChannelEntry>,
    /// Channels to start. `run` starts them, those of the session as well as
    /// those created later, so that all of them run in its `JoinSet` - and
    /// stop with it when the session is stopped.
    start_tx: mpsc::UnboundedSender<ChannelId>,
    /// Taken by `run`, which is how it knows it is not already running.
    start_rx: Mutex<mpsc::UnboundedReceiver<ChannelId>>,
}

/// Removes the document of channel `channel_id` for good, in the runtime's
/// name, with the group of its participants: marked deleted first if it is
/// not - a channel of a session being purged, which is hidden with the
/// session, or one whose creation failed - since only what was deleted is
/// purged (issue 164).
async fn purge_channel_document(store: &dyn EntityStore, channel_id: ChannelId) -> StoreResult<bool> {
    let system = SecurityContext::system();
    if let Err(e) = purge_participants(store, GroupId::of_channel(channel_id)).await {
        if !matches!(e, StoreError::NotFound { .. }) {
            return Err(e);
        }
    }
    store.delete_channel(&system, channel_id).await?;
    store.purge_channel(&system, channel_id).await
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
async fn require_attachable(workspaces: &WorkspaceManager, security: &dyn SecurityManager, context: &SecurityContext, id: WorkspaceId, session_id: SessionId) -> Result<()> {
    let shared = workspaces.reached(context, id, Grant::Read).await?;
    if !shared.name.scope.admits(&Scope::SessionScope { session_id }) {
        return Err(SessionError::WorkspaceOutOfScope(id).into());
    }
    security.require(context, ActionType::update(Workspace::KIND, &shared.security_attributes), || StoreError::PermissionDenied).await?;
    Ok(())
}

impl SessionLoop {
    /// The loop of `session`, with its `channels` - documents of their own
    /// (issue 70), in the order the session lists them - each attached to
    /// it, so that a channel reaches the others of its session (issues 17,
    /// 123).
    pub async fn new(session: Session, channels: Vec<Channel>, runtime_manager: Arc<RuntimeManager>) -> Result<Arc<Self>> {
        let id = session.id;
        let commands = Arc::new(runtime_manager.new_command_executor(&session).await?);
        let session_controller = runtime_manager.controller_manager().new_session_controller(session);
        let (start_tx, start_rx) = mpsc::unbounded_channel();
        let channel_docs = channels;
        let channels = DashMap::new();
        for channel in channel_docs {
            let runtime = ChannelRuntime::new(Arc::clone(&runtime_manager), Arc::clone(&commands), Arc::clone(&session_controller));
            let cl = ChannelLoop::new(channel, runtime).await?;
            let id = cl.id();
            channels.insert(id, ChannelEntry { channel: cl, task: None });
            let _ = start_tx.send(id);
        }

        let session_loop = Arc::new(Self {
            id,
            session_controller,
            security: Arc::clone(runtime_manager.security_manager()),
            confidentiality: Arc::clone(runtime_manager.confidentiality()),
            entities: Arc::clone(runtime_manager.entity_store()),
            events: Arc::clone(runtime_manager.event_bus()),
            scoped: Arc::clone(runtime_manager.scoped_entities()),
            runtime_manager,
            commands,
            channels,
            start_tx,
            start_rx: Mutex::new(start_rx),
        });
        for entry in session_loop.channels.iter() {
            entry.channel.attach(&session_loop);
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
    pub fn session(&self) -> arc_swap::Guard<Arc<Session>> {
        self.session_controller.load()
    }

    /// The session as privacy sees it now (issue 134).
    pub fn privacy(&self) -> SessionPrivacy {
        SessionPrivacy::of(&self.session())
    }

    pub fn name(&self) -> Name {
        self.session_controller.name()
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
        // take in (issue 121). Its participants are fixed once it is made
        // (issue 221).
        let mut topics = vec![SubscriptionTopic::Session(self.id())];
        topics.extend(self.session().participants.iter().filter_map(|p| match p {
            Participant::Agent(id) => Some(SubscriptionTopic::Agent(*id)),
            _ => None,
        }));
        let mut global_rx = self.events.subscribe(topics).into_receiver();
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
                            let reach = self.channels.get_mut(&channel_id)
                                .map(|mut entry| {
                                    entry.task = None;
                                    (entry.channel.security_attributes(), entry.channel.classification())
                                });
                            // Gone: a deleted channel, whose actors all
                            // stopped in time; `delete_channel` announces it.
                            if let Some((attributes, classification)) = reach {
                                self.events.publish_classified(GlobalEvent::ChannelStopped(self.id, channel_id), attributes, &[classification]);
                            }
                        }
                        // A deleted channel; `delete_channel` announces it.
                        Err(e) if e.is_cancelled() => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                received = global_rx.recv(), if following => {
                    match received {
                        Ok(global_event) => self.handle_global_event(&global_event).await,
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

    async fn handle_global_event(&self, global_event: &GlobalEvent) {
        match global_event {
            GlobalEvent::AgentDeleted(id) | GlobalEvent::AgentRestored(id) => self.follow_agents(Some(*id)).await,
            _ => self.session_controller.process_global_event(global_event),
        }
    }

    /// Starts the channel `id` in `join_set` - unless it was deleted while
    /// it waited to be started. The entry stays locked while its task is
    /// set, so that `delete_channel` either finds the task or no channel.
    fn start_channel(&self, id: ChannelId, join_set: &mut JoinSet<ChannelId>) {
        let Some(mut entry) = self.channels.get_mut(&id) else { return };
        let channel = Arc::clone(&entry.channel);
        self.events.publish_classified(GlobalEvent::ChannelStarted(self.id, id), channel.security_attributes(), &[channel.classification()]);
        entry.task = Some(join_set.spawn(async move {
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
                runtime.event_bus().publish(GlobalEvent::UserUpdated(owner, UserUpdateEvent::NameChanged("renamed".to_string())), Default::default());
                let names: Vec<String> = [&first, &second].iter().map(|c| c.user_actor(owner).expect("takes part").name().to_string()).collect();
                if names.iter().all(|n| n == "renamed") {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(2), renamed).await.expect("both channels show the new name");
        assert_eq!(first.user_actor(other.as_user_id().unwrap()).unwrap().name(), "other", "nobody else is renamed");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

}
