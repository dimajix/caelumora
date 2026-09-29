use crate::channel::ChannelLoop;
use crate::command::CommandExecutor;
use crate::runtime::{ChannelRuntime, RuntimeManager};
use anyhow::{anyhow, Result};
use caelumora_model::actor::ActorId;
use caelumora_model::channel::{Channel, DEFAULT_MODERATOR};
use caelumora_model::confidentiality::Confidentiality;
use caelumora_model::event::GlobalEvent;
use caelumora_model::security::{ActionType, SecurityAttributes, SecurityContext};
use caelumora_model::store::StoreError;
use tracing::{info, warn};
use caelumora_model::session::{ChannelId, Session, SessionId};
use crate::controller::SessionController;
use dashmap::DashMap;
use std::sync::{Arc, OnceLock, Weak};
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
    #[error("Session {session} has no channel {channel}")]
    ChannelNotFound { session: SessionId, channel: ChannelId },
    #[error("A channel cannot be {0}: that level is an actor's, not a channel's")]
    InvalidConfidentiality(Confidentiality),
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
    /// How far the channel's contents may travel (issue 61); `None` for the
    /// default, as in a template. Set by whoever may create the channel -
    /// its owner.
    pub confidentiality: Option<Confidentiality>,
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
    /// Shared with the runtime of every channel, so that a channel reaches
    /// the others of its session (issue 17). Set by `attach`.
    handle: Arc<OnceLock<Weak<SessionLoop>>>,
}

impl SessionLoop {
    /// The loop of `session`, with its `channels` - documents of their own
    /// (issue 70), in the order the session lists them.
    pub async fn new(session: Session, channels: Vec<Channel>, runtime_manager: Arc<RuntimeManager>) -> Result<Self> {
        let id = session.id;
        let commands = Arc::new(runtime_manager.new_command_executor(&session).await?);
        let (start_tx, start_rx) = mpsc::unbounded_channel();
        let handle = Arc::new(OnceLock::new());
        let channel_docs = channels;
        let channels = DashMap::new();
        for channel in channel_docs {
            let runtime = ChannelRuntime::new(runtime_manager.clone(), commands.clone()).with_session(handle.clone());
            let cl = ChannelLoop::new(channel, runtime).await?;
            let id = cl.id();
            channels.insert(id, ChannelEntry { channel: cl, task: None });
            let _ = start_tx.send(id);
        }

        let session_controller = runtime_manager.controller_manager().new_session_controller(session);

        Ok(Self {
            id,
            session_controller,
            runtime_manager,
            commands,
            channels,
            start_tx,
            start_rx: Mutex::new(start_rx),
            handle,
        })
    }

    /// Makes the session reachable from the runtimes of its channels. Called
    /// once, as soon as it is in an `Arc`.
    pub(crate) fn attach(self: &Arc<Self>) {
        let _ = self.handle.set(Arc::downgrade(self));
    }

    pub fn id(&self) -> SessionId {
        self.session_controller.id()
    }

    pub fn get_session(&self) -> arc_swap::Guard<Arc<Session>> {
        self.session_controller.load()
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
        self.get_session().channels.iter().filter_map(|c| self.channel_by_id(*c)).collect()
    }

    pub fn channel_by_id(&self, channel_id: ChannelId) -> Option<Arc<ChannelLoop>> {
        self.channels.get(&channel_id).map(|entry| entry.channel.clone())
    }

    pub async fn channel_by_name(&self, name: &str) -> Option<Arc<ChannelLoop>> {
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
    pub async fn create_channel(&self, context: &SecurityContext, spec: ChannelSpec) -> Result<Arc<ChannelLoop>> {
        let session = self.get_session();
        let name = spec.name.trim().to_string();
        if name.is_empty() {
            return Err(SessionError::EmptyChannelName.into());
        }
        if self.channels.iter().any(|entry| entry.channel.name() == name) {
            return Err(SessionError::DuplicateChannelName { session: self.id, name }.into());
        }
        let confidentiality = spec.confidentiality.unwrap_or(Confidentiality::CHANNEL_DEFAULT);
        if !confidentiality.is_channel_level() {
            return Err(SessionError::InvalidConfidentiality(confidentiality).into());
        }
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

        let channel = Channel::build(self.id, session.security_attributes.owner_id, name, moderator, confidentiality, participants);
        drop(session);
        let store = self.runtime_manager.entity_store();
        store.create_channel(context, &channel).await?;
        if let Err(error) = self.session_controller.add_channel(context, channel.id).await {
            self.discard_channel(channel.id).await;
            return Err(error.into());
        }

        let runtime = ChannelRuntime::new(self.runtime_manager.clone(), self.commands.clone()).with_session(self.handle.clone());
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
        let channel = running.channel();
        if let Err(e) = self.runtime_manager.entity_store().delete_channel(context, channel_id).await {
            warn!("Channel {} was taken out of session {} but not deleted: {}", channel_id, self.id, e);
        }

        let task = self.channels.remove(&channel_id).and_then(|(_, entry)| entry.task);
        let event_bus = self.runtime_manager.event_bus();
        if let Some(task) = task {
            task.abort();
            event_bus.publish(GlobalEvent::ChannelStopped(self.id, channel_id), channel.security_attributes.clone());
        }
        event_bus.publish(GlobalEvent::ChannelDeleted(self.id, channel_id), channel.security_attributes);
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

    pub async fn cancel_channel(&self, channel_id: ChannelId, context: &SecurityContext) -> Result<()> {
        if let Some(c) = self.channel_by_id(channel_id) {
            c.cancel(context).await?;
            Ok(())
        } else {
            Err(anyhow!("Channel not found"))
        }
    }

    /// Runs the session's channels, those it has and those created while it
    /// runs, until the session is stopped - which aborts this, and with it
    /// every channel.
    pub async fn run(&self) -> Result<()> {
        let mut start_rx = self.start_rx.try_lock()
            .map_err(|_| anyhow!("Already running"))?;

        let mut join_set = JoinSet::new();
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
        let session_loop = Arc::new(SessionLoop::new(session, channels, self.runtime_manager.clone()).await?);
        session_loop.attach();
        self.sessions.insert(id, session_loop.clone());

        let loop_for_task = session_loop.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = loop_for_task.run().await {
                tracing::error!("Session loop for {} failed: {}", id, e);
            }
        });
        self.handles.insert(id, handle);

        self.runtime_manager.event_bus().publish(GlobalEvent::SessionStarted(id), attributes);
        Ok(session_loop)
    }

    /// Create a brand-new session with its `channels`: persist them - the
    /// channels as documents of their own (issue 70) - announce their
    /// existence, then start the session.
    pub async fn create_session(&self, ctx: &SecurityContext, session: Session, channels: Vec<Channel>) -> Result<Arc<SessionLoop>> {
        let id = session.id;
        let start = self.start_lock(id);
        let _starting = start.lock().await;

        let store = self.runtime_manager.entity_store();
        store.create_session(ctx, &session).await?;
        for (index, channel) in channels.iter().enumerate() {
            if let Err(error) = store.create_channel(ctx, channel).await {
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

    /// Load an already-persisted session and start it. Fires `SessionStarted`, but not
    /// `SessionCreated` — the session already exists.
    ///
    /// Started once, however many ask at the same time (issue 62): whoever
    /// comes second waits for the first and gets the session it started -
    /// if they may read it.
    pub async fn load_session(&self, id: SessionId, security_context: &SecurityContext) -> Result<Arc<SessionLoop>> {
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        if let Some(running) = self.running(security_context, id).await? {
            return Ok(running);
        }
        let session = self.runtime_manager.entity_store().get_session(security_context, id).await?;
        let channels = self.load_channels(&session).await?;
        self.spawn_session(session, channels).await
    }

    /// The running session `id`, if `context` may read it - asked about
    /// like a stored one, `GetSession` with its attributes, so that it being
    /// in memory is no way around the policy (issue 11).
    async fn running(&self, context: &SecurityContext, id: SessionId) -> Result<Option<Arc<SessionLoop>>> {
        let Some(session_loop) = self.get_session(id) else { return Ok(None) };
        self.may(context, ActionType::GetSession(&session_loop.get_session().security_attributes), id, StoreError::NotFound { entity: "session", id: id.to_string() }).await?;
        Ok(Some(session_loop))
    }

    pub fn get_session(&self, id: SessionId) -> Option<Arc<SessionLoop>> {
        self.sessions.get(&id).map(|s| s.clone())
    }

    /// The session `id` for `context`: the running one, or loaded from the
    /// store and started. A running session is asked about like a stored
    /// one - `GetSession` with its attributes - so that it being in memory is
    /// no way around the policy (issue 11).
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such session or `context` may
    /// not read it; whatever loading or starting it reports otherwise.
    pub async fn open_session(&self, context: &SecurityContext, id: SessionId) -> Result<Arc<SessionLoop>> {
        match self.running(context, id).await? {
            Some(session_loop) => Ok(session_loop),
            None => self.load_session(id, context).await,
        }
    }

    /// Asks the `SecurityManager` about `action` on session `id`, answering a
    /// refusal with `refusal`.
    async fn may(&self, context: &SecurityContext, action: ActionType<'_>, id: SessionId, refusal: StoreError) -> Result<()> {
        if self.runtime_manager.security_manager().check(context, action).await {
            return Ok(());
        }
        info!("Refused access to session {} for {}", id, context.initiator_id);
        Err(refusal.into())
    }

    /// Stop a running session's loop without deleting its durable definition.
    ///
    /// Waits for a start of the same session that is under way (issue 62):
    /// otherwise that start could register the session after it was stopped,
    /// and it would run on.
    pub async fn stop_session(&self, id: SessionId) -> Option<Arc<SessionLoop>> {
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        self.stop_running(id)
    }

    /// Stops session `id` if it runs. Called with its start lock held.
    fn stop_running(&self, id: SessionId) -> Option<Arc<SessionLoop>> {
        if let Some((_, handle)) = self.handles.remove(&id) {
            handle.abort();
        }
        let removed = self.sessions.remove(&id).map(|(_, s)| s);
        if let Some(session_loop) = &removed {
            let attributes = session_loop.get_session().security_attributes.clone();
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
    /// `StoreError::NotFound` if there is no such session or `ctx` may not
    /// read it, `StoreError::PermissionDenied` if they may read but not
    /// delete it.
    pub async fn delete_session(&self, ctx: &SecurityContext, id: SessionId) -> Result<bool> {
        // Held until the session is gone from the store, so that no request
        // starts it again from there in the meantime (issue 62).
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        let session: Session = match self.get_session(id) {
            Some(session_loop) => (**session_loop.get_session()).clone(),
            None => self.runtime_manager.entity_store().get_session(ctx, id).await?,
        };
        let not_found = || StoreError::NotFound { entity: "session", id: id.to_string() };
        self.may(ctx, ActionType::GetSession(&session.security_attributes), id, not_found()).await?;
        self.may(ctx, ActionType::DeleteSession(&session.security_attributes), id, StoreError::PermissionDenied).await?;
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
        let removed = store.delete_session(ctx, id).await?;
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
        state_store.delete_session_state(id).await?;
        event_bus.publish(GlobalEvent::SessionDeleted(id), session.security_attributes.clone());
        self.starts.remove(&id);
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_impl::ollama::OllamaClient;
    use caelumora_model::command::default_commands;
    use caelumora_model::channel::Channel;
    use caelumora_model::meta::{Metadata, Scope};
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{DummySecurityManager, SecurityAttributes};
    use caelumora_model::store_impl::memory::MemoryStore;
    use uuid::Uuid;

    fn test_runtime() -> Arc<RuntimeManager> {
        let store = Arc::new(MemoryStore::new());
        let secret_manager = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
        Arc::new(RuntimeManager::new(store.clone(), store, secret_manager, Arc::new(DummySecurityManager::new()), default_commands(), Arc::new(OllamaClient::default())).unwrap())
    }

    fn make_session(name: &str) -> Session {
        Session {
            commands: Default::default(),
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
            confidentiality: caelumora_model::confidentiality::Confidentiality::CHANNEL_DEFAULT,
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
        assert!(manager.get_session(session_id).is_some());

        manager.stop_session(session_id).await;
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
        assert!(manager.get_session(session_id).is_none());
    }

    #[tokio::test]
    async fn load_session_emits_started_but_not_created() {
        let runtime = test_runtime();
        let manager = SessionManager::new(runtime.clone());
        let ctx = SecurityContext::default();

        // Persist a session directly, without going through create_session.
        let session = make_session("s3");
        let session_id = session.id;
        persist(&runtime, &ctx, &session).await;

        let mut rx = runtime.event_bus().subscribe_all();
        manager.load_session(session_id, &ctx).await.unwrap();

        let events = drain(&mut rx);
        assert!(
            events.iter().any(|e| matches!(e, GlobalEvent::SessionStarted(id) if *id == session_id)),
            "expected SessionStarted, got {:?}", events
        );
        assert!(
            !events.iter().any(|e| matches!(e, GlobalEvent::SessionCreated(_))),
            "load must not emit SessionCreated, got {:?}", events
        );

        manager.stop_session(session_id).await;
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
            manager.load_session(session_id, &ctx),
        );
        let (first, second, third) = (first.unwrap(), second.unwrap(), third.unwrap());
        assert!(Arc::ptr_eq(&first, &second) && Arc::ptr_eq(&first, &third), "one loop for all of them");

        let started = drain(&mut rx).into_iter().filter(|e| matches!(e, GlobalEvent::SessionStarted(id) if *id == session_id)).count();
        assert_eq!(started, 1, "the session started once");
        manager.stop_session(session_id).await;
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
            async move { manager.stop_session(session_id).await.is_some() }
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!stopping.is_finished(), "the stop waits for the start");

        // ... and registers the session before it lets go: the stop then
        // finds it, and nothing keeps running.
        let channels = vec![main_channel(&session)];
        manager.spawn_session(session, channels).await.unwrap();
        drop(starting);
        assert!(stopping.await.unwrap(), "the stop stopped what the start started");
        assert!(manager.get_session(session_id).is_none());
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
                deps: Vec::new(),
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
        use caelumora_model::session::Participant;
        use caelumora_model::user::{User, UserId};
        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new() };
        let (owner, other) = (user("owner"), user("other"));
        let (owner_id, other_id) = (ActorId::User(owner.id), ActorId::User(other.id));
        let mut session = make_session("with-users");
        session.participants = vec![Participant::User(owner), Participant::User(other)];
        session.security_attributes = SecurityAttributes { owner_id, contributors: Vec::new() };
        let ctx = SecurityContext::new(owner_id, session.id, Default::default());
        let session_loop = create(&manager, &ctx, session).await.expect("the session starts");
        (session_loop, ctx, other_id)
    }

    fn spec(name: &str, participants: Vec<caelumora_model::actor::ActorId>) -> ChannelSpec {
        ChannelSpec { name: name.to_string(), moderator: None, participants, confidentiality: None }
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

        manager.stop_session(session.id()).await;
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
        manager.stop_session(session.id()).await;
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
        let main = session.channel_by_name("main").await.unwrap().id();
        session.delete_channel(&ctx, main).await.expect("the last channel may be deleted");
        assert!(session.channels().is_empty());
        session.create_channel(&ctx, spec("again", vec![])).await.expect("and a new one created");
        manager.stop_session(session.id()).await;
    }

    #[tokio::test]
    async fn a_channel_deleted_before_the_session_runs_is_never_started() {
        let runtime = test_runtime();
        let ctx = SecurityContext::default();
        let session = make_session("waiting");
        let channel_id = session.channels[0];
        persist(&runtime, &ctx, &session).await;
        let session_loop = Arc::new(SessionLoop::new(session.clone(), vec![main_channel(&session)], runtime.clone()).await.unwrap());
        session_loop.attach();
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
        use caelumora_model::session::Participant;
        use caelumora_model::user::{User, UserId};

        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store, secrets, policy, default_commands(), Arc::new(OllamaClient::default())).unwrap());
        let manager = SessionManager::new(runtime);

        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new() };
        let (alice, bob) = (user("alice"), user("bob"));
        let as_user = |id: UserId| SecurityContext::new(ActorId::User(id), Default::default(), Default::default());
        let (alice_ctx, bob_ctx) = (as_user(alice.id), as_user(bob.id));
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes { owner_id: ActorId::User(alice.id), contributors: vec![ActorId::User(bob.id)] };
        session.participants = vec![Participant::User(alice), Participant::User(bob)];
        let session = create(&manager, &alice_ctx, session).await.unwrap();
        let main = session.channel_by_name("main").await.unwrap().id();

        let refused = session.create_channel(&bob_ctx, spec("bobs", vec![])).await.expect_err("a contributor may not");
        assert!(crate::error::is_forbidden(&refused), "{refused}");
        let refused = session.delete_channel(&bob_ctx, main).await.expect_err("a contributor may not");
        assert!(crate::error::is_forbidden(&refused), "{refused}");
        assert!(session.channel_by_id(main).is_some(), "a refused deletion leaves the channel");

        session.create_channel(&alice_ctx, spec("hers", vec![])).await.expect("the owner may");
        manager.stop_session(session.id()).await;
    }

    #[tokio::test]
    async fn a_channel_reads_another_by_their_confidentiality_not_by_who_takes_part() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::session::Participant;
        use caelumora_model::user::{User, UserId};

        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store, secrets, policy, default_commands(), Arc::new(OllamaClient::default())).unwrap());
        let manager = SessionManager::new(runtime.clone());

        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new() };
        let (alice, bob) = (user("alice"), user("bob"));
        let alice_id = ActorId::User(alice.id);
        let bob_id = ActorId::User(bob.id);
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default());
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes { owner_id: alice_id, contributors: vec![bob_id] };
        session.participants = vec![Participant::User(alice), Participant::User(bob)];
        let session = create(&manager, &alice_ctx, session).await.unwrap();

        let with = |name: &str, level, participants| ChannelSpec { confidentiality: Some(level), ..spec(name, participants) };
        let public = session.create_channel(&alice_ctx, with("public", Confidentiality::Public, vec![bob_id])).await.unwrap();
        let team = session.create_channel(&alice_ctx, with("team", Confidentiality::Session, vec![bob_id])).await.unwrap();
        let secret = session.create_channel(&alice_ctx, with("secret", Confidentiality::Channel, vec![])).await.unwrap();

        let security = runtime.security_manager();
        let may_read = |reader: &Arc<ChannelLoop>, source: &Arc<ChannelLoop>| {
            let (reader, source, security) = (reader.clone(), source.clone(), security.clone());
            async move {
                use caelumora_model::security::AccessQuestions;
                security.may_channel_read(&reader.classification(), reader.security_attributes().owner_id, &source.classification()).await
            }
        };
        assert!(may_read(&secret, &secret).await, "a channel reads itself");
        assert!(may_read(&team, &public).await);
        assert!(!may_read(&public, &team).await, "what public passes on may leave the session");
        assert!(!may_read(&team, &secret).await, "secret stays in its channel");
        assert!(may_read(&secret, &team).await, "although bob, who is in team, is not in secret");

        let refused = session.create_channel(&alice_ctx, with("mine", Confidentiality::Private, vec![])).await.expect_err("private is an actor's");
        assert!(matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::InvalidConfidentiality(_))), "{refused}");
        manager.stop_session(session.id()).await;
    }

    #[tokio::test]
    async fn deleting_a_session_deletes_its_state_from_the_state_store() {
        use caelumora_model::store::StateStore;
        let entities = Arc::new(MemoryStore::new());
        let states = Arc::new(MemoryStore::new());
        let secret_manager = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
        let runtime = Arc::new(RuntimeManager::new(states.clone(), entities.clone(), secret_manager, Arc::new(DummySecurityManager::new()), default_commands(), Arc::new(OllamaClient::default())).unwrap());
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
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store.clone(), secrets, policy, default_commands(), Arc::new(OllamaClient::default())).unwrap());
        let manager = SessionManager::new(runtime);
        let (alice, bob) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        let as_user = |actor| SecurityContext::new(actor, Default::default(), Default::default());
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes { owner_id: alice, contributors: vec![bob] };
        let running = create(&manager, &as_user(alice), session).await.unwrap();
        let main = running.channel_by_name("main").await.unwrap();

        let refused = main.update_name(&as_user(bob), "bobs".to_string()).await.expect_err("a contributor may not");
        assert!(crate::error::is_forbidden(&refused), "{refused}");
        assert_eq!(main.name(), "main", "unchanged");

        main.update_name(&as_user(alice), "lobby".to_string()).await.expect("the owner may");
        assert_eq!(main.name(), "lobby");
        assert_eq!(store.get_channel(&as_user(alice), main.id()).await.unwrap().name, "lobby", "stored, not only in memory");
        manager.stop_session(running.id()).await;
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
            writer.send_message(text.to_string()).await.unwrap();
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
        writer.send_message("four".to_string()).await.unwrap();
        history_of(&channel, 4).await;
        assert_eq!(channel.visible_history(other).await.messages.len(), 2);

        let refusal = |error: anyhow::Error| error.downcast::<ChannelError>().expect("a ChannelError");
        let again = channel.add_participant(&ctx, other, HistoryAccess::All).await.expect_err("already in");
        assert!(matches!(refusal(again), ChannelError::AlreadyParticipant { .. }));
        let stranger = caelumora_model::actor::ActorId::User(caelumora_model::user::UserId::new());
        let outside = channel.add_participant(&ctx, stranger, HistoryAccess::All).await.expect_err("not in the session");
        assert!(matches!(refusal(outside), ChannelError::NotInSession { .. }));
        manager.stop_session(session.id()).await;
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
        manager.stop_session(session.id()).await;
    }

    #[tokio::test]
    async fn only_who_may_change_a_channel_adds_or_removes_its_participants() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::channel::HistoryAccess;
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::session::Participant;
        use caelumora_model::user::{User, UserId};

        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store, secrets, policy, default_commands(), Arc::new(OllamaClient::default())).unwrap());
        let manager = SessionManager::new(runtime);
        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new() };
        let (alice, bob, carol) = (user("alice"), user("bob"), user("carol"));
        let (alice_id, bob_id, carol_id) = (ActorId::User(alice.id), ActorId::User(bob.id), ActorId::User(carol.id));
        let as_user = |actor| SecurityContext::new(actor, Default::default(), Default::default());
        let mut session = make_session("hers");
        session.participants = vec![Participant::User(alice), Participant::User(bob), Participant::User(carol)];
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
        manager.stop_session(running.id()).await;
    }

    #[tokio::test]
    async fn a_running_session_is_no_way_around_the_policy() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::UserId;

        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store, secrets, policy, default_commands(), Arc::new(OllamaClient::default())).unwrap());
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
        assert!(manager.get_session(id).is_some(), "a refused deletion must not stop the session on the way");

        manager.delete_session(&as_user(alice), id).await.expect("the owner deletes it");
        assert!(manager.get_session(id).is_none());
    }
}
