use crate::channel::ChannelLoop;
use crate::command::CommandExecutor;
use crate::runtime::{ChannelRuntime, RuntimeManager};
use anyhow::{anyhow, Result};
use caelumora_model::actor::ActorId;
use caelumora_model::channel::{Channel, Participant, DEFAULT_MODERATOR};
use caelumora_model::confidentiality::Classification;
use caelumora_model::event::GlobalEvent;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, SecurityAttributes, SecurityContext};
use caelumora_model::view::{ChannelView, MemberView, ParticipantView, SessionView};
use caelumora_model::mention::handle_of;
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::store::StoreError;
use tracing::{info, warn};
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::template::SessionTemplateId;
use crate::controller::SessionController;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::task::{AbortHandle, JoinHandle, JoinSet};

/// A request for a channel that cannot be met as asked (issue 09). The
/// messages are meant for users (see `crate::error::user_message`).
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("A channel needs a name")]
    EmptyChannelName,
    #[error("Session {session} already has a channel named \"{name}\"")]
    DuplicateChannelName { session: SessionId, name: String },
    #[error("There is no moderator \"{0}\"")]
    UnknownModerator(String),
    #[error("{participant} is not a participant of session {session}")]
    NotAParticipant { session: SessionId, participant: ActorId },
    /// Only a user owns a session made from a template (issue 88).
    #[error("{0} cannot own a session: only a user can")]
    OwnerNotAUser(ActorId),
    #[error("Session {session} has no channel {channel}")]
    ChannelNotFound { session: SessionId, channel: ChannelId },
    /// Refused by `ActionType::Classify`: a compartment that is not
    /// configured, or one the policy does not let the creator give.
    #[error("A channel cannot be classified {0}: a compartment is not configured or may not be given")]
    ClassificationRefused(Classification),
}

/// Nothing if `context` may give a channel `classification`
/// (`ActionType::Classify`, issue 133) - asked before anything is stored.
async fn require_classification(runtime_manager: &RuntimeManager, context: &SecurityContext, classification: &Classification) -> Result<()> {
    runtime_manager.security_manager()
        .require(context, ActionType::Classify(classification), || SessionError::ClassificationRefused(classification.clone())).await?;
    Ok(())
}

/// Nothing if every participant of `channel` is cleared for its
/// classification (`RuntimeManager::require_cleared`, issue 133) - asked
/// before anything is stored.
async fn require_cleared(runtime_manager: &RuntimeManager, context: &SecurityContext, channel: &Channel) -> Result<()> {
    for participant in channel.participants.iter().map(|p| p.actor_id()).filter(|p| !matches!(p, ActorId::Approver(_))) {
        runtime_manager.require_cleared(context, participant, &channel.classification, channel.id).await?;
    }
    Ok(())
}

/// Nothing if the user `context` acts for is cleared for `channel` (issue
/// 133); `refusal` otherwise - reading a channel one is not cleared for is
/// the same as reading one that does not exist.
async fn require_seen<E: Into<anyhow::Error> + Send>(runtime_manager: &RuntimeManager, context: &SecurityContext, channel: &Classification, refusal: impl FnOnce() -> E + Send) -> Result<()> {
    let roles = runtime_manager.roles_of(context.initiator_id).await?;
    if runtime_manager.security_manager().may_see_channel(context, &roles, channel).await {
        Ok(())
    } else {
        Err(refusal().into())
    }
}

/// What a new channel should be (issue 09).
#[derive(Debug, Clone)]
pub struct ChannelSpec {
    pub name: String,
    /// The moderator kind; `None` for the default, as in a template.
    pub moderator: Option<String>,
    /// Who takes part, from the session's participants. Whoever creates the
    /// channel takes part anyway, if they are a participant of the session.
    pub participants: Vec<ActorId>,
    /// The classification of the channel's contents (issue 133); `None`
    /// for the default, as in a template. Set by whoever may create the
    /// channel - its owner.
    pub classification: Option<Classification>,
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
    runtime_manager: Arc<RuntimeManager>,
    /// The user commands of every channel of the session.
    commands: Arc<CommandExecutor>,
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

impl SessionLoop {
    /// The loop of `session`, with its `channels` - documents of their own
    /// (issue 70), in the order the session lists them - each attached to
    /// it, so that a channel reaches the others of its session (issues 17,
    /// 123).
    pub async fn new(session: Session, channels: Vec<Channel>, runtime_manager: Arc<RuntimeManager>) -> Result<Arc<Self>> {
        let id = session.id;
        let commands = Arc::new(runtime_manager.new_command_executor(&session).await?);
        let (start_tx, start_rx) = mpsc::unbounded_channel();
        let channel_docs = channels;
        let channels = DashMap::new();
        for channel in channel_docs {
            let runtime = ChannelRuntime::new(runtime_manager.clone(), commands.clone());
            let cl = ChannelLoop::new(channel, runtime).await?;
            let id = cl.id();
            channels.insert(id, ChannelEntry { channel: cl, task: None });
            let _ = start_tx.send(id);
        }

        let session_controller = runtime_manager.controller_manager().new_session_controller(session);

        let session_loop = Arc::new(Self {
            id,
            session_controller,
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

    pub fn name(&self) -> String {
        self.session_controller.name()
    }

    /// Renames the session on behalf of `context`.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// session, or whatever else storing it reports.
    pub async fn update_name(&self, context: &SecurityContext, new_name: String) -> Result<()> {
        Ok(self.session_controller.update_name(context, new_name).await?)
    }

    /// The running channels, in the order of the session: those from its
    /// template first, then those created later, oldest first.
    pub fn channels(&self) -> Vec<Arc<ChannelLoop>> {
        self.session().channels.iter().filter_map(|c| self.channel_by_id(*c)).collect()
    }

    pub fn channel_by_id(&self, channel_id: ChannelId) -> Option<Arc<ChannelLoop>> {
        self.channels.get(&channel_id).map(|entry| entry.channel.clone())
    }

    /// The channel `channel_id`, if `context` may read it (`GetChannel`,
    /// issues 70, 102) - the counterpart of `SessionManager::open_session`.
    ///
    /// # Errors
    /// `SessionError::ChannelNotFound` if there is no such channel, and
    /// equally if `context` may not read it, so that channels cannot be
    /// probed.
    pub async fn channel_for(&self, context: &SecurityContext, channel_id: ChannelId) -> Result<Arc<ChannelLoop>> {
        let not_found = || SessionError::ChannelNotFound { session: self.id, channel: channel_id };
        let channel = self.channel_by_id(channel_id).ok_or_else(not_found)?;
        self.runtime_manager.security_manager()
            .require(context, ActionType::GetChannel(&channel.security_attributes()), not_found).await?;
        require_seen(&self.runtime_manager, context, &channel.classification(), not_found).await?;
        Ok(channel)
    }

    pub fn channel_by_name(&self, name: &str) -> Option<Arc<ChannelLoop>> {
        self.channels.iter().find(|entry| entry.channel.name() == name).map(|entry| entry.channel.clone())
    }

    /// Creates a channel in the running session on behalf of `context` and
    /// starts it (issue 09). The channel is a document of its own (issue 70),
    /// stored first (`CreateChannel`), and then listed in the session, which
    /// takes the permission to change the session (`UpdateSession`); the
    /// channel belongs to the session's owner, like the session's others.
    ///
    /// # Errors
    /// - [`SessionError`] if `spec` asks for something that cannot be: no
    ///   name, a name the session already has, an unknown moderator, or a
    ///   participant who is not one of the session's.
    /// - `StoreError::PermissionDenied` if `context` may not change the
    ///   session; nothing is changed then.
    pub async fn create_channel(self: &Arc<Self>, context: &SecurityContext, spec: ChannelSpec) -> Result<Arc<ChannelLoop>> {
        let session = self.session();
        let name = spec.name.trim().to_string();
        if name.is_empty() {
            return Err(SessionError::EmptyChannelName.into());
        }
        if self.channels.iter().any(|entry| entry.channel.name() == name) {
            return Err(SessionError::DuplicateChannelName { session: self.id, name }.into());
        }
        let classification = spec.classification.unwrap_or_default();
        require_classification(&self.runtime_manager, context, &classification).await?;
        let moderator = spec.moderator.unwrap_or_else(|| DEFAULT_MODERATOR.to_string());
        // Asked for here, before anything is stored, rather than found out
        // when the channel is built.
        self.runtime_manager.new_moderator(&moderator)?;

        // The creator first, if they take part in the session at all.
        let mut ids = vec![context.initiator_id];
        ids.extend(spec.participants);
        let mut participants = Vec::new();
        for id in ids {
            if participants.iter().any(|p: &caelumora_model::channel::Participant| p.actor_id() == id) {
                continue;
            }
            match session.participants.iter().find(|p| p.actor_id() == id) {
                Some(participant) => participants.push(participant.clone()),
                None if id == context.initiator_id => {}
                None => return Err(SessionError::NotAParticipant { session: self.id, participant: id }.into()),
            }
        }

        let channel = Channel::build(self.id, session.security_attributes.owner_id, name, moderator, classification, participants);
        drop(session);
        require_cleared(&self.runtime_manager, context, &channel).await?;
        let store = self.runtime_manager.entity_store();
        store.create_channel(context, &channel).await?;
        if let Err(error) = self.session_controller.add_channel(context, channel.id).await {
            self.discard_channel(channel.id).await;
            return Err(error.into());
        }

        let runtime = ChannelRuntime::new(self.runtime_manager.clone(), self.commands.clone());
        let channel_loop = match ChannelLoop::new(channel.clone(), runtime).await {
            Ok(channel_loop) => channel_loop,
            Err(error) => {
                // Stored, but it cannot run: taken back out, so that the
                // session does not keep a channel that fails every start.
                if let Err(undo) = self.session_controller.remove_channel(context, channel.id).await {
                    warn!("Channel {} of session {} could not be built and not be removed again: {}", channel.id, self.id, undo);
                }
                self.discard_channel(channel.id).await;
                return Err(error);
            }
        };
        channel_loop.attach(self);
        self.channels.insert(channel.id, ChannelEntry { channel: channel_loop.clone(), task: None });
        self.runtime_manager.event_bus().publish(GlobalEvent::ChannelCreated(self.id, channel.id), channel.security_attributes.clone());
        let _ = self.start_tx.send(channel.id);
        info!("Channel {}/{} created in session {}", channel.name, channel.id, self.id);
        Ok(channel_loop)
    }

    /// Stops and deletes the channel `channel_id` on behalf of `context`,
    /// together with its state (issue 09). Asked about first, like creating
    /// one: a refused deletion leaves the channel running.
    ///
    /// # Errors
    /// - `SessionError::ChannelNotFound` if the session has no such channel.
    /// - `StoreError::PermissionDenied` if `context` may not change the
    ///   session; nothing is changed then.
    pub async fn delete_channel(&self, context: &SecurityContext, channel_id: ChannelId) -> Result<()> {
        let Some(running) = self.channel_by_id(channel_id) else {
            return Err(SessionError::ChannelNotFound { session: self.id, channel: channel_id }.into());
        };
        self.session_controller.remove_channel(context, channel_id).await?;
        // Not the guard itself: it would be held across the awaits below.
        let channel = Arc::clone(&running.channel());
        if let Err(e) = self.runtime_manager.entity_store().delete_channel(context, channel_id).await {
            warn!("Channel {} was taken out of session {} but not deleted: {}", channel_id, self.id, e);
        }

        let task = self.channels.remove(&channel_id).and_then(|(_, entry)| entry.task);
        running.stop_actors();
        let event_bus = self.runtime_manager.event_bus();
        if let Some(task) = task {
            task.abort();
            event_bus.publish(GlobalEvent::ChannelStopped(self.id, channel_id), channel.security_attributes.clone());
        }
        event_bus.publish(GlobalEvent::ChannelDeleted(self.id, channel_id), channel.security_attributes.clone());
        self.runtime_manager.state_store().delete_channel_state(channel_id).await?;
        info!("Channel {}/{} deleted from session {}", channel.name, channel_id, self.id);
        Ok(())
    }

    /// Deletes the stored channel `channel_id` again after creating it failed
    /// half-way - in the runtime's name: its creation was permitted.
    async fn discard_channel(&self, channel_id: ChannelId) {
        if let Err(e) = self.runtime_manager.entity_store().delete_channel(&SecurityContext::system(), channel_id).await {
            warn!("Channel {} of session {} could not be deleted again: {}", channel_id, self.id, e);
        }
    }

    /// Runs the session's channels, those it has and those created while it
    /// runs, until the session is stopped - which aborts this, and with it
    /// every channel.
    pub async fn run(&self) -> Result<()> {
        let mut start_rx = self.start_rx.try_lock()
            .map_err(|_| anyhow!("Already running"))?;

        let mut join_set = JoinSet::new();
        // The session's own events; those about its users and agents are the
        // `ControllerManager`'s to take in (issue 121).
        let mut global_rx = self.runtime_manager.event_bus().subscribe_session(self.id());

        loop {
            tokio::select! {
                Some(channel_id) = start_rx.recv() => {
                    self.start_channel(channel_id, &mut join_set);
                }
                Some(res) = join_set.join_next(), if !join_set.is_empty() => {
                    match res {
                        Ok(channel_id) => {
                            let attributes = self.channels.get_mut(&channel_id)
                                .map(|mut entry| {
                                    entry.task = None;
                                    entry.channel.security_attributes()
                                })
                                .unwrap_or_default();
                            self.runtime_manager.event_bus().publish(GlobalEvent::ChannelStopped(self.id, channel_id), attributes);
                        }
                        // A deleted channel; `delete_channel` announced it.
                        Err(e) if e.is_cancelled() => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                maybe_global = global_rx.recv() => {
                    self.handle_global_event(maybe_global).await;
                }
            }
        }
    }

    async fn handle_global_event(&self, maybe_global: Option<GlobalEvent>) {
        if let Some(global_event) = maybe_global {
            self.session_controller.process_global_event(&global_event);
        }
    }

    /// Starts the channel `id` in `join_set` - unless it was deleted while
    /// it waited to be started. The entry stays locked while its task is
    /// set, so that `delete_channel` either finds the task or no channel.
    fn start_channel(&self, id: ChannelId, join_set: &mut JoinSet<ChannelId>) {
        let Some(mut entry) = self.channels.get_mut(&id) else { return };
        let channel = entry.channel.clone();
        self.runtime_manager.event_bus().publish(GlobalEvent::ChannelStarted(self.id, id), channel.security_attributes());
        entry.task = Some(join_set.spawn(async move {
            let _ = channel.run().await;
            channel.id()
        }));
    }
}

pub struct SessionManager {
    sessions: DashMap<SessionId, Arc<SessionLoop>>,
    handles: DashMap<SessionId, JoinHandle<()>>,
    /// One lock per session id, held while that session is being started,
    /// so that a session starts only once however many requests ask for it
    /// at the same time (issue 62): two loops of one session would run two
    /// actors per agent and channel, which overwrite each other's state.
    starts: DashMap<SessionId, Arc<tokio::sync::Mutex<()>>>,
    runtime_manager: Arc<RuntimeManager>,
}

/// A session as [`SessionManager::find`] found it for a context that may
/// read it: running or not, each asked about alike.
enum Found {
    /// It runs: its loop.
    Running(Arc<SessionLoop>),
    /// It does not run: its document, from the store.
    Stored(Session),
}

impl Found {
    /// The session as it is now.
    fn session(&self) -> Session {
        match self {
            Found::Running(session_loop) => (**session_loop.session()).clone(),
            Found::Stored(session) => session.clone(),
        }
    }
}

impl SessionManager {
    pub fn new(runtime_manager: Arc<RuntimeManager>) -> Self {
        Self {
            sessions: DashMap::new(),
            handles: DashMap::new(),
            starts: DashMap::new(),
            runtime_manager,
        }
    }

    /// The lock held while session `id` is started.
    fn start_lock(&self, id: SessionId) -> Arc<tokio::sync::Mutex<()>> {
        self.starts.entry(id).or_default().clone()
    }

    /// Build the runtime loop for an (already-persisted) session, register it, spawn its
    /// run task, and announce that the session has started running. Called
    /// with the session's start lock held.
    async fn spawn_session(&self, session: Session, channels: Vec<Channel>) -> Result<Arc<SessionLoop>> {
        let id = session.id;
        let attributes = session.security_attributes.clone();
        let session_loop = SessionLoop::new(session, channels, self.runtime_manager.clone()).await?;
        self.sessions.insert(id, Arc::clone(&session_loop));

        let loop_for_task = Arc::clone(&session_loop);
        let handle = tokio::spawn(async move {
            if let Err(e) = loop_for_task.run().await {
                tracing::error!("Session loop for {} failed: {}", id, e);
            }
        });
        self.handles.insert(id, handle);

        self.runtime_manager.event_bus().publish(GlobalEvent::SessionStarted(id), attributes);
        Ok(session_loop)
    }

    /// Session `id` as `context` may read it: its loop if it runs, its
    /// document from the store if not. A running session is asked about
    /// like a stored one - `GetSession` with its attributes - so that it
    /// being in memory is no way around the policy (issue 11).
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such session or `context` may
    /// not read it, alike whether it runs.
    async fn find(&self, context: &SecurityContext, id: SessionId) -> Result<Found> {
        let running = self.sessions.get(&id).map(|entry| entry.clone());
        match running {
            Some(session_loop) => {
                let attributes = session_loop.session().security_attributes.clone();
                self.runtime_manager.security_manager()
                    .require(context, ActionType::GetSession(&attributes), || StoreError::NotFound { entity: "session", id: id.to_string() }).await?;
                Ok(Found::Running(session_loop))
            }
            None => Ok(Found::Stored(self.runtime_manager.entity_store().get_session(context, id).await?)),
        }
    }

    /// Creates a session from the template `template_id` on behalf of
    /// `context`, whose user owns it and is the template's "user", names it
    /// `name` - the template's name if `None` - and starts it (issue 88).
    /// The template's agents are found by name in their scope, or made.
    ///
    /// # Errors
    /// `SessionError::OwnerNotAUser` if `context` acts for no user; a
    /// `StoreError` if the template, a user or a personality it names does
    /// not exist or may not be read; a `TemplateError` if the template
    /// cannot be made into a session; whatever creating the session reports.
    pub async fn create_from_template(&self, context: &SecurityContext, template_id: SessionTemplateId, name: Option<String>) -> Result<Arc<SessionLoop>> {
        let owner = context.initiator_id.as_user_id().ok_or(SessionError::OwnerNotAUser(context.initiator_id))?;
        let store = self.runtime_manager.entity_store();
        let template = store.get_session_template(context, template_id).await?;
        let id = SessionId::new();
        let participants = crate::template::resolve_participants(store.as_ref(), context, &template, id, owner).await?;
        let name = name.unwrap_or_else(|| template.metadata.name.clone());
        let (session, channels) = template.build(id, owner, name, participants)?;
        self.create_session(context, session, channels).await
    }

    /// Create a brand-new session with its `channels`: persist them - the
    /// channels as documents of their own (issue 70) - announce their
    /// existence, then start the session.
    async fn create_session(&self, context: &SecurityContext, session: Session, channels: Vec<Channel>) -> Result<Arc<SessionLoop>> {
        let id = session.id;
        let start = self.start_lock(id);
        let _starting = start.lock().await;

        for channel in &channels {
            require_classification(&self.runtime_manager, context, &channel.classification).await?;
            require_cleared(&self.runtime_manager, context, channel).await?;
        }
        let store = self.runtime_manager.entity_store();
        store.create_session(context, &session).await?;
        for (index, channel) in channels.iter().enumerate() {
            if let Err(error) = store.create_channel(context, channel).await {
                // Nothing half-made stays behind; the session's creation was
                // permitted, so it is taken back in the runtime's name.
                let system = SecurityContext::system();
                for created in &channels[..index] {
                    let _ = store.delete_channel(&system, created.id).await;
                }
                let _ = store.delete_session(&system, id).await;
                return Err(error.into());
            }
        }

        let event_bus = self.runtime_manager.event_bus();
        event_bus.publish(GlobalEvent::SessionCreated(id), session.security_attributes.clone());
        for channel in &channels {
            event_bus.publish(GlobalEvent::ChannelCreated(id, channel.id), channel.security_attributes.clone());
        }

        self.spawn_session(session, channels).await
    }

    /// The channels `session` lists, loaded in the runtime's name: starting a
    /// session the caller may read follows from an action already permitted,
    /// and it must not fail because the caller is not in one of its channels.
    async fn load_channels(&self, session: &Session) -> Result<Vec<Channel>> {
        let store = self.runtime_manager.entity_store();
        let system = SecurityContext::system();
        let mut channels = Vec::with_capacity(session.channels.len());
        for id in &session.channels {
            channels.push(store.get_channel(&system, *id).await?);
        }
        Ok(channels)
    }

    /// The session `id` for `context`: the running one, or loaded from the
    /// store and started - which announces `SessionStarted`, not
    /// `SessionCreated`, as the session exists already.
    ///
    /// Started once, however many ask at the same time (issue 62): whoever
    /// comes second waits for the first and gets the session it started -
    /// if they may read it.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such session or `context` may
    /// not read it; whatever loading or starting it reports otherwise.
    pub async fn open_session(&self, context: &SecurityContext, id: SessionId) -> Result<Arc<SessionLoop>> {
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        match self.find(context, id).await? {
            Found::Running(session_loop) => Ok(session_loop),
            Found::Stored(session) => {
                let channels = self.load_channels(&session).await?;
                self.spawn_session(session, channels).await
            }
        }
    }

    /// Stops session `id` on behalf of `context` without deleting it: its
    /// channels and actors stop, for everybody in it (`StopSession`).
    /// Returns whether it ran.
    ///
    /// Waits for a start of the same session that is under way (issue 62):
    /// otherwise that start could register the session after it was stopped,
    /// and it would run on.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such session or `context` may
    /// not read it, `StoreError::PermissionDenied` if they may read but not
    /// stop it - whether it runs or not.
    pub async fn stop_session(&self, context: &SecurityContext, id: SessionId) -> Result<bool> {
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        let found = self.find(context, id).await?;
        let attributes = found.session().security_attributes;
        self.runtime_manager.security_manager()
            .require(context, ActionType::StopSession(&attributes), || StoreError::PermissionDenied).await?;
        Ok(matches!(found, Found::Running(_)) && self.stop_running(id).is_some())
    }

    /// Stops session `id` if it runs. Called with its start lock held.
    fn stop_running(&self, id: SessionId) -> Option<Arc<SessionLoop>> {
        if let Some((_, handle)) = self.handles.remove(&id) {
            handle.abort();
        }
        let removed = self.sessions.remove(&id).map(|(_, s)| s);
        if let Some(session_loop) = &removed {
            let attributes = session_loop.session().security_attributes.clone();
            self.runtime_manager.event_bus().publish(GlobalEvent::SessionStopped(id), attributes);
        }
        removed
    }

    /// Stop and permanently delete a session together with all of its channels.
    ///
    /// Asked about before anything happens - a refused deletion must not
    /// stop the session on the way (issue 11).
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such session or `context` may
    /// not read it, `StoreError::PermissionDenied` if they may read but not
    /// delete it.
    pub async fn delete_session(&self, context: &SecurityContext, id: SessionId) -> Result<bool> {
        // Held until the session is gone from the store, so that no request
        // starts it again from there in the meantime (issue 62).
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        let session = self.find(context, id).await?.session();
        self.runtime_manager.security_manager()
            .require(context, ActionType::DeleteSession(&session.security_attributes), || StoreError::PermissionDenied).await?;
        // Captured before the runtime loop is torn down.
        // The events carry the attributes as they were: once deleted, the
        // session is nowhere to look them up (issue 39).
        let channels: Vec<(ChannelId, SecurityAttributes)> = self.load_channels(&session).await?.into_iter()
            .map(|c| (c.id, c.security_attributes))
            .collect();

        self.stop_running(id);

        let event_bus = self.runtime_manager.event_bus();
        for (channel_id, attributes) in channels {
            event_bus.publish(GlobalEvent::ChannelDeleted(id, channel_id), attributes);
        }

        let store = self.runtime_manager.entity_store();
        let removed = store.delete_session(context, id).await?;
        // Its channels go with it - in the runtime's name, since deleting the
        // session was permitted (issue 70).
        for channel_id in &session.channels {
            store.delete_channel(&SecurityContext::system(), *channel_id).await?;
        }
        // The state lives in the `StateStore`, which may be another store
        // than the session's.
        let state_store = self.runtime_manager.state_store();
        for channel_id in &session.channels {
            state_store.delete_channel_state(*channel_id).await?;
        }
        event_bus.publish(GlobalEvent::SessionDeleted(id), session.security_attributes.clone());
        self.starts.remove(&id);
        Ok(removed)
    }

    /// Every session `context` may read, as they see it (issue 80). Running
    /// or not; describing a session does not start it.
    ///
    /// # Errors
    /// Whatever listing the sessions in the store reports.
    pub async fn session_views(&self, context: &SecurityContext) -> Result<Vec<SessionView>> {
        let sessions = self.runtime_manager.entity_store().list_sessions(context).await?;
        let mut views = Vec::with_capacity(sessions.len());
        for session in sessions {
            // The store lists what `context` may read; a running session is
            // described by its loop.
            let running = self.sessions.get(&session.id).map(|entry| entry.clone());
            let found = match running {
                Some(session_loop) => Found::Running(session_loop),
                None => Found::Stored(session),
            };
            views.push(self.describe_session(context, &found).await);
        }
        Ok(views)
    }

    /// Session `id` as `context` sees it (issue 80): its participants and
    /// the channels they may read. Running or not; describing it does not
    /// start it.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such session or `context` may
    /// not read it.
    pub async fn session_view(&self, context: &SecurityContext, id: SessionId) -> Result<SessionView> {
        let found = self.find(context, id).await?;
        Ok(self.describe_session(context, &found).await)
    }

    /// Channel `channel_id` of session `session_id` as `context` sees it
    /// (issue 80). Running or not; describing it does not start it.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such session or `context` may
    /// not read it; `SessionError::ChannelNotFound` if the session has no
    /// such channel, and equally if `context` may not read it, so that
    /// channels cannot be probed.
    pub async fn channel_view(&self, context: &SecurityContext, session_id: SessionId, channel_id: ChannelId) -> Result<ChannelView> {
        let not_found = || SessionError::ChannelNotFound { session: session_id, channel: channel_id };
        let channel = match self.find(context, session_id).await? {
            Found::Running(session_loop) => session_loop.channel_by_id(channel_id).map(|channel| (**channel.channel()).clone()),
            Found::Stored(session) if session.channels.contains(&channel_id) => {
                Some(self.runtime_manager.entity_store().get_channel(&SecurityContext::system(), channel_id).await?)
            }
            Found::Stored(_) => None,
        };
        let channel = channel.ok_or_else(not_found)?;
        self.runtime_manager.security_manager()
            .require(context, ActionType::GetChannel(&channel.security_attributes), not_found).await?;
        require_seen(&self.runtime_manager, context, &channel.classification, not_found).await?;
        Ok(self.describe_channel(context, &channel).await)
    }

    /// `found` as `context` sees it: only the channels they may read
    /// (`GetChannel`, issue 70). A channel whose document cannot be read is
    /// left out, so that one broken channel does not hide its session.
    async fn describe_session(&self, context: &SecurityContext, found: &Found) -> SessionView {
        let session = found.session();
        let channels = match found {
            Found::Running(session_loop) => session_loop.channels().iter().map(|channel| (**channel.channel()).clone()).collect(),
            Found::Stored(session) => {
                let store = self.runtime_manager.entity_store();
                let system = SecurityContext::system();
                let mut channels = Vec::with_capacity(session.channels.len());
                for id in &session.channels {
                    match store.get_channel(&system, *id).await {
                        Ok(channel) => channels.push(channel),
                        Err(e) => warn!("Cannot read channel {} of session {}: {}", id, session.id, e),
                    }
                }
                channels
            }
        };

        let security = self.runtime_manager.security_manager();
        // Without roles, only what is public: a failed lookup shows less.
        let roles = self.runtime_manager.roles_of(context.initiator_id).await.unwrap_or_default();
        let mut readable = Vec::with_capacity(channels.len());
        for channel in &channels {
            if security.may_read_channel(context, &channel.security_attributes).await
                && security.may_see_channel(context, &roles, &channel.classification).await
            {
                readable.push(self.describe_channel(context, channel).await);
            }
        }
        let mut participants = Vec::with_capacity(session.participants.len());
        for participant in &session.participants {
            let (name, _) = self.look_up(participant).await;
            participants.push(ParticipantView { id: participant.actor_id(), name });
        }
        SessionView { id: session.id, name: session.metadata.name, participants, channels: readable }
    }

    /// `channel` as `context` sees it: whether they may change it
    /// (`UpdateChannel`) and, per agent, read what it saw
    /// (`ReadAgentContext`). The one place a channel is described, running
    /// or not.
    async fn describe_channel(&self, context: &SecurityContext, channel: &Channel) -> ChannelView {
        let security = self.runtime_manager.security_manager();
        let mut members = Vec::with_capacity(channel.participants.len());
        for participant in &channel.participants {
            let (name, agent) = self.look_up(participant).await;
            let can_read_context = match &agent {
                Some(attributes) => security.may_read_agent_context(context, attributes).await,
                None => false,
            };
            let handle = match participant {
                Participant::Approver(_) => None,
                _ => name.as_deref().map(handle_of).filter(|handle| !handle.is_empty()),
            };
            members.push(MemberView { id: participant.actor_id(), name, handle, can_read_context });
        }
        ChannelView {
            id: channel.id,
            name: channel.name.clone(),
            moderator: channel.moderator.clone(),
            classification: channel.classification.clone(),
            can_change: security.may_change_channel(context, &channel.security_attributes).await,
            members,
        }
    }

    /// The name of `participant` and, for an agent, its attributes; `None`
    /// for a user or agent that no longer exists. Read in the runtime's
    /// name: taking part in a session makes a name known to everybody in
    /// it - it stands at every message - even where the record itself is
    /// not theirs to read (issue 120).
    async fn look_up(&self, participant: &Participant) -> (Option<String>, Option<SecurityAttributes>) {
        let store = self.runtime_manager.entity_store();
        let system = SecurityContext::system();
        match participant {
            Participant::User(id) => (store.get_user(&system, *id).await.ok().map(|user| user.username), None),
            Participant::Agent(id) => match store.get_agent(&system, *id).await {
                Ok(agent) => (Some(agent.metadata.name), Some(agent.security_attributes)),
                Err(_) => (None, None),
            },
            Participant::Approver(_) => (Some("Approver".to_string()), None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::confidentiality::{Classification, Visibility};
    use caelumora_model::command::default_commands;
    use caelumora_model::channel::Channel;
    use caelumora_model::meta::{Metadata, Scope};
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{DummySecurityManager, SecurityAttributes};
    use caelumora_model::store_impl::memory::MemoryStore;
    use uuid::Uuid;

    /// `users` as participants, recorded in the store the session reads
    /// them from (issue 120).
    async fn record_users(manager: &SessionManager, users: &[&caelumora_model::user::User]) -> Vec<caelumora_model::session::Participant> {
        let store = manager.runtime_manager.entity_store();
        let mut participants = Vec::new();
        for user in users {
            store.create_user(&SecurityContext::system(), user).await.unwrap();
            participants.push(caelumora_model::session::Participant::User(user.id));
        }
        participants
    }

    fn test_runtime() -> Arc<RuntimeManager> {
        let store = Arc::new(MemoryStore::new());
        let secret_manager = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
        Arc::new(RuntimeManager::new(store.clone(), store.clone(), secret_manager, Arc::new(DummySecurityManager::new()), default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(store.clone())).unwrap())
    }

    fn make_session(name: &str) -> Session {
        Session {
            commands: Default::default(),
            privacy: Default::default(),
            id: SessionId(Uuid::new_v4()),
            metadata: Metadata {
                name: name.to_string(),
                scope: Scope::GlobalScope,
                ..Metadata::default()
            },
            participants: Vec::new(),
            channels: vec![ChannelId(Uuid::new_v4())],
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            security_attributes: SecurityAttributes::default(),
        }
    }

    /// The channel `session` lists, as a document of its own (issue 70):
    /// "main", belonging to whom the session belongs to.
    fn main_channel(session: &Session) -> Channel {
        Channel {
            id: session.channels[0],
            session_id: session.id,
            name: "main".to_string(),
            moderator: "broadcast".to_string(),
            classification: caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT,
            history_limits: Vec::new(),
            participants: Vec::new(),
            security_attributes: session.security_attributes.clone(),
        }
    }

    /// Creates and starts `session` with its main channel.
    async fn create(manager: &SessionManager, ctx: &SecurityContext, session: Session) -> Result<Arc<SessionLoop>> {
        let channels = vec![main_channel(&session)];
        manager.create_session(ctx, session, channels).await
    }

    /// Stores `session` and its main channel without starting it.
    async fn persist(runtime: &Arc<RuntimeManager>, ctx: &SecurityContext, session: &Session) {
        runtime.entity_store().create_session(ctx, session).await.unwrap();
        runtime.entity_store().create_channel(ctx, &main_channel(session)).await.unwrap();
    }

    /// Drain every event currently buffered on the receiver.
    fn drain(rx: &mut crate::event::EventSubscription) -> Vec<GlobalEvent> {
        let mut out = Vec::new();
        while let Some(event) = rx.try_recv() {
            out.push(event);
        }
        out
    }

    #[tokio::test]
    async fn create_session_emits_created_then_started() {
        let runtime = test_runtime();
        let manager = SessionManager::new(runtime.clone());
        let mut rx = runtime.event_bus().subscribe_all();
        let ctx = SecurityContext::default();

        let session = make_session("s1");
        let session_id = session.id;
        let channel_id = session.channels[0];

        create(&manager, &ctx, session).await.unwrap();

        // The manager emits these synchronously, in order, before returning.
        let events = drain(&mut rx);
        let created = events.iter().position(|e| matches!(e, GlobalEvent::SessionCreated(id) if *id == session_id));
        let ch_created = events.iter().position(|e| matches!(e, GlobalEvent::ChannelCreated(sid, cid) if *sid == session_id && *cid == channel_id));
        let started = events.iter().position(|e| matches!(e, GlobalEvent::SessionStarted(id) if *id == session_id));

        assert!(created.is_some(), "expected SessionCreated, got {:?}", events);
        assert!(ch_created.is_some(), "expected ChannelCreated, got {:?}", events);
        assert!(started.is_some(), "expected SessionStarted, got {:?}", events);
        assert!(created < ch_created && ch_created < started, "wrong order: {:?}", events);

        // It is now persisted and running.
        assert!(runtime.entity_store().get_session(&ctx, session_id).await.is_ok());
        assert!(manager.sessions.contains_key(&session_id));

        manager.stop_session(&SecurityContext::system(), session_id).await.unwrap();
    }

    #[tokio::test]
    async fn delete_session_emits_stopped_then_deleted() {
        let runtime = test_runtime();
        let manager = SessionManager::new(runtime.clone());
        let ctx = SecurityContext::default();

        let session = make_session("s2");
        let session_id = session.id;
        let channel_id = session.channels[0];
        create(&manager, &ctx, session).await.unwrap();

        // Subscribe after creation so we only observe the deletion events.
        let mut rx = runtime.event_bus().subscribe_all();
        assert!(manager.delete_session(&ctx, session_id).await.unwrap());

        let events = drain(&mut rx);
        let stopped = events.iter().position(|e| matches!(e, GlobalEvent::SessionStopped(id) if *id == session_id));
        let ch_deleted = events.iter().position(|e| matches!(e, GlobalEvent::ChannelDeleted(sid, cid) if *sid == session_id && *cid == channel_id));
        let deleted = events.iter().position(|e| matches!(e, GlobalEvent::SessionDeleted(id) if *id == session_id));

        assert!(stopped.is_some(), "expected SessionStopped, got {:?}", events);
        assert!(ch_deleted.is_some(), "expected ChannelDeleted, got {:?}", events);
        assert!(deleted.is_some(), "expected SessionDeleted, got {:?}", events);
        assert!(stopped < ch_deleted && ch_deleted < deleted, "wrong order: {:?}", events);

        assert!(runtime.entity_store().get_session(&ctx, session_id).await.is_err());
        assert!(!manager.sessions.contains_key(&session_id));
    }

    #[tokio::test]
    async fn opening_a_stored_session_emits_started_but_not_created() {
        let runtime = test_runtime();
        let manager = SessionManager::new(runtime.clone());
        let ctx = SecurityContext::default();

        // Persist a session directly, without going through create_session.
        let session = make_session("s3");
        let session_id = session.id;
        persist(&runtime, &ctx, &session).await;

        let mut rx = runtime.event_bus().subscribe_all();
        manager.open_session(&ctx, session_id).await.unwrap();

        let events = drain(&mut rx);
        assert!(
            events.iter().any(|e| matches!(e, GlobalEvent::SessionStarted(id) if *id == session_id)),
            "expected SessionStarted, got {:?}", events
        );
        assert!(
            !events.iter().any(|e| matches!(e, GlobalEvent::SessionCreated(_))),
            "load must not emit SessionCreated, got {:?}", events
        );

        manager.stop_session(&SecurityContext::system(), session_id).await.unwrap();
    }

    #[tokio::test]
    async fn a_session_asked_for_at_the_same_time_starts_once() {
        let runtime = test_runtime();
        let manager = SessionManager::new(runtime.clone());
        let ctx = SecurityContext::default();
        let session = make_session("concurrent");
        let session_id = session.id;
        persist(&runtime, &ctx, &session).await;

        let mut rx = runtime.event_bus().subscribe_all();
        let (first, second, third) = tokio::join!(
            manager.open_session(&ctx, session_id),
            manager.open_session(&ctx, session_id),
            manager.open_session(&ctx, session_id),
        );
        let (first, second, third) = (first.unwrap(), second.unwrap(), third.unwrap());
        assert!(Arc::ptr_eq(&first, &second) && Arc::ptr_eq(&first, &third), "one loop for all of them");

        let started = drain(&mut rx).into_iter().filter(|e| matches!(e, GlobalEvent::SessionStarted(id) if *id == session_id)).count();
        assert_eq!(started, 1, "the session started once");
        manager.stop_session(&SecurityContext::system(), session_id).await.unwrap();
    }

    #[tokio::test]
    async fn a_stop_waits_for_a_start_under_way_and_then_stops_what_it_started() {
        let runtime = test_runtime();
        let manager = Arc::new(SessionManager::new(runtime.clone()));
        let ctx = SecurityContext::default();
        let session = make_session("stopped");
        let session_id = session.id;
        persist(&runtime, &ctx, &session).await;

        // A start under way holds the lock ...
        let start = manager.start_lock(session_id);
        let starting = start.lock().await;
        let stopping = tokio::spawn({
            let manager = manager.clone();
            async move { manager.stop_session(&SecurityContext::system(), session_id).await.unwrap() }
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!stopping.is_finished(), "the stop waits for the start");

        // ... and registers the session before it lets go: the stop then
        // finds it, and nothing keeps running.
        let channels = vec![main_channel(&session)];
        manager.spawn_session(session, channels).await.unwrap();
        drop(starting);
        assert!(stopping.await.unwrap(), "the stop stopped what the start started");
        assert!(!manager.sessions.contains_key(&session_id));
    }

    #[tokio::test]
    async fn a_session_has_the_global_commands_and_its_own_but_no_name_twice() {
        use caelumora_model::command::HELP_COMMAND_KIND;
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        let runtime = test_runtime();
        let names = |executor: crate::command::CommandExecutor| async move {
            executor.visible(&SecurityContext::default()).await.into_iter().map(|c| c.name).collect::<Vec<_>>()
        };

        let session = make_session("plain");
        let executor = runtime.new_command_executor(&session).await.expect("the global commands build");
        assert_eq!(names(executor).await, vec!["help", "wakeup"]);

        // A second /help from the session's own configuration - with the same
        // component id, which is fine across the two configurations.
        let mut session = make_session("clashing");
        session.commands = ComponentConfig {
            component_nodes: vec![ComponentDescriptor {
                security_attributes: Default::default(),
                id: "help".to_string(),
                kind: HELP_COMMAND_KIND.to_string(),
                params: serde_json::Value::Null,
                tags: Vec::new(),
                fail_policy: Default::default(),
            }],
        };
        let error = runtime.new_command_executor(&session).await.expect_err("/help twice");
        assert!(error.to_string().contains("/help"), "{error}");
    }

    /// A running session owned by a user who takes part in it, with that
    /// user's context and a second user of the session.
    async fn running_session_of_a_user(manager: &SessionManager) -> (Arc<SessionLoop>, SecurityContext, caelumora_model::actor::ActorId) {
        use caelumora_model::actor::ActorId;
        use caelumora_model::user::{User, UserId};
        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new() };
        let (owner, other) = (user("owner"), user("other"));
        let (owner_id, other_id) = (ActorId::User(owner.id), ActorId::User(other.id));
        let mut session = make_session("with-users");
        session.participants = record_users(manager, &[&owner, &other]).await;
        session.security_attributes = SecurityAttributes { owner_id, contributors: Vec::new() };
        let ctx = SecurityContext::new(owner_id, session.id, Default::default());
        let session_loop = create(&manager, &ctx, session).await.expect("the session starts");
        (session_loop, ctx, other_id)
    }

    fn spec(name: &str, participants: Vec<caelumora_model::actor::ActorId>) -> ChannelSpec {
        ChannelSpec { name: name.to_string(), moderator: None, participants, classification: None }
    }

    #[tokio::test]
    async fn a_session_is_created_from_a_template_for_its_owner() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::approval::ApprovalStrategy;
        use caelumora_model::channel::Participant;
        use caelumora_model::template::{ChannelTemplate, ParticipantTemplate, ScopeTemplate, SessionTemplate, SessionTemplateId};
        use caelumora_model::user::{User, UserId};

        let runtime = test_runtime();
        let manager = SessionManager::new(runtime.clone());
        let store = runtime.entity_store();
        let owner = UserId::new();
        store.create_user(&SecurityContext::system(), &User { id: owner, username: "alice".to_string(), email: String::new(), roles: Vec::new() }).await.unwrap();
        let template = SessionTemplate {
            id: SessionTemplateId::new(),
            metadata: Metadata { name: "pairing".to_string(), ..Metadata::default() },
            scope: ScopeTemplate::SessionScope,
            participants: vec![
                ParticipantTemplate::User { name: "user".to_string() },
                ParticipantTemplate::Approver { name: "approval".to_string(), strategy: ApprovalStrategy::All },
            ],
            channels: vec![ChannelTemplate {
                name: "main".to_string(),
                moderator: "broadcast".to_string(),
                classification: Classification::CHANNEL_DEFAULT,
                participants: vec!["user".to_string(), "approval".to_string()],
            }],
            security_attributes: Default::default(),
            commands: Default::default(),
            privacy: Default::default(),
        };
        store.create_session_template(&SecurityContext::system(), &template).await.unwrap();
        let hers = SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());

        let session = manager.create_from_template(&hers, template.id, None).await.expect("created and started");
        assert_eq!(session.name(), "pairing", "named after the template unless named");
        assert_eq!(session.session().security_attributes.owner_id, ActorId::User(owner));
        let main = session.channel_by_name("main").expect("the template's channel runs");
        assert!(main.user_actor(owner).is_some(), "the owner is the template's \"user\"");
        let stored = store.get_channel(&hers, main.id()).await.unwrap();
        assert!(stored.participants.iter().any(|p| matches!(p, Participant::Approver(a) if a.strategy == ApprovalStrategy::All)), "the template's approver");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();

        let missing = manager.create_from_template(&hers, SessionTemplateId::new(), None).await.expect_err("no such template");
        assert!(crate::error::is_not_found(&missing, "session_template"), "{missing}");
        let not_a_user = manager.create_from_template(&SecurityContext::system(), template.id, None).await.expect_err("the system owns no session");
        assert!(matches!(not_a_user.downcast_ref::<SessionError>(), Some(SessionError::OwnerNotAUser(_))), "{not_a_user}");
    }

    #[tokio::test]
    async fn a_renamed_user_is_renamed_in_every_channel_of_the_session() {
        use crate::actor::Actor;
        use caelumora_model::actor::ActorId;
        use caelumora_model::user::UserUpdateEvent;

        let runtime = test_runtime();
        let manager = SessionManager::new(runtime.clone());
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let ActorId::User(owner) = ctx.initiator_id else { unreachable!() };
        let first = session.create_channel(&ctx, spec("first", vec![other])).await.unwrap();
        let second = session.create_channel(&ctx, spec("second", vec![other])).await.unwrap();

        // Announced about the user, as whoever changes users will: the
        // session takes it into the one controller both actors share.
        let renamed = async {
            loop {
                runtime.event_bus().publish(GlobalEvent::UserUpdated(owner, UserUpdateEvent::NameChanged("renamed".to_string())), Default::default());
                let names: Vec<String> = [&first, &second].iter().map(|c| c.user_actor(owner).expect("takes part").name()).collect();
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

    #[tokio::test]
    async fn a_channel_created_in_a_running_session_is_stored_announced_and_runs() {
        let runtime = test_runtime();
        let manager = SessionManager::new(runtime.clone());
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let mut rx = runtime.event_bus().subscribe_all();

        let channel = session.create_channel(&ctx, spec(" side ", vec![other])).await.expect("the channel is created");

        assert_eq!(channel.name(), "side", "the name is trimmed");
        assert_eq!(channel.moderator(), DEFAULT_MODERATOR);
        let kinds: Vec<String> = channel.members().into_iter().map(|m| m.kind).collect();
        assert_eq!(kinds.iter().filter(|k| *k == "user").count(), 2, "the creator and the other user: {kinds:?}");
        assert!(kinds.iter().any(|k| k == "approver"), "an approver is added: {kinds:?}");
        assert!(session.channel_by_id(channel.id()).is_some());
        let names: Vec<String> = session.channels().iter().map(|c| c.name()).collect();
        assert_eq!(names, ["main", "side"], "in the order of the session");

        let stored = runtime.entity_store().get_session(&ctx, session.id()).await.unwrap();
        assert!(stored.channels.contains(&channel.id()), "the session lists the channel");
        let document = runtime.entity_store().get_channel(&ctx, channel.id()).await.expect("stored as a document of its own");
        assert_eq!(document.name, "side");

        // `run` starts it asynchronously.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let events = drain(&mut rx);
        let created = events.iter().position(|e| matches!(e, GlobalEvent::ChannelCreated(_, cid) if *cid == channel.id()));
        let started = events.iter().position(|e| matches!(e, GlobalEvent::ChannelStarted(_, cid) if *cid == channel.id()));
        assert!(created.is_some() && started.is_some() && created < started, "created, then started: {events:?}");

        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_that_cannot_be_is_refused_before_anything_is_stored() {
        let runtime = test_runtime();
        let manager = SessionManager::new(runtime.clone());
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let stranger = caelumora_model::actor::ActorId::User(caelumora_model::user::UserId::new());

        let refusal = |result: Result<Arc<ChannelLoop>>| result.expect_err("refused").downcast::<SessionError>().expect("a SessionError");
        assert!(matches!(refusal(session.create_channel(&ctx, spec("  ", vec![])).await), SessionError::EmptyChannelName));
        assert!(matches!(refusal(session.create_channel(&ctx, spec("main", vec![])).await), SessionError::DuplicateChannelName { .. }));
        assert!(matches!(refusal(session.create_channel(&ctx, spec("x", vec![stranger])).await), SessionError::NotAParticipant { .. }));
        let unknown = ChannelSpec { moderator: Some("nobody".to_string()), ..spec("x", vec![]) };
        assert!(matches!(refusal(session.create_channel(&ctx, unknown).await), SessionError::UnknownModerator(_)));

        assert_eq!(session.channels().len(), 1);
        assert_eq!(runtime.entity_store().get_session(&ctx, session.id()).await.unwrap().channels.len(), 1);
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_deleted_channel_is_stopped_announced_and_its_state_is_gone() {
        use caelumora_model::agent::{AgentChannelState, AgentId};
        let runtime = test_runtime();
        let manager = SessionManager::new(runtime.clone());
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let channel = session.create_channel(&ctx, spec("side", vec![])).await.unwrap();
        let id = channel.id();
        let agent = AgentId::new();
        runtime.state_store().put_agent_channel_state(agent, id, &AgentChannelState::default()).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let mut rx = runtime.event_bus().subscribe_all();

        session.delete_channel(&ctx, id).await.expect("the owner deletes it");

        assert!(session.channel_by_id(id).is_none());
        assert!(!runtime.entity_store().get_session(&ctx, session.id()).await.unwrap().channels.contains(&id));
        assert!(runtime.state_store().get_channel_state(id).await.is_err(), "its state is deleted");
        let events = drain(&mut rx);
        let stopped = events.iter().position(|e| matches!(e, GlobalEvent::ChannelStopped(_, cid) if *cid == id));
        let deleted = events.iter().position(|e| matches!(e, GlobalEvent::ChannelDeleted(_, cid) if *cid == id));
        assert!(stopped.is_some() && deleted.is_some() && stopped < deleted, "stopped, then deleted: {events:?}");

        let missing = session.delete_channel(&ctx, id).await.expect_err("gone");
        assert!(matches!(missing.downcast_ref::<SessionError>(), Some(SessionError::ChannelNotFound { .. })));

        // The last channel may go too; the session keeps running without.
        let main = session.channel_by_name("main").unwrap().id();
        session.delete_channel(&ctx, main).await.expect("the last channel may be deleted");
        assert!(session.channels().is_empty());
        session.create_channel(&ctx, spec("again", vec![])).await.expect("and a new one created");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_deleted_before_the_session_runs_is_never_started() {
        let runtime = test_runtime();
        let ctx = SecurityContext::default();
        let session = make_session("waiting");
        let channel_id = session.channels[0];
        persist(&runtime, &ctx, &session).await;
        let session_loop = SessionLoop::new(session.clone(), vec![main_channel(&session)], runtime.clone()).await.unwrap();
        let mut rx = runtime.event_bus().subscribe_all();

        // Queued to be started, but deleted before `run` gets to it.
        session_loop.delete_channel(&ctx, channel_id).await.expect("deleted");
        let running = tokio::spawn({ let session_loop = session_loop.clone(); async move { session_loop.run().await } });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let events = drain(&mut rx);
        let about = |wanted: fn(&GlobalEvent) -> Option<ChannelId>| events.iter().any(|e| wanted(e) == Some(channel_id));
        assert!(about(|e| match e { GlobalEvent::ChannelDeleted(_, id) => Some(*id), _ => None }), "{events:?}");
        assert!(!about(|e| match e { GlobalEvent::ChannelStarted(_, id) => Some(*id), _ => None }), "never started: {events:?}");
        assert!(!about(|e| match e { GlobalEvent::ChannelStopped(_, id) => Some(*id), _ => None }), "so never stopped: {events:?}");
        running.abort();
    }

    #[tokio::test]
    async fn only_who_may_change_the_session_creates_or_deletes_its_channels() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::{User, UserId};

        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store.clone(), secrets, policy, default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(store.clone())).unwrap());
        let manager = SessionManager::new(runtime);

        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new() };
        let (alice, bob) = (user("alice"), user("bob"));
        let as_user = |id: UserId| SecurityContext::new(ActorId::User(id), Default::default(), Default::default());
        let (alice_ctx, bob_ctx) = (as_user(alice.id), as_user(bob.id));
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes { owner_id: ActorId::User(alice.id), contributors: vec![ActorId::User(bob.id)] };
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let session = create(&manager, &alice_ctx, session).await.unwrap();
        let main = session.channel_by_name("main").unwrap().id();

        let refused = session.create_channel(&bob_ctx, spec("bobs", vec![])).await.expect_err("a contributor may not");
        assert!(crate::error::is_forbidden(&refused), "{refused}");
        let refused = session.delete_channel(&bob_ctx, main).await.expect_err("a contributor may not");
        assert!(crate::error::is_forbidden(&refused), "{refused}");
        assert!(session.channel_by_id(main).is_some(), "a refused deletion leaves the channel");

        session.create_channel(&alice_ctx, spec("hers", vec![])).await.expect("the owner may");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_its_viewer_may_not_read_is_not_there_for_them() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::{User, UserId};

        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store.clone(), secrets, policy, default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(store.clone())).unwrap());
        let manager = SessionManager::new(runtime);
        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new() };
        let (alice, bob) = (user("alice"), user("bob"));
        let as_user = |id: UserId| SecurityContext::new(ActorId::User(id), Default::default(), Default::default());
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes { owner_id: ActorId::User(alice.id), contributors: vec![ActorId::User(bob.id)] };
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let session = create(&manager, &as_user(alice.id), session).await.unwrap();
        // Alice's alone: bob takes part in the session, not in this channel.
        let hers = session.create_channel(&as_user(alice.id), spec("hers", vec![])).await.unwrap();

        let hidden = session.channel_for(&as_user(bob.id), hers.id()).await.expect_err("not bob's to read");
        assert!(matches!(hidden.downcast_ref::<SessionError>(), Some(SessionError::ChannelNotFound { .. })), "absent, not refused: {hidden}");
        assert!(session.channel_for(&as_user(alice.id), hers.id()).await.is_ok(), "the owner reads it");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_is_described_with_handles_only_for_who_can_be_mentioned() {
        use caelumora_model::approval::{ApprovalStrategy, Approver, ApproverId};
        use caelumora_model::user::{User, UserId};

        let manager = SessionManager::new(test_runtime());
        let alice = User { id: UserId::new(), username: "Alice Example".to_string(), email: String::new(), roles: Vec::new() };
        let gone = UserId::new();
        let approver = Approver { id: ApproverId(Uuid::new_v4()), strategy: ApprovalStrategy::Any };
        let mut channel = main_channel(&make_session("s"));
        channel.participants = record_users(&manager, &[&alice]).await;
        channel.participants.extend([Participant::User(gone), Participant::Approver(approver)]);

        let view = manager.describe_channel(&SecurityContext::default(), &channel).await;

        let [alice, gone, approver] = view.members.as_slice() else { panic!("three members, in order: {view:?}") };
        assert_eq!((alice.name.as_deref(), alice.handle.as_deref()), (Some("Alice Example"), Some("alice-example")));
        assert_eq!((gone.name.as_deref(), gone.handle.as_deref()), (None, None), "a deleted user is listed, but cannot be mentioned");
        assert!(approver.handle.is_none(), "an approver cannot be mentioned");
        assert!(view.members.iter().all(|m| !m.can_read_context), "no member is an agent");
    }

    #[tokio::test]
    async fn a_view_shows_what_its_viewer_may_read_and_do_whether_the_session_runs_or_not() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::agent::{Agent, Personality, PersonalityId};
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::store::EntityStore;
        use caelumora_model::user::{User, UserId};

        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store.clone(), secrets, policy, default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(store.clone())).unwrap());
        let manager = SessionManager::new(runtime);
        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new() };
        let (alice, bob, carol) = (user("alice"), user("bob"), user("carol"));
        let as_user = |id: UserId| SecurityContext::new(ActorId::User(id), Default::default(), Default::default());

        let personality = Personality {
            reasoning_visibility: Visibility::Channel,
            id: PersonalityId::new(),
            metadata: Metadata { name: "plain".to_string(), ..Metadata::default() },
            components: Default::default(),
            security_attributes: Default::default(),
        };
        store.create_personality(&SecurityContext::system(), &personality).await.unwrap();
        let helper = Agent::new("Helper".to_string(), Scope::GlobalScope, &personality, ActorId::User(alice.id));
        store.create_agent(&SecurityContext::system(), &helper).await.unwrap();

        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes { owner_id: ActorId::User(alice.id), contributors: vec![ActorId::User(bob.id), ActorId::Agent(helper.id)] };
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(helper.id));
        let mut main = main_channel(&session);
        main.participants = session.participants.clone();
        let running = manager.create_session(&as_user(alice.id), session, vec![main]).await.unwrap();
        // Alice's alone: bob takes part in the session, not in this channel.
        let hers = running.create_channel(&as_user(alice.id), spec("private", vec![])).await.unwrap().id();
        let id = running.id();

        let check = |state: &'static str| {
            let manager = &manager;
            async move {
                let names = |view: &SessionView| view.channels.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
                let for_alice = manager.session_view(&as_user(alice.id), id).await.unwrap();
                assert_eq!(names(&for_alice), ["main", "private"], "{state}: the owner reads every channel");
                assert_eq!(for_alice.participants.iter().filter_map(|p| p.name.as_deref()).collect::<Vec<_>>(), ["alice", "bob", "Helper"], "{state}");
                let for_bob = manager.session_view(&as_user(bob.id), id).await.unwrap();
                assert_eq!(names(&for_bob), ["main"], "{state}: bob is not in the private channel");

                let main = &for_alice.channels[0];
                assert!(main.can_change, "{state}: the owner changes the channel");
                let agent = main.members.iter().find(|m| m.id == ActorId::Agent(helper.id)).expect("the agent is a member");
                assert_eq!(agent.handle.as_deref(), Some("helper"), "{state}");
                assert!(agent.can_read_context, "{state}: alice made the agent");
                let main = &for_bob.channels[0];
                assert!(!main.can_change, "{state}: a participant does not change it");
                assert!(main.members.iter().all(|m| !m.can_read_context), "{state}: bob reads no agent's context");

                let hidden = manager.channel_view(&as_user(bob.id), id, hers).await.expect_err("not bob's to read");
                assert!(matches!(hidden.downcast_ref::<SessionError>(), Some(SessionError::ChannelNotFound { .. })), "{state}: absent, not refused: {hidden}");
                assert_eq!(manager.channel_view(&as_user(alice.id), id, hers).await.unwrap().name, "private", "{state}");
                let refused = manager.session_view(&as_user(carol.id), id).await.expect_err("carol may not see it");
                assert!(crate::error::is_not_found(&refused, "session"), "{state}: {refused}");
                assert_eq!(manager.session_views(&as_user(carol.id)).await.unwrap(), Vec::new(), "{state}: nor is it listed for her");
                assert_eq!(manager.session_views(&as_user(bob.id)).await.unwrap(), [for_bob], "{state}");
            }
        };
        check("running").await;
        assert!(manager.stop_session(&as_user(alice.id), id).await.unwrap(), "it ran");
        check("stopped").await;
        assert!(!manager.sessions.contains_key(&id), "describing a session does not start it");
    }

    #[tokio::test]
    async fn a_channel_reads_another_by_their_classification_not_by_who_takes_part() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::confidentiality::{Compartment, Compartments, Confidentiality};
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::{User, UserId};

        let compartments = Compartments::new([(Compartment::new("hr"), vec![])]).unwrap();
        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::with_compartments(compartments));
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store.clone(), secrets, policy, default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(store.clone())).unwrap());
        let manager = SessionManager::new(runtime.clone());

        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new() };
        let (alice, bob) = (user("alice"), user("bob"));
        let alice_id = ActorId::User(alice.id);
        let bob_id = ActorId::User(bob.id);
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default());
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes { owner_id: alice_id, contributors: vec![bob_id] };
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let session = create(&manager, &alice_ctx, session).await.unwrap();

        let classified = |level, compartments: &[&str]| Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap();
        let with = |name: &str, classification, participants| ChannelSpec { classification: Some(classification), ..spec(name, participants) };
        let public = session.create_channel(&alice_ctx, with("public", classified(Confidentiality::Public, &[]), vec![bob_id])).await.unwrap();
        let team = session.create_channel(&alice_ctx, with("team", classified(Confidentiality::Internal, &[]), vec![bob_id])).await.unwrap();
        let secret = session.create_channel(&alice_ctx, with("secret", classified(Confidentiality::Confidential, &[]), vec![])).await.unwrap();
        let hr = session.create_channel(&alice_ctx, with("hr", classified(Confidentiality::Internal, &["hr"]), vec![])).await.unwrap();

        let security = runtime.security_manager();
        let may_read = |reader: &Arc<ChannelLoop>, source: &Arc<ChannelLoop>| {
            let (reader, source, security, context) = (reader.clone(), source.clone(), security.clone(), alice_ctx.clone());
            async move {
                use caelumora_model::security::AccessQuestions;
                use caelumora_model::security::Place;
                let (reader_channel, reader_classification, reader_session) = (reader.channel_ref(), reader.classification(), reader.session_privacy());
                let (source_channel, source_classification, source_session) = (source.channel_ref(), source.classification(), source.session_privacy());
                security.may_channel_read(
                    &context,
                    Place { channel: &reader_channel, classification: &reader_classification, session: &reader_session },
                    Place { channel: &source_channel, classification: &source_classification, session: &source_session },
                ).await
            }
        };
        assert!(may_read(&secret, &secret).await, "a channel reads itself");
        assert!(may_read(&team, &public).await);
        assert!(!may_read(&public, &team).await, "a public channel reads only what is public");
        assert!(!may_read(&team, &secret).await, "confidential is not read from internal");
        assert!(may_read(&secret, &team).await, "although bob, who is in team, is not in secret");
        assert!(may_read(&hr, &team).await, "what is general flows into HR");
        assert!(!may_read(&secret, &hr).await, "what is HR's never into a general channel");

        let refused = session.create_channel(&alice_ctx, with("typo", classified(Confidentiality::Internal, &["hrr"]), vec![])).await.expect_err("not configured");
        assert!(matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::ClassificationRefused(_))), "{refused}");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_released_message_is_read_where_its_channel_is_not() {
        use crate::channel::ChannelError;
        use crate::session_reader::ReadError;
        use caelumora_model::actor::ActorId;
        use caelumora_model::confidentiality::{Compartment, Compartments, Confidentiality};
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::{User, UserId};

        let compartments = Compartments::new([(Compartment::new("hr"), vec![]), (Compartment::new("finance"), vec![])]).unwrap();
        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::with_compartments(compartments));
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store.clone(), secrets, policy, default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(store.clone())).unwrap());
        let manager = SessionManager::new(runtime.clone());
        let alice = User { id: UserId::new(), username: "alice".to_string(), email: String::new(), roles: Vec::new() };
        let alice_id = ActorId::User(alice.id);
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default());
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes::owned_by(alice_id);
        session.participants = record_users(&manager, &[&alice]).await;
        let session = create(&manager, &alice_ctx, session).await.unwrap();

        let classified = |level, compartments: &[&str]| Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap();
        let with = |name: &str, classification| ChannelSpec { classification: Some(classification), ..spec(name, vec![]) };
        let hr = session.create_channel(&alice_ctx, with("hr", classified(Confidentiality::Internal, &["hr"]))).await.unwrap();
        let finance = session.create_channel(&alice_ctx, with("finance", classified(Confidentiality::Internal, &["finance"]))).await.unwrap();
        let team = session.create_channel(&alice_ctx, with("team", classified(Confidentiality::Internal, &[]))).await.unwrap();

        let writer = hr.user_actor(alice.id).expect("the creator takes part");
        writer.send_message("for HR".to_string(), None).await.unwrap();
        writer.send_message("for finance too".to_string(), Some(classified(Confidentiality::Internal, &["hr", "finance"]))).await.unwrap();
        let above = writer.send_message("too secret".to_string(), Some(classified(Confidentiality::Confidential, &["hr"]))).await.expect_err("above the channel");
        assert!(matches!(above.downcast_ref::<ChannelError>(), Some(ChannelError::ClassificationRefused { .. })), "{above}");
        let history = history_of(&hr, 2).await;
        assert_eq!(history[0].classification, classified(Confidentiality::Internal, &["hr"]), "the channel's, unless classified otherwise");

        // Read as an agent in another channel reads.
        let reader = |channel: &Arc<ChannelLoop>| {
            let channel_runtime = ChannelRuntime::new(runtime.clone(), session.commands.clone());
            channel_runtime.attach(channel);
            channel_runtime.new_actor_runtime(alice_id, session.id(), channel.id()).session_reader(alice_ctx.clone())
        };
        let read = reader(&finance).history_of("hr").await.expect("what was released to finance");
        let texts: Vec<_> = read.history.messages.iter().map(|m| m.content.clone().unwrap_or_default()).collect();
        assert_eq!(texts, ["for finance too"], "only what was released");
        assert!(matches!(reader(&team).history_of("hr").await, Err(ReadError::NotReadable(_))), "nothing was released to a general channel");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_and_its_messages_are_reclassified_by_who_may() {
        use crate::channel::ChannelError;
        use caelumora_model::actor::ActorId;
        use caelumora_model::confidentiality::{Compartment, Compartments, Confidentiality};
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::{User, UserId};

        let compartments = Compartments::new([(Compartment::new("hr"), vec![]), (Compartment::new("finance"), vec![])]).unwrap();
        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::with_compartments(compartments));
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store.clone(), secrets, policy, default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(store.clone())).unwrap());
        let manager = SessionManager::new(runtime.clone());
        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new() };
        let (alice, bob) = (user("alice"), user("bob"));
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(bob.id));
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default());
        let bob_ctx = SecurityContext::new(bob_id, Default::default(), Default::default());
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes { owner_id: alice_id, contributors: vec![bob_id] };
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let session = create(&manager, &alice_ctx, session).await.unwrap();

        let classified = |level, compartments: &[&str]| Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap();
        let spec = ChannelSpec { classification: Some(classified(Confidentiality::Confidential, &["hr"])), ..spec("hr", vec![bob_id]) };
        let hr = session.create_channel(&alice_ctx, spec).await.unwrap();
        hr.user_actor(bob.id).unwrap().send_message("from bob".to_string(), None).await.unwrap();
        hr.user_actor(alice.id).unwrap().send_message("from alice".to_string(), None).await.unwrap();
        let history = history_of(&hr, 2).await;
        // Two users' messages arrive in either order.
        let id_of = |history: &[caelumora_model::message::Message], text: &str| history.iter().find(|m| m.content.as_deref() == Some(text)).map(|m| m.id).unwrap();
        let (bobs, alices) = (id_of(&history, "from bob"), id_of(&history, "from alice"));
        let mut events = runtime.event_bus().subscribe_all();

        let released = classified(Confidentiality::Internal, &["hr", "finance"]);
        hr.reclassify_message(&bob_ctx, bobs, released.clone()).await.expect("the author releases their message");
        let refused = hr.reclassify_message(&bob_ctx, alices, released.clone()).await.expect_err("not another's");
        assert!(matches!(refused.downcast_ref::<ChannelError>(), Some(ChannelError::Forbidden { .. })), "{refused}");
        let missing = hr.reclassify_message(&alice_ctx, caelumora_model::message::MessageId::new(), released.clone()).await.expect_err("no such message");
        assert!(matches!(missing.downcast_ref::<ChannelError>(), Some(ChannelError::MessageNotFound { .. })), "{missing}");
        assert!(drain(&mut events).iter().any(|e| matches!(e, GlobalEvent::MessageReclassified(_, cid, mid, c) if *cid == hr.id() && *mid == bobs && *c == released)),
                "announced");

        let lowered = classified(Confidentiality::Internal, &["hr"]);
        assert!(hr.reclassify(&bob_ctx, lowered.clone()).await.is_err(), "only the owner reclassifies the channel");
        hr.reclassify(&alice_ctx, lowered.clone()).await.expect("the owner lowers it");
        assert_eq!(hr.classification(), lowered);
        assert_eq!(runtime.entity_store().get_channel(&alice_ctx, hr.id()).await.unwrap().classification, lowered, "stored");
        let history = hr.history().await;
        let classification_of = |id| history.iter().find(|m| m.id == id).map(|m| m.classification.clone()).unwrap();
        assert_eq!(classification_of(bobs), released, "what was released stays released");
        assert_eq!(classification_of(alices), lowered, "the rest is released with the channel");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn only_who_is_cleared_takes_part_in_or_reads_a_channel() {
        use crate::channel::ChannelError;
        use caelumora_model::actor::ActorId;
        use caelumora_model::confidentiality::{Clearance, Compartment, Compartments, Confidentiality, Roles};
        use caelumora_model::store::EntityStore;
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::{User, UserId};

        let compartments = Compartments::new([(Compartment::new("hr"), vec![])]).unwrap();
        let roles = Roles::new([
            ("staff".to_string(), vec![Clearance { level: Confidentiality::Internal, compartment: None }]),
            ("hr".to_string(), vec![Clearance { level: Confidentiality::Confidential, compartment: Some(Compartment::new("hr")) }]),
        ]).unwrap();
        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::with_compartments(compartments).with_roles(roles));
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store.clone(), secrets, policy, default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(store.clone())).unwrap());
        let manager = SessionManager::new(runtime.clone());
        let user = |name: &str, roles: &[&str]| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: roles.iter().map(|r| r.to_string()).collect() };
        let (alice, bob) = (user("alice", &["staff", "hr"]), user("bob", &["staff"]));
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(bob.id));
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default());
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes { owner_id: alice_id, contributors: vec![bob_id] };
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let session = create(&manager, &alice_ctx, session).await.unwrap();

        let hr_confidential = Classification::new(Confidentiality::Confidential, [Compartment::new("hr")]).unwrap();
        let with = |name: &str, classification: &Classification, participants| ChannelSpec { classification: Some(classification.clone()), ..spec(name, participants) };
        let refused = session.create_channel(&alice_ctx, with("hr", &hr_confidential, vec![bob_id])).await.expect_err("bob is not cleared for HR");
        assert!(matches!(refused.downcast_ref::<ChannelError>(), Some(ChannelError::NotCleared { participant, .. }) if *participant == bob_id), "{refused}");
        let hr = session.create_channel(&alice_ctx, with("hr", &hr_confidential, vec![])).await.expect("alice is");
        let joining = hr.add_participant(&alice_ctx, bob_id, caelumora_model::channel::HistoryAccess::All).await.expect_err("nor may he join");
        assert!(matches!(joining.downcast_ref::<ChannelError>(), Some(ChannelError::NotCleared { .. })), "{joining}");

        let team = session.create_channel(&alice_ctx, with("team", &Classification::general(Confidentiality::Internal), vec![bob_id])).await.expect("both are cleared for internal");
        let raising = team.reclassify(&alice_ctx, Classification::general(Confidentiality::Confidential)).await.expect_err("above bob");
        assert!(matches!(raising.downcast_ref::<ChannelError>(), Some(ChannelError::NotCleared { .. })), "{raising}");
        assert_eq!(team.classification(), Classification::general(Confidentiality::Internal), "unchanged");

        // Alice, no longer in HR, no longer sees its channel - though she owns it.
        store.update_user(&SecurityContext::system(), &User { roles: vec!["staff".to_string()], ..alice.clone() }).await.unwrap();
        assert!(matches!(session.channel_for(&alice_ctx, hr.id()).await.expect_err("not cleared").downcast_ref::<SessionError>(), Some(SessionError::ChannelNotFound { .. })));
        let view = manager.session_view(&alice_ctx, session.id()).await.unwrap();
        assert!(view.channels.iter().all(|c| c.id != hr.id()) && view.channels.iter().any(|c| c.id == team.id()), "listed without it");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn deleting_a_session_deletes_its_state_from_the_state_store() {
        use caelumora_model::store::StateStore;
        let entities = Arc::new(MemoryStore::new());
        let states = Arc::new(MemoryStore::new());
        let secret_manager = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
        let runtime = Arc::new(RuntimeManager::new(states.clone(), entities.clone(), secret_manager, Arc::new(DummySecurityManager::new()), default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(entities.clone())).unwrap());
        let manager = SessionManager::new(runtime);
        let ctx = SecurityContext::default();
        let session = make_session("two-stores");
        let (id, channel_id) = (session.id, session.channels[0]);
        create(&manager, &ctx, session).await.unwrap();
        assert!(states.get_channel_state(channel_id).await.is_ok(), "the channel keeps its state in the state store");

        manager.delete_session(&ctx, id).await.unwrap();

        assert!(states.get_channel_state(channel_id).await.is_err(), "deleted from the state store, not only from the entity store");
        use caelumora_model::store::EntityStore;
        assert!(entities.get_channel(&ctx, channel_id).await.is_err(), "its channels go with it (issue 70)");
    }

    #[tokio::test]
    async fn renaming_a_channel_is_stored_and_only_the_owner_may() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::store::EntityStore;
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::UserId;

        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store.clone(), secrets, policy, default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(store.clone())).unwrap());
        let manager = SessionManager::new(runtime);
        let (alice, bob) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        let as_user = |actor| SecurityContext::new(actor, Default::default(), Default::default());
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes { owner_id: alice, contributors: vec![bob] };
        let running = create(&manager, &as_user(alice), session).await.unwrap();
        let main = running.channel_by_name("main").unwrap();

        let refused = main.update_name(&as_user(bob), "bobs".to_string()).await.expect_err("a contributor may not");
        assert!(crate::error::is_forbidden(&refused), "{refused}");
        assert_eq!(main.name(), "main", "unchanged");

        main.update_name(&as_user(alice), "lobby".to_string()).await.expect("the owner may");
        assert_eq!(main.name(), "lobby");
        assert_eq!(store.get_channel(&as_user(alice), main.id()).await.unwrap().name, "lobby", "stored, not only in memory");
        manager.stop_session(&SecurityContext::system(), running.id()).await.unwrap();
    }

    /// Waits until `channel`'s history holds `count` messages.
    async fn history_of(channel: &ChannelLoop, count: usize) -> Vec<caelumora_model::message::Message> {
        for _ in 0..100 {
            let history = channel.history().await;
            if history.len() >= count {
                return history;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the channel's history never reached {count} messages");
    }

    #[tokio::test]
    async fn a_participant_added_later_sees_the_history_granted_and_is_announced() {
        use crate::channel::ChannelError;
        use caelumora_model::channel::HistoryAccess;
        let runtime = test_runtime();
        let manager = SessionManager::new(runtime.clone());
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let channel = session.create_channel(&ctx, spec("side", vec![])).await.unwrap();
        let owner = match ctx.initiator_id { caelumora_model::actor::ActorId::User(id) => id, _ => unreachable!() };
        let writer = channel.user_actor(owner).expect("the creator takes part");
        for text in ["one", "two", "three"] {
            writer.send_message(text.to_string(), None).await.unwrap();
        }
        let before = history_of(&channel, 3).await;
        let mut rx = runtime.event_bus().subscribe_all();

        channel.add_participant(&ctx, other, HistoryAccess::LastMessages { count: 1 }).await.expect("a user of the session joins");

        assert!(channel.members().iter().any(|m| m.id == other), "a member now");
        let other_user = match other { caelumora_model::actor::ActorId::User(id) => id, _ => unreachable!() };
        assert!(channel.user_actor(other_user).is_some(), "with an actor of their own");
        let visible: Vec<_> = channel.visible_history(other).await.messages.into_iter().map(|m| m.content).collect();
        assert_eq!(visible, vec![Some("three".to_string())], "the last message and no more");
        assert_eq!(channel.visible_history(ctx.initiator_id).await.messages.len(), before.len(), "the others still see everything");
        // What a reader records as read says from where (issue 72).
        let (limited, full) = (channel.visible_history(other).await, channel.visible_history(ctx.initiator_id).await);
        assert_eq!(limited.start, Some(caelumora_model::channel::HistoryStart::From(before[2].id)), "from the last message");
        assert_eq!(full.start, None, "all of it");
        assert_eq!(limited.digest, full.digest, "up to where is the same for both");
        let stored = runtime.entity_store().get_channel(&ctx, channel.id()).await.unwrap();
        assert!(stored.participants.iter().any(|p| p.actor_id() == other), "stored, not only in memory");
        assert_eq!(stored.history_limits.len(), 1, "with the limit");
        let events = drain(&mut rx);
        assert!(events.iter().any(|e| matches!(e, GlobalEvent::UserChannelJoined(_, cid, uid) if *cid == channel.id() && *uid == other_user)),
                "the join is announced: {events:?}");

        // What is written after joining is visible.
        writer.send_message("four".to_string(), None).await.unwrap();
        history_of(&channel, 4).await;
        assert_eq!(channel.visible_history(other).await.messages.len(), 2);

        let refusal = |error: anyhow::Error| error.downcast::<ChannelError>().expect("a ChannelError");
        let again = channel.add_participant(&ctx, other, HistoryAccess::All).await.expect_err("already in");
        assert!(matches!(refusal(again), ChannelError::AlreadyParticipant { .. }));
        let stranger = caelumora_model::actor::ActorId::User(caelumora_model::user::UserId::new());
        let outside = channel.add_participant(&ctx, stranger, HistoryAccess::All).await.expect_err("not in the session");
        assert!(matches!(refusal(outside), ChannelError::NotInSession { .. }));
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_users_watch_ends_when_they_leave_or_the_channel_is_deleted() {
        let runtime = test_runtime();
        let manager = SessionManager::new(runtime.clone());
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let owner = ctx.initiator_id.as_user_id().unwrap();
        let within = |ended: tokio_util::sync::CancellationToken| async move {
            tokio::time::timeout(std::time::Duration::from_secs(5), ended.cancelled_owned()).await
        };

        // Leaving ends it.
        let channel = session.create_channel(&ctx, spec("side", vec![other])).await.unwrap();
        let theirs = channel.user_actor(other.as_user_id().unwrap()).unwrap().watch().unwrap();
        let mine = channel.user_actor(owner).unwrap().watch().unwrap();
        channel.remove_participant(&ctx, other).await.expect("removed");
        within(theirs.ended).await.expect("the watch of whoever left ends");
        assert!(!mine.ended.is_cancelled(), "everybody else's goes on");

        // So does deleting the channel, for everybody.
        session.delete_channel(&ctx, channel.id()).await.expect("deleted");
        within(mine.ended).await.expect("a deleted channel is watched by nobody");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_removed_participant_is_gone_and_announced_but_the_approver_stays() {
        use crate::channel::ChannelError;
        let runtime = test_runtime();
        let manager = SessionManager::new(runtime.clone());
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let channel = session.create_channel(&ctx, spec("side", vec![other])).await.unwrap();
        let mut rx = runtime.event_bus().subscribe_all();

        channel.remove_participant(&ctx, other).await.expect("removed");

        assert!(!channel.members().iter().any(|m| m.id == other), "no longer a member");
        let stored = runtime.entity_store().get_channel(&ctx, channel.id()).await.unwrap();
        assert!(!stored.participants.iter().any(|p| p.actor_id() == other), "stored, not only in memory");
        let events = drain(&mut rx);
        assert!(events.iter().any(|e| matches!(e, GlobalEvent::UserChannelLeft(_, cid, _) if *cid == channel.id())),
                "the leave is announced: {events:?}");

        let refusal = |error: anyhow::Error| error.downcast::<ChannelError>().expect("a ChannelError");
        let gone = channel.remove_participant(&ctx, other).await.expect_err("not in it any more");
        assert!(matches!(refusal(gone), ChannelError::NotAParticipant { .. }));
        let approver = channel.members().into_iter().find(|m| m.kind == "approver").expect("an approver").id;
        let stays = channel.remove_participant(&ctx, approver).await.expect_err("the approver stays");
        assert!(matches!(refusal(stays), ChannelError::ApproverStays { .. }));
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn only_who_may_change_a_channel_adds_or_removes_its_participants() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::channel::HistoryAccess;
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::{User, UserId};

        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store.clone(), secrets, policy, default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(store.clone())).unwrap());
        let manager = SessionManager::new(runtime);
        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new() };
        let (alice, bob, carol) = (user("alice"), user("bob"), user("carol"));
        let (alice_id, bob_id, carol_id) = (ActorId::User(alice.id), ActorId::User(bob.id), ActorId::User(carol.id));
        let as_user = |actor| SecurityContext::new(actor, Default::default(), Default::default());
        let mut session = make_session("hers");
        session.participants = record_users(&manager, &[&alice, &bob, &carol]).await;
        session.security_attributes = SecurityAttributes { owner_id: alice_id, contributors: vec![bob_id, carol_id] };
        let running = create(&manager, &as_user(alice_id), session).await.unwrap();
        let channel = running.create_channel(&as_user(alice_id), spec("side", vec![bob_id])).await.unwrap();

        let refused = channel.add_participant(&as_user(bob_id), carol_id, HistoryAccess::All).await.expect_err("a participant may not");
        assert!(crate::error::is_forbidden(&refused), "{refused}");
        let refused = channel.remove_participant(&as_user(bob_id), bob_id).await.expect_err("nor remove anyone");
        assert!(crate::error::is_forbidden(&refused), "{refused}");
        assert_eq!(channel.members().len(), 3, "alice, bob and the approver, unchanged");

        channel.add_participant(&as_user(alice_id), carol_id, HistoryAccess::None).await.expect("the owner may");
        channel.remove_participant(&as_user(alice_id), bob_id).await.expect("the owner may");
        assert!(channel.members().iter().any(|m| m.id == carol_id) && !channel.members().iter().any(|m| m.id == bob_id));
        manager.stop_session(&SecurityContext::system(), running.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_running_session_is_no_way_around_the_policy() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::UserId;

        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store.clone(), secrets, policy, default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(store.clone())).unwrap());
        let manager = SessionManager::new(runtime);

        let as_user = |id| SecurityContext::new(ActorId::User(id), Default::default(), Default::default());
        let (alice, bob, carol) = (UserId::new(), UserId::new(), UserId::new());
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes { owner_id: ActorId::User(alice), contributors: vec![ActorId::User(bob)] };
        let id = session.id;
        create(&manager, &as_user(alice), session).await.expect("alice creates her own session");

        let refused = manager.open_session(&as_user(carol), id).await.expect_err("carol may not see it");
        assert!(crate::error::is_not_found(&refused, "session"), "not refused - absent: {refused}");
        manager.open_session(&as_user(bob), id).await.expect("a contributor opens it");

        let refused = manager.delete_session(&as_user(bob), id).await.expect_err("a contributor may not delete it");
        assert!(crate::error::is_forbidden(&refused), "{refused}");
        assert!(manager.sessions.contains_key(&id), "a refused deletion must not stop the session on the way");

        let refused = manager.stop_session(&as_user(carol), id).await.expect_err("carol may not see it");
        assert!(crate::error::is_not_found(&refused, "session"), "not refused - absent: {refused}");
        let refused = manager.stop_session(&as_user(bob), id).await.expect_err("a contributor may not stop it");
        assert!(crate::error::is_forbidden(&refused), "{refused}");
        assert!(manager.sessions.contains_key(&id), "it runs on");
        assert!(manager.stop_session(&as_user(alice), id).await.unwrap(), "the owner stops it");
        assert!(!manager.stop_session(&as_user(alice), id).await.unwrap(), "stopped already");

        manager.delete_session(&as_user(alice), id).await.expect("the owner deletes it");
        assert!(!manager.sessions.contains_key(&id));
    }
}
