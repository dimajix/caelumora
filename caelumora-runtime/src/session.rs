use caelumora_model::approval::ApprovalRule;
use caelumora_model::name::Name;
use crate::channel::ChannelLoop;
use crate::command::CommandExecutor;
use crate::runtime::{ChannelRuntime, RuntimeManager};
use crate::event::EventBus;
use anyhow::{anyhow, Result};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentId};
use caelumora_model::channel::Channel;
use caelumora_model::component::ComponentConfig;
use caelumora_model::confidentiality::Classification;
use caelumora_model::language::Language;
use caelumora_model::event::GlobalEvent;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, SecurityAttributes, SecurityContext, SecurityManager};
use crate::view::{Viewer, Views};
use caelumora_model::privacy::{PrivacyConfig, SessionPrivacy};
use caelumora_model::store::{EntityStore, StoreError, StoreResult, StoreResultExt};
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
    #[error("Session {session} already has a channel named \"{name}\"")]
    DuplicateChannelName { session: SessionId, name: Name },
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

/// Nothing if the user `context` acts for is cleared for `channel` (issue
/// 133); `refusal` otherwise - reading a channel one is not cleared for is
/// the same as reading one that does not exist.
async fn require_seen<E: Into<anyhow::Error> + Send>(runtime_manager: &RuntimeManager, context: &SecurityContext, channel: &Classification, refusal: impl FnOnce() -> E + Send) -> Result<()> {
    let roles = caelumora_model::confidentiality::user_roles(runtime_manager.entity_store().as_ref(), context.initiator_id).await?;
    if runtime_manager.security_manager().may_see_channel(context, &roles, channel).await {
        Ok(())
    } else {
        Err(refusal().into())
    }
}

/// What a new channel should be (issue 09).
#[derive(Debug, Clone)]
pub struct ChannelSpec {
    pub name: Name,
    /// The channel's components (issue 174): its moderator and the policy
    /// for the moderator's slot; the defaults for what is left out, as in a
    /// template.
    pub components: ComponentConfig,
    /// Who takes part, from the session's participants. Whoever creates the
    /// channel takes part anyway, if they are a participant of the session.
    pub participants: Vec<ActorId>,
    /// The classification of the channel's contents (issue 133); `None`
    /// for the default, as in a template. Set by whoever may create the
    /// channel - its owner.
    pub classification: Option<Classification>,
    /// The language its conversation is held in (issue 56); `None` for the
    /// one its creator prefers, if they prefer one.
    pub language: Option<Language>,
}

/// The language the user `context` acts for prefers (issue 56), which
/// what they create is held in unless they say otherwise; none if they
/// prefer none or act for no user.
///
/// # Errors
/// Whatever reading their profile reports, except that there is none.
async fn preferred_language(store: &dyn EntityStore, context: &SecurityContext) -> StoreResult<Option<Language>> {
    let Some(user) = context.initiator_id.as_user_id() else { return Ok(None) };
    let profile = store.get_user_profile(context, user).await.optional()?;
    Ok(profile.and_then(|profile| profile.preferred_language))
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
    entities: Arc<dyn EntityStore>,
    events: Arc<EventBus>,
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
            entities: Arc::clone(runtime_manager.entity_store()),
            events: Arc::clone(runtime_manager.event_bus()),
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

    /// Changes whom the session lets read what was said in it (issue 134)
    /// on behalf of `context`. What was remembered before keeps the privacy
    /// it was said under: a wider policy lets go only what is said from now
    /// on, a narrower one takes back what it let go.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// session, or whatever else storing it reports.
    pub async fn update_privacy(&self, context: &SecurityContext, privacy: PrivacyConfig) -> Result<()> {
        Ok(self.session_controller.update_privacy(context, privacy).await?)
    }

    /// Sets what the session restricts of the tool calls made in any of its
    /// channels (issue 13) on behalf of `context`: stored first, which is
    /// where the permission is checked (`UpdateSession`). Its agents' next
    /// calls are decided by them.
    ///
    /// # Errors
    /// `StoreError::Validation` if a rule does not only ask or deny,
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// session, or whatever else storing it reports.
    pub async fn update_approval_rules(&self, context: &SecurityContext, rules: Vec<ApprovalRule>) -> Result<()> {
        let count = rules.len();
        self.session_controller.update_approval_rules(context, rules).await?;
        info!(session = %self.id, rules = count, by = %context.caller_id, "Session approval rules changed");
        Ok(())
    }

    /// Says what the session is for (issue 245) on behalf of `context`:
    /// stored first, which is where the permission is checked
    /// (`UpdateSession`). The agents of all its channels are told from their
    /// next turn on.
    ///
    /// # Errors
    /// `StoreError::Validation` if `summary` or `briefing` is too long or
    /// holds what it may not, `StoreError::PermissionDenied` if `context`
    /// may not change the session, or whatever else storing it reports.
    pub async fn update_description(&self, context: &SecurityContext, summary: Option<&str>, briefing: Option<&str>) -> Result<()> {
        self.session_controller.update_description(context, summary, briefing).await?;
        info!(session = %self.id, by = %context.caller_id, "Session description changed");
        Ok(())
    }

    /// The running channels, in the order of the session: those from its
    /// template first, then those created later, oldest first.
    pub fn channels(&self) -> Vec<Arc<ChannelLoop>> {
        self.session().channels.iter().filter_map(|c| self.channel_by_id(*c)).collect()
    }

    pub fn channel_by_id(&self, channel_id: ChannelId) -> Option<Arc<ChannelLoop>> {
        self.channels.get(&channel_id).map(|entry| Arc::clone(&entry.channel))
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
        self.security
            .require(context, ActionType::GetChannel(&channel.security_attributes()), not_found).await?;
        require_seen(&self.runtime_manager, context, &channel.classification(), not_found).await?;
        Ok(channel)
    }

    pub fn channel_by_name(&self, name: &str) -> Option<Arc<ChannelLoop>> {
        self.channels.iter().find(|entry| entry.channel.name() == name).map(|entry| Arc::clone(&entry.channel))
    }

    /// Creates a channel in the running session on behalf of `context` and
    /// starts it (issue 09). The channel is a document of its own (issue 70),
    /// stored first (`CreateChannel`), and then listed in the session, which
    /// takes the permission to change the session (`UpdateSession`); the
    /// channel belongs to the session's owner, like the session's others.
    ///
    /// # Errors
    /// - [`SessionError`] if `spec` asks for something that cannot be: no
    ///   name, a name the session already has, components that make no
    ///   moderator, or a
    ///   participant who is not one of the session's.
    /// - `StoreError::PermissionDenied` if `context` may not change the
    ///   session; nothing is changed then.
    pub async fn create_channel(self: &Arc<Self>, context: &SecurityContext, spec: ChannelSpec) -> Result<Arc<ChannelLoop>> {
        let session = Arc::clone(&self.session());
        // Whether the actor may change the session at all, before anything
        // is asked of the others in the channel (issue 198, F2): who may not
        // learns nothing of their clearance.
        self.security
            .require(context, ActionType::UpdateSession(&session.security_attributes), || StoreError::PermissionDenied).await?;
        let name = spec.name;
        if self.channels.iter().any(|entry| entry.channel.name() == name) {
            return Err(SessionError::DuplicateChannelName { session: self.id, name }.into());
        }
        let classification = spec.classification.unwrap_or_default();
        require_classification(&self.runtime_manager, context, &classification).await?;
        let language = match spec.language {
            Some(language) => Some(language),
            None => preferred_language(self.entities.as_ref(), context).await?,
        };

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

        let channel = Channel::build(self.id, session.security_attributes.owner_id, name, spec.components, classification, language, participants);
        crate::channel::require_cleared(&self.runtime_manager, context, &channel).await?;
        let store = &self.entities;
        store.create_channel(context, &channel).await?;
        if let Err(error) = self.session_controller.add_channel(context, channel.id).await {
            self.discard_channel(channel.id).await;
            return Err(error.into());
        }

        let channel_loop = match self.add_channel_loop(&channel).await {
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
        self.events.publish(GlobalEvent::ChannelCreated(self.id, channel.id), channel.security_attributes.clone());
        info!("Channel {}/{} created in session {}", channel.name, channel.id, self.id);
        Ok(channel_loop)
    }

    /// Builds the loop of `channel`, which the session lists, and has the
    /// session run it.
    async fn add_channel_loop(self: &Arc<Self>, channel: &Channel) -> Result<Arc<ChannelLoop>> {
        let runtime = ChannelRuntime::new(Arc::clone(&self.runtime_manager), Arc::clone(&self.commands), Arc::clone(&self.session_controller));
        let channel_loop = ChannelLoop::new(channel.clone(), runtime).await?;
        channel_loop.attach(self);
        self.channels.insert(channel.id, ChannelEntry { channel: Arc::clone(&channel_loop), task: None });
        let _ = self.start_tx.send(channel.id);
        Ok(channel_loop)
    }

    /// Stops and deletes the channel `channel_id` on behalf of `context`.
    /// Deleting marks (issue 164): the channel is taken out of the session
    /// and its document marked, but it and its state stay stored - it can
    /// be restored (`restore_channel`) - until it is purged with its session
    /// (`SessionManager::purge_session`). Asked about first, like creating
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
        if let Err(e) = self.entities.delete_channel(context, channel_id).await {
            warn!("Channel {} was taken out of session {} but not marked deleted: {}", channel_id, self.id, e);
        }

        let task = self.channels.remove(&channel_id).and_then(|(_, entry)| entry.task);
        // Given the time to end by themselves before the channel's task -
        // and with it whoever did not - is aborted (issue 187): they store
        // their state on their way out, which stays.
        running.stop_actors().await;
        let event_bus = &self.events;
        if let Some(task) = task {
            task.abort();
            event_bus.publish(GlobalEvent::ChannelStopped(self.id, channel_id), channel.security_attributes.clone());
        }
        event_bus.publish(GlobalEvent::ChannelDeleted(self.id, channel_id), channel.security_attributes.clone());
        info!("Channel {}/{} deleted from session {} by {}", channel.name, channel_id, self.id, context.caller_id);
        Ok(())
    }

    /// Restores the deleted channel `channel_id` of this session on behalf
    /// of `context` (issue 164): its mark taken away, listed in the session
    /// again and started from its stored state. Asks to change the session
    /// (`UpdateSession`) and its participants' clearance, as creating one
    /// does, and the store asks to restore it (`Restore`).
    ///
    /// # Errors
    /// - `SessionError::ChannelNotFound` if the session has no such deleted
    ///   channel, or `context` may not see it.
    /// - `SessionError::DuplicateChannelName` if its name was taken since.
    /// - `StoreError::PermissionDenied` if `context` may not change the
    ///   session or restore the channel; nothing is changed then.
    pub async fn restore_channel(self: &Arc<Self>, context: &SecurityContext, channel_id: ChannelId) -> Result<Arc<ChannelLoop>> {
        let session = Arc::clone(&self.session());
        self.security
            .require(context, ActionType::UpdateSession(&session.security_attributes), || StoreError::PermissionDenied).await?;
        let not_found = || SessionError::ChannelNotFound { session: self.id, channel: channel_id };
        let store = &self.entities;
        let channel = match store.get_deleted_channel(context, channel_id).await.optional()? {
            Some(channel) if channel.session_id == self.id => channel,
            _ => return Err(not_found().into()),
        };
        if self.channels.iter().any(|entry| entry.channel.name() == channel.name) {
            return Err(SessionError::DuplicateChannelName { session: self.id, name: channel.name }.into());
        }
        // As when it was created: its participants' clearance may have
        // changed while it was deleted.
        crate::channel::require_cleared(&self.runtime_manager, context, &channel).await?;
        store.restore_channel(context, channel_id).await?;
        let channel = Channel { deleted: None, ..channel };
        let listed = match self.session_controller.add_channel(context, channel_id).await {
            Ok(()) => self.add_channel_loop(&channel).await,
            Err(error) => Err(error.into()),
        };
        let channel_loop = match listed {
            Ok(channel_loop) => channel_loop,
            Err(error) => {
                // Deleted again, so that it is neither half back nor lost.
                let _ = self.session_controller.remove_channel(context, channel_id).await;
                if let Err(undo) = store.delete_channel(&SecurityContext::system(), channel_id).await {
                    warn!("Channel {} of session {} could not be restored and not be marked deleted again: {}", channel_id, self.id, undo);
                }
                return Err(error);
            }
        };
        self.events.publish(GlobalEvent::ChannelRestored(self.id, channel_id), channel.security_attributes.clone());
        info!("Channel {}/{} restored in session {} by {}", channel.name, channel_id, self.id, context.caller_id);
        Ok(channel_loop)
    }

    /// Deletes the stored channel `channel_id` again after creating it failed
    /// half-way - in the runtime's name: its creation was permitted.
    async fn discard_channel(&self, channel_id: ChannelId) {
        if let Err(e) = purge_channel_document(self.entities.as_ref(), channel_id).await {
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
        let mut global_rx = self.events.subscribe_session(self.id());

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
                                });
                            // Gone: a deleted channel, whose actors all
                            // stopped in time; `delete_channel` announces it.
                            if let Some(attributes) = attributes {
                                self.events.publish(GlobalEvent::ChannelStopped(self.id, channel_id), attributes);
                            }
                        }
                        // A deleted channel; `delete_channel` announces it.
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
        let channel = Arc::clone(&entry.channel);
        self.events.publish(GlobalEvent::ChannelStarted(self.id, id), channel.security_attributes());
        entry.task = Some(join_set.spawn(async move {
            let _ = channel.run().await;
            channel.id()
        }));
    }
}

/// Removes the document of channel `channel_id` for good, in the runtime's
/// name: marked deleted first if it is not - a channel of a session being
/// purged, which is hidden with the session, or one whose creation failed
/// - since only what was deleted is purged (issue 164).
async fn purge_channel_document(store: &dyn EntityStore, channel_id: ChannelId) -> StoreResult<bool> {
    let system = SecurityContext::system();
    store.delete_channel(&system, channel_id).await?;
    store.purge_channel(&system, channel_id).await
}

pub struct SessionManager {
    sessions: DashMap<SessionId, Arc<SessionLoop>>,
    handles: DashMap<SessionId, JoinHandle<()>>,
    /// One lock per session id, held while that session is being started,
    /// so that a session starts only once however many requests ask for it
    /// at the same time (issue 62): two loops of one session would run two
    /// actors per agent and channel, which overwrite each other's state.
    starts: DashMap<SessionId, Arc<tokio::sync::Mutex<()>>>,
    /// What builds the loops of the sessions it starts.
    runtime_manager: Arc<RuntimeManager>,
    /// The services it works with, taken from `runtime_manager` once (issue
    /// 229).
    security: Arc<dyn SecurityManager>,
    entities: Arc<dyn EntityStore>,
    events: Arc<EventBus>,
}

/// A session as [`SessionManager::find`] found it for a context that may
/// read it: running or not, each asked about alike.
pub(crate) enum Found {
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
            security: Arc::clone(runtime_manager.security_manager()),
            entities: Arc::clone(runtime_manager.entity_store()),
            events: Arc::clone(runtime_manager.event_bus()),
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
        let session_loop = SessionLoop::new(session, channels, Arc::clone(&self.runtime_manager)).await?;
        self.sessions.insert(id, Arc::clone(&session_loop));

        let loop_for_task = Arc::clone(&session_loop);
        let handle = tokio::spawn(async move {
            if let Err(e) = loop_for_task.run().await {
                tracing::error!("Session loop for {} failed: {}", id, e);
            }
        });
        self.handles.insert(id, handle);

        self.events.publish(GlobalEvent::SessionStarted(id), attributes);
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
    pub(crate) async fn find(&self, context: &SecurityContext, id: SessionId) -> Result<Found> {
        let running = self.sessions.get(&id).map(|entry| entry.clone());
        match running {
            Some(session_loop) => {
                let attributes = session_loop.session().security_attributes.clone();
                self.security
                    .require(context, ActionType::GetSession(&attributes), || StoreError::NotFound { entity: "session", id: id.to_string() }).await?;
                Ok(Found::Running(session_loop))
            }
            None => Ok(Found::Stored(self.entities.get_session(context, id).await?)),
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
        let store = &self.entities;
        let template = store.get_session_template(context, template_id).await?;
        let id = SessionId::new();
        let participants = crate::template::resolve_participants(store.as_ref(), context, &template, id, owner).await?;
        let name = name.unwrap_or_else(|| template.name.to_string());
        let language = preferred_language(store.as_ref(), context).await?;
        let (session, channels) = template.build(id, owner, name, language.as_ref(), participants)?;
        self.create_session(context, session, channels).await
    }

    /// Create a brand-new session with its `channels`: persist them - the
    /// channels as documents of their own (issue 70) - announce their
    /// existence, then start the session.
    pub(crate) async fn create_session(&self, context: &SecurityContext, session: Session, channels: Vec<Channel>) -> Result<Arc<SessionLoop>> {
        let id = session.id;
        let start = self.start_lock(id);
        let _starting = start.lock().await;

        for channel in &channels {
            require_classification(&self.runtime_manager, context, &channel.classification).await?;
            crate::channel::require_cleared(&self.runtime_manager, context, channel).await?;
        }
        let store = &self.entities;
        store.create_session(context, &session).await?;
        for (index, channel) in channels.iter().enumerate() {
            if let Err(error) = store.create_channel(context, channel).await {
                // Nothing half-made stays behind; the session's creation was
                // permitted, so it is taken back in the runtime's name.
                let system = SecurityContext::system();
                for created in &channels[..index] {
                    let _ = purge_channel_document(store.as_ref(), created.id).await;
                }
                if store.delete_session(&system, id).await.is_ok() {
                    let _ = store.purge_session(&system, id).await;
                }
                return Err(error.into());
            }
        }

        let event_bus = &self.events;
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
        let store = &self.entities;
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
        self.security
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
            self.events.publish(GlobalEvent::SessionStopped(id), attributes);
        }
        removed
    }

    /// Stops and deletes a session. Deleting marks (issue 164): the session
    /// is kept, with its channels and their state, hidden until it is
    /// restored (`restore_session`) or purged (`purge_session`).
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
        self.security
            .require(context, ActionType::DeleteSession(&session.security_attributes), || StoreError::PermissionDenied).await?;
        // Captured before the runtime loop is torn down.
        // The events carry the attributes as they were: once deleted, the
        // session is nowhere to look them up (issue 39).
        let channels: Vec<(ChannelId, SecurityAttributes)> = self.load_channels(&session).await?.into_iter()
            .map(|c| (c.id, c.security_attributes))
            .collect();

        self.stop_running(id);

        let event_bus = &self.events;
        for (channel_id, attributes) in channels {
            event_bus.publish(GlobalEvent::ChannelDeleted(id, channel_id), attributes);
        }

        // Marked, not removed (issue 164): its channels and their state stay
        // as they are, hidden with it - a channel is reached only through
        // its session - until it is purged.
        let removed = self.entities.delete_session(context, id).await?;
        event_bus.publish(GlobalEvent::SessionDeleted(id), session.security_attributes.clone());
        self.starts.remove(&id);
        Ok(removed)
    }

    /// Restores the deleted session `id` on behalf of `context` (issue 164):
    /// its mark taken away, with the channels it listed when it was
    /// deleted. It is not started; opening it starts it, as any stored
    /// session.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such deleted session or
    /// `context` may not see it, `StoreError::PermissionDenied` if they may
    /// but not restore it.
    pub async fn restore_session(&self, context: &SecurityContext, id: SessionId) -> Result<Session> {
        // The lock of deleting and purging it: a purge never meets it half
        // restored.
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        let store = &self.entities;
        store.restore_session(context, id).await?;
        let session = store.get_session(context, id).await?;
        self.events.publish(GlobalEvent::SessionRestored(id), session.security_attributes.clone());
        info!("Session {} restored by {}", id, context.caller_id);
        Ok(session)
    }

    /// Removes the deleted session `id` for good on behalf of `context`
    /// (issue 164): its document, and - in the runtime's name, since the
    /// purge was permitted - the documents and the state of its channels,
    /// those it listed and those deleted from it before. Only what was
    /// deleted is purged; the store asks `Purge`. Under the lock of
    /// deleting and restoring it, so that what is restored meanwhile is not
    /// purged.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such deleted session or
    /// `context` may not see it, `StoreError::PermissionDenied` if they may
    /// but not purge it.
    pub async fn purge_session(&self, context: &SecurityContext, id: SessionId) -> Result<()> {
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        let store = &self.entities;
        let session = store.get_deleted_session(context, id).await?;
        if !store.purge_session(context, id).await? {
            return Err(StoreError::NotFound { entity: "session", id: id.to_string() }.into());
        }
        let mut channels = session.channels.clone();
        channels.extend(store.list_deleted_channels(&SecurityContext::system()).await?.into_iter().filter(|c| c.session_id == id).map(|c| c.id));
        // The state lives in the `StateStore`, which may be another store
        // than the session's.
        let state_store = self.runtime_manager.state_store();
        for channel_id in channels {
            purge_channel_document(store.as_ref(), channel_id).await?;
            state_store.delete_channel_state(channel_id).await?;
        }
        self.starts.remove(&id);
        info!("Session {} purged by {}", id, context.caller_id);
        Ok(())
    }

    /// Deletes agent `id` on behalf of `context` (issue 164): it leaves
    /// every channel it takes part in - kept there as a former participant,
    /// removed in the runtime's name, since deleting the agent was
    /// permitted - and is marked deleted, with its state kept until it is
    /// purged. What it said stays in the channels' histories. Asked about
    /// before anything happens (`DeleteAgent`).
    ///
    /// Only the channels of sessions it takes part in are looked at: an
    /// agent joins a channel only from its session. Those of deleted
    /// sessions keep it; it is left out of them if they are restored, as
    /// any participant that no longer exists.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such agent or `context` may
    /// not read it, `StoreError::PermissionDenied` if they may but not
    /// delete it.
    pub async fn delete_agent(&self, context: &SecurityContext, id: AgentId) -> Result<()> {
        let store = &self.entities;
        let agent = store.get_agent(context, id).await?;
        self.security
            .require(context, ActionType::DeleteAgent(&agent.security_attributes), || StoreError::PermissionDenied).await?;
        let actor = ActorId::Agent(id);
        let system = SecurityContext::system();
        for session in store.list_sessions(&system).await? {
            if !session.participants.iter().any(|p| p.actor_id() == actor) {
                continue;
            }
            // Held so that the session does not start while its channels'
            // documents are changed here.
            let start = self.start_lock(session.id);
            let _starting = start.lock().await;
            match self.running(session.id) {
                Some(running) => {
                    for channel in running.channels() {
                        if channel.channel().participants.iter().any(|p| p.actor_id() == actor) {
                            if let Err(e) = channel.remove_participant(&system, actor).await {
                                warn!("Deleted agent {} could not leave channel {}: {:#}", id, channel.id(), e);
                            }
                        }
                    }
                }
                None => {
                    for channel in self.load_channels(&session).await? {
                        if channel.participants.iter().any(|p| p.actor_id() == actor) {
                            let channel_id = channel.id;
                            if let Err(e) = self.runtime_manager.controller_manager().new_channel_controller(channel).remove_participant(&system, actor).await {
                                warn!("Deleted agent {} could not leave channel {}: {:#}", id, channel_id, e);
                            }
                        }
                    }
                }
            }
        }
        store.delete_agent(context, id).await?;
        self.events.publish(GlobalEvent::AgentDeleted(id), agent.security_attributes.clone());
        info!("Agent {} deleted by {}", id, context.caller_id);
        Ok(())
    }

    /// Restores the deleted agent `id` on behalf of `context` (issue 164):
    /// its mark taken away, with its state. It takes part in no channel
    /// again - it left them when it was deleted - until it is added.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such deleted agent or
    /// `context` may not see it, `StoreError::PermissionDenied` if they may
    /// but not restore it, `StoreError::Conflict` if another agent took its
    /// name in its scope meanwhile.
    pub async fn restore_agent(&self, context: &SecurityContext, id: AgentId) -> Result<Agent> {
        let store = &self.entities;
        let deleted = store.get_deleted_agent(context, id).await?;
        if store.list_agents(&SecurityContext::system()).await?.iter().any(|a| a.name == deleted.name) {
            return Err(StoreError::Conflict(format!("agent {} is taken meanwhile", deleted.name)).into());
        }
        store.restore_agent(context, id).await?;
        let agent = Agent { deleted: None, ..deleted };
        self.events.publish(GlobalEvent::AgentRestored(id), agent.security_attributes.clone());
        info!("Agent {} restored by {}", id, context.caller_id);
        Ok(agent)
    }

    /// Removes the deleted agent `id` for good on behalf of `context` (issue
    /// 164): its document, its own state and its state in every channel.
    /// Only what was deleted is purged; the store asks `Purge`. What it
    /// said stays in the channels' histories, by an agent that no longer
    /// exists.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such deleted agent or
    /// `context` may not see it, `StoreError::PermissionDenied` if they may
    /// but not purge it.
    pub async fn purge_agent(&self, context: &SecurityContext, id: AgentId) -> Result<()> {
        if !self.entities.purge_agent(context, id).await? {
            return Err(StoreError::NotFound { entity: "agent", id: id.to_string() }.into());
        }
        self.runtime_manager.state_store().delete_agent_state(id).await?;
        info!("Agent {} purged by {}", id, context.caller_id);
        Ok(())
    }

    /// The root of the views (issue 163): every session `viewer` may
    /// read, and from there their channels and members, running or not.
    pub fn views(self: &Arc<Self>, viewer: Viewer) -> Views {
        Views::new(&self.runtime_manager, Arc::<SessionManager>::clone(self), viewer)
    }

    /// The loop of session `id`, if it runs.
    pub(crate) fn running(&self, id: SessionId) -> Option<Arc<SessionLoop>> {
        self.sessions.get(&id).map(|entry| entry.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::name::{Name, ScopedName};
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::command::default_commands;
    use caelumora_model::channel::Channel;
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
        Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secret_manager, Arc::new(DummySecurityManager::new()), default_commands(), Default::default(), Vec::new(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap())
    }

    fn make_session(name: &str) -> Session {
        Session {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            commands: Default::default(),
            privacy: Default::default(),
            id: SessionId(Uuid::new_v4()),
            name: name.to_string(),
            participants: Vec::new(),
            channels: vec![ChannelId(Uuid::new_v4())],
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            security_attributes: SecurityAttributes::default(),
            deleted: None,
        }
    }

    /// The channel `session` lists, as a document of its own (issue 70):
    /// "main", belonging to whom the session belongs to.
    fn main_channel(session: &Session) -> Channel {
        Channel {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            id: session.channels[0],
            session_id: session.id,
            name: Name::new("main").unwrap(),
            components: caelumora_model::channel::moderation_components(None, Some("broadcast_policy")),
            classification: caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT,
            language: None,
            history_limits: Vec::new(),
            participants: Vec::new(),
            security_attributes: session.security_attributes.clone(),
            deleted: None,
            former: Vec::new(),
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
        let manager = SessionManager::new(Arc::clone(&runtime));
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
        let manager = SessionManager::new(Arc::clone(&runtime));
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
        let manager = SessionManager::new(Arc::clone(&runtime));
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
        let manager = SessionManager::new(Arc::clone(&runtime));
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
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let ctx = SecurityContext::default();
        let session = make_session("stopped");
        let session_id = session.id;
        persist(&runtime, &ctx, &session).await;

        // A start under way holds the lock ...
        let start = manager.start_lock(session_id);
        let starting = start.lock().await;
        let stopping = tokio::spawn({
            let manager = Arc::clone(&manager);
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
        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new(), deleted: None };
        let (owner, other) = (user("owner"), user("other"));
        let (owner_id, other_id) = (ActorId::User(owner.id), ActorId::User(other.id));
        let mut session = make_session("with-users");
        session.participants = record_users(manager, &[&owner, &other]).await;
        session.security_attributes = SecurityAttributes { owner_id, contributors: Vec::new() };
        let ctx = SecurityContext::new(owner_id, session.id, Default::default());
        let session_loop = create(&manager, &ctx, session).await.expect("the session starts");
        (session_loop, ctx, other_id)
    }

    #[tokio::test]
    async fn a_deleted_agent_leaves_its_channels_keeps_its_name_and_is_restored_into_none() {
        use caelumora_model::agent::{Personality, PersonalityId};
        use caelumora_model::channel::Participant;
        use caelumora_model::scope::Scope;
        let runtime = test_runtime();
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let store = runtime.entity_store();
        let (system, ctx) = (SecurityContext::system(), SecurityContext::default());
        let personality = Personality {
            id: PersonalityId::new(), name: ScopedName::new("plain", Scope::GlobalScope).unwrap(), kind: Default::default(),
            roles: None, description: String::new(), introduction: None, components: Default::default(),
            security_attributes: Default::default(), reasoning_visibility: Default::default(), deleted: None,
        };
        store.create_personality(&system, &personality).await.unwrap();
        let helper = Agent::new(ScopedName::new("helper", Scope::GlobalScope).unwrap(), &personality, ActorId::None);
        store.create_agent(&system, &helper).await.unwrap();
        let actor = ActorId::Agent(helper.id);

        // In a running session's channel, and in a stopped one's.
        let mut running = make_session("running");
        running.participants = vec![Participant::Agent(helper.id)];
        let running = create(&manager, &ctx, running).await.unwrap();
        let talk = running.create_channel(&ctx, spec("talk", vec![actor])).await.unwrap();
        let mut stopped = make_session("stopped");
        stopped.participants = vec![Participant::Agent(helper.id)];
        store.create_session(&system, &stopped).await.unwrap();
        store.create_channel(&system, &Channel { participants: vec![Participant::Agent(helper.id)], ..main_channel(&stopped) }).await.unwrap();

        manager.delete_agent(&ctx, helper.id).await.expect("deleted");

        let left = |channel: &Channel| !channel.participants.iter().any(|p| p.actor_id() == actor) && channel.former.iter().any(|f| f.participant.actor_id() == actor);
        assert!(left(&talk.channel()), "it left the running channel");
        assert!(!talk.actor_ids().contains(&actor), "and its actor is gone");
        assert!(left(&store.get_channel(&system, stopped.channels[0]).await.unwrap()), "and the stopped one");
        assert!(store.get_agent(&system, helper.id).await.is_err(), "it is not there");
        let reached = manager.views(Viewer::new(ctx.clone())).channel_of(running.id(), talk.id()).await.unwrap().there().unwrap();
        let former = reached.member(actor).await.unwrap();
        assert!(former.former() && former.name() == Some("helper"), "what it said keeps its sender");

        manager.restore_agent(&ctx, helper.id).await.expect("restored");
        assert!(store.get_agent(&system, helper.id).await.is_ok());
        assert!(left(&talk.channel()), "but into no channel");
        assert!(manager.restore_agent(&ctx, helper.id).await.is_err(), "not deleted");
        assert!(manager.purge_agent(&ctx, helper.id).await.is_err(), "only what was deleted is purged");

        // Its name taken meanwhile: not restored.
        manager.delete_agent(&ctx, helper.id).await.unwrap();
        let again = Agent::new(ScopedName::new("helper", Scope::GlobalScope).unwrap(), &personality, ActorId::None);
        store.create_agent(&system, &again).await.unwrap();
        let taken = manager.restore_agent(&ctx, helper.id).await.expect_err("its name is taken");
        assert!(matches!(taken.downcast_ref::<StoreError>(), Some(StoreError::Conflict(_))), "{taken}");

        manager.purge_agent(&ctx, helper.id).await.expect("purged");
        assert!(store.get_deleted_agent(&system, helper.id).await.is_err(), "nothing left to restore");
        manager.stop_session(&system, running.id()).await.unwrap();
    }

    fn spec(name: &str, participants: Vec<caelumora_model::actor::ActorId>) -> ChannelSpec {
        ChannelSpec { name: Name::new(name).unwrap(), components: Default::default(), participants, classification: None, language: None }
    }

    #[tokio::test]
    async fn a_session_is_created_from_a_template_for_its_owner() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::approval::ApprovalStrategy;
        use caelumora_model::channel::Participant;
        use caelumora_model::template::{ChannelTemplate, ParticipantTemplate, SessionTemplate, SessionTemplateId};
        use caelumora_model::user::{User, UserId};

        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let store = runtime.entity_store();
        let owner = UserId::new();
        store.create_user(&SecurityContext::system(), &User { id: owner, username: "alice".to_string(), email: String::new(), roles: Vec::new(), deleted: None }).await.unwrap();
        let template = SessionTemplate {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            id: SessionTemplateId::new(),
            name: ScopedName::new("pairing", Default::default()).unwrap(),
            description: String::new(),
            participants: vec![
                ParticipantTemplate::User { name: "user".to_string() },
                ParticipantTemplate::Approver { name: "approval".to_string(), strategy: ApprovalStrategy::All },
            ],
            channels: vec![ChannelTemplate {
                summary: None, briefing: None,
                approval_rules: Vec::new(),
                name: Name::new("main").unwrap(),
                components: caelumora_model::channel::moderation_components(None, Some("broadcast_policy")),
                classification: Classification::CHANNEL_DEFAULT,
                participants: vec!["user".to_string(), "approval".to_string()],
            }],
            security_attributes: Default::default(),
            commands: Default::default(),
            privacy: Default::default(),
            deleted: None,
        };
        store.create_session_template(&SecurityContext::system(), &template).await.unwrap();
        let hers = SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let mut profile = caelumora_model::user::UserProfile::new(owner, None);
        profile.preferred_language = Some(Language::parse("de").unwrap());
        store.create_user_profile(&hers, &profile).await.unwrap();

        let session = manager.create_from_template(&hers, template.id, None).await.expect("created and started");
        assert_eq!(session.name(), "pairing", "named after the template unless named");
        assert_eq!(session.session().security_attributes.owner_id, ActorId::User(owner));
        let main = session.channel_by_name("main").expect("the template's channel runs");
        assert!(main.user_actor(owner).is_some(), "the owner is the template's \"user\"");
        assert_eq!(main.channel().language, profile.preferred_language, "held in the language the owner prefers");
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
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let mut rx = runtime.event_bus().subscribe_all();

        let channel = session.create_channel(&ctx, spec(" side ", vec![other])).await.expect("the channel is created");

        assert_eq!(channel.name(), "side", "the name is trimmed");
        let kinds: Vec<String> = channel.members().into_iter().map(|m| m.kind).collect();
        assert_eq!(kinds.iter().filter(|k| *k == "user").count(), 2, "the creator and the other user: {kinds:?}");
        assert!(kinds.iter().any(|k| k == "approver"), "an approver is added: {kinds:?}");
        assert!(session.channel_by_id(channel.id()).is_some());
        let names: Vec<String> = session.channels().iter().map(|c| c.name().to_string()).collect();
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
    async fn a_channel_is_held_in_the_language_its_creator_prefers_unless_asked_otherwise() {
        use caelumora_model::user::UserProfile;
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let language = |tag: &str| Some(Language::parse(tag).unwrap());

        let unsaid = session.create_channel(&ctx, spec("unsaid", vec![])).await.unwrap();
        assert_eq!(unsaid.channel().language, None, "no profile, no language");

        let owner = ctx.initiator_id.as_user_id().unwrap();
        let mut profile = UserProfile::new(owner, None);
        profile.preferred_language = language("de");
        runtime.entity_store().create_user_profile(&ctx, &profile).await.unwrap();
        let preferred = session.create_channel(&ctx, spec("preferred", vec![])).await.unwrap();
        assert_eq!(preferred.channel().language, language("de"), "the one the creator prefers");
        let asked = session.create_channel(&ctx, ChannelSpec { language: language("en"), ..spec("asked", vec![]) }).await.unwrap();
        assert_eq!(asked.channel().language, language("en"), "unless another is asked for");

        asked.update_language(&ctx, None).await.expect("the owner changes it");
        assert_eq!(asked.channel().language, None);
        let stored = runtime.entity_store().get_channel(&ctx, asked.id()).await.unwrap();
        assert_eq!(stored.language, None, "stored, not only in memory");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_that_cannot_be_is_refused_before_anything_is_stored() {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let stranger = caelumora_model::actor::ActorId::User(caelumora_model::user::UserId::new());

        let refusal = |result: Result<Arc<ChannelLoop>>| result.expect_err("refused").downcast::<SessionError>().expect("a SessionError");
        assert!(matches!(refusal(session.create_channel(&ctx, spec("main", vec![])).await), SessionError::DuplicateChannelName { .. }));
        assert!(matches!(refusal(session.create_channel(&ctx, spec("x", vec![stranger])).await), SessionError::NotAParticipant { .. }));
        let unknown = ChannelSpec { components: caelumora_model::channel::moderation_components(None, Some("nobody")), ..spec("x", vec![]) };
        assert!(matches!(refusal(session.create_channel(&ctx, unknown).await), SessionError::InvalidModerator(_)));
        let two = caelumora_model::component::ComponentConfig { component_nodes: vec![
            caelumora_model::component::ComponentDescriptor::new("one", crate::moderator_impl::broadcast_policy::BROADCAST_POLICY_KIND),
            caelumora_model::component::ComponentDescriptor::new("two", crate::moderator_impl::random_policy::RANDOM_POLICY_KIND),
        ] };
        assert!(matches!(refusal(session.create_channel(&ctx, ChannelSpec { components: two, ..spec("x", vec![]) }).await), SessionError::InvalidModerator(_)), "a channel has one policy");

        assert_eq!(session.channels().len(), 1);
        assert_eq!(runtime.entity_store().get_session(&ctx, session.id()).await.unwrap().channels.len(), 1);
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_deleted_channel_is_stopped_announced_and_kept_until_it_is_restored() {
        use caelumora_model::agent::{AgentChannelState, AgentId};
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let channel = session.create_channel(&ctx, spec("side", vec![])).await.unwrap();
        let id = channel.id();
        let agent = AgentId::new();
        runtime.state_store().put_agent_channel_state(agent, id, &mut AgentChannelState::default()).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let mut rx = runtime.event_bus().subscribe_all();

        session.delete_channel(&ctx, id).await.expect("the owner deletes it");

        assert!(session.channel_by_id(id).is_none());
        assert!(!runtime.entity_store().get_session(&ctx, session.id()).await.unwrap().channels.contains(&id));
        assert!(runtime.entity_store().get_channel(&ctx, id).await.is_err(), "not there");
        assert!(runtime.entity_store().get_deleted_channel(&ctx, id).await.is_ok(), "but kept, marked (issue 164)");
        assert!(runtime.state_store().get_channel_state(id).await.is_ok(), "with its state");
        let events = drain(&mut rx);
        let stopped = events.iter().position(|e| matches!(e, GlobalEvent::ChannelStopped(_, cid) if *cid == id));
        let deleted = events.iter().position(|e| matches!(e, GlobalEvent::ChannelDeleted(_, cid) if *cid == id));
        assert!(stopped.is_some() && deleted.is_some() && stopped < deleted, "stopped, then deleted: {events:?}");

        let missing = session.delete_channel(&ctx, id).await.expect_err("gone");
        assert!(matches!(missing.downcast_ref::<SessionError>(), Some(SessionError::ChannelNotFound { .. })));

        // Restored: listed and running again, from the state it kept.
        let taken = session.create_channel(&ctx, spec("side", vec![])).await.unwrap();
        let refused = session.restore_channel(&ctx, id).await.expect_err("its name is taken");
        assert!(matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::DuplicateChannelName { .. })), "{refused}");
        session.delete_channel(&ctx, taken.id()).await.unwrap();
        session.restore_channel(&ctx, id).await.expect("restored");
        assert!(session.channel_by_id(id).is_some());
        assert!(runtime.entity_store().get_session(&ctx, session.id()).await.unwrap().channels.contains(&id), "listed again");
        assert!(drain(&mut rx).iter().any(|e| matches!(e, GlobalEvent::ChannelRestored(_, cid) if *cid == id)), "and announced");
        assert!(runtime.state_store().get_channel_state(id).await.is_ok());
        let again = session.restore_channel(&ctx, id).await.expect_err("not deleted");
        assert!(matches!(again.downcast_ref::<SessionError>(), Some(SessionError::ChannelNotFound { .. })), "{again}");
        session.delete_channel(&ctx, id).await.unwrap();

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
        let session_loop = SessionLoop::new(session.clone(), vec![main_channel(&session)], Arc::clone(&runtime)).await.unwrap();
        let mut rx = runtime.event_bus().subscribe_all();

        // Queued to be started, but deleted before `run` gets to it.
        session_loop.delete_channel(&ctx, channel_id).await.expect("deleted");
        let running = tokio::spawn({ let session_loop = Arc::clone(&session_loop); async move { session_loop.run().await } });
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
        let store = Arc::new(MemoryStore::with_security_manager(Arc::clone(&policy)));
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, default_commands(), Default::default(), Vec::new(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let manager = SessionManager::new(runtime);

        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new(), deleted: None };
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
        let store = Arc::new(MemoryStore::with_security_manager(Arc::clone(&policy)));
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, default_commands(), Default::default(), Vec::new(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let manager = SessionManager::new(runtime);
        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new(), deleted: None };
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
    async fn only_the_owner_changes_a_sessions_privacy_and_everyone_in_it_sees_it() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::privacy::PrivacyConfig;
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::session::SessionUpdateEvent;
        use caelumora_model::user::{User, UserId};

        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(Arc::clone(&policy)));
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, default_commands(), Default::default(), Vec::new(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new(), deleted: None };
        let (alice, bob) = (user("alice"), user("bob"));
        let as_user = |id: UserId| SecurityContext::new(ActorId::User(id), Default::default(), Default::default());
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes { owner_id: ActorId::User(alice.id), contributors: vec![ActorId::User(bob.id)] };
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let session = create(&manager, &as_user(alice.id), session).await.unwrap();
        let mut events = runtime.event_bus().subscribe_all();

        let refused = session.update_privacy(&as_user(bob.id), PrivacyConfig::SharedAudience).await.expect_err("a participant may not");
        assert!(matches!(refused.downcast_ref::<StoreError>(), Some(StoreError::PermissionDenied { .. })), "{refused}");
        assert_eq!(session.privacy().policy, PrivacyConfig::Private, "unchanged");

        session.update_privacy(&as_user(alice.id), PrivacyConfig::SharedAudience).await.expect("the owner may");
        assert_eq!(session.privacy().policy, PrivacyConfig::SharedAudience);
        let stored = runtime.entity_store().get_session(&SecurityContext::system(), session.id()).await.unwrap();
        assert_eq!(stored.privacy, PrivacyConfig::SharedAudience, "stored");
        assert!(drain(&mut events).iter().any(|e| matches!(e, GlobalEvent::SessionUpdated(id, SessionUpdateEvent::PrivacyChanged(PrivacyConfig::SharedAudience)) if *id == session.id())),
                "announced");
        let for_bob = manager.views(Viewer::new(as_user(bob.id))).session(session.id()).await.unwrap();
        assert_eq!(for_bob.privacy().policy, PrivacyConfig::SharedAudience, "shown to everyone who takes part");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_reads_another_by_their_classification_not_by_who_takes_part() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::confidentiality::{Compartment, Compartments, ConfidentialityRules, Confidentiality};
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::{User, UserId};

        let compartments = Compartments::new([(Compartment::new("hr"), vec![])]).unwrap();
        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::with_rules(ConfidentialityRules::new(compartments)));
        let store = Arc::new(MemoryStore::with_security_manager(Arc::clone(&policy)));
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, default_commands(), Default::default(), Vec::new(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let manager = SessionManager::new(Arc::clone(&runtime));

        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new(), deleted: None };
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
            let (reader, source, security, context) = (Arc::clone(reader), Arc::clone(source), Arc::clone(security), alice_ctx.clone());
            async move {
                use caelumora_model::security::AccessQuestions;
                use caelumora_model::security::Place;
                let place = |channel: &ChannelLoop| Place { channel: channel.id(), classification: channel.classification(), session: channel.session_loop().map(|s| s.privacy()).unwrap_or_default() };
                security.may_channel_read(&context, &place(&reader), &place(&source)).await
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
    async fn a_reader_knows_the_privacy_of_its_session_and_looks_up_others() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::privacy::PrivacyConfig;
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::{User, UserId};

        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(Arc::clone(&policy)));
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, default_commands(), Default::default(), Vec::new(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let manager = SessionManager::new(Arc::clone(&runtime));
        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new(), deleted: None };
        let (alice, carol) = (user("alice"), user("carol"));
        let (alice_id, carol_id) = (ActorId::User(alice.id), ActorId::User(carol.id));
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default());

        let mut hers = make_session("hers");
        hers.security_attributes = SecurityAttributes::owned_by(alice_id);
        hers.participants = record_users(&manager, &[&alice]).await;
        hers.privacy = PrivacyConfig::SameOwner;
        let hers = create(&manager, &alice_ctx, hers).await.unwrap();
        let mut theirs = make_session("theirs");
        theirs.security_attributes = SecurityAttributes::owned_by(carol_id);
        persist(&runtime, &SecurityContext::new(carol_id, Default::default(), Default::default()), &theirs).await;

        let channel = Arc::clone(&hers.channels()[0]);
        let channel_runtime = ChannelRuntime::new(Arc::clone(&runtime), Arc::clone(&hers.commands), Arc::clone(&hers.session_controller));
        channel_runtime.attach(&channel);
        let views = channel_runtime.new_actor_runtime(alice_id, hers.id(), channel.id()).views(alice_ctx.clone());
        let session = views.session(channel.id(), channel.classification()).expect("attached to its session");

        let own = session.privacy();
        assert_eq!((own.policy, own.owner), (PrivacyConfig::SameOwner, alice_id));
        assert_eq!(own.people, [alice.id].into_iter().collect(), "its users and its owner");
        let other = session.views().privacy_of(theirs.id).await.unwrap().expect("stored, running or not");
        assert_eq!((other.policy, other.owner), (PrivacyConfig::Private, carol_id), "even a session the reader may not read");
        assert!(session.views().privacy_of(SessionId::new()).await.unwrap().is_none(), "one that is gone");
        manager.stop_session(&SecurityContext::system(), hers.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_released_message_is_read_where_its_channel_is_not() {
        use crate::channel::ChannelError;
        use crate::view::ViewError;
        use caelumora_model::actor::ActorId;
        use caelumora_model::confidentiality::{Compartment, Compartments, ConfidentialityRules, Confidentiality};
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::{User, UserId};

        let compartments = Compartments::new([(Compartment::new("hr"), vec![]), (Compartment::new("finance"), vec![])]).unwrap();
        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::with_rules(ConfidentialityRules::new(compartments)));
        let store = Arc::new(MemoryStore::with_security_manager(Arc::clone(&policy)));
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, default_commands(), Default::default(), Vec::new(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let alice = User { id: UserId::new(), username: "alice".to_string(), email: String::new(), roles: Vec::new(), deleted: None };
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

        // Read as an agent in another channel reads: through its turn's views.
        let reading_from = |channel: &Arc<ChannelLoop>| {
            let channel_runtime = ChannelRuntime::new(Arc::clone(&runtime), Arc::clone(&session.commands), Arc::clone(&session.session_controller));
            channel_runtime.attach(channel);
            let views = channel_runtime.new_actor_runtime(alice_id, session.id(), channel.id()).views(alice_ctx.clone());
            views.session(channel.id(), channel.classification()).expect("attached to its session")
        };
        let read = reading_from(&finance).channel_named("hr").await.expect("reachable for what was released").history().await.expect("what was released to finance");
        let texts: Vec<_> = read.messages.iter().map(|m| m.content.clone().unwrap_or_default()).collect();
        assert_eq!(texts, ["for finance too"], "only what was released");
        let nothing = reading_from(&team).channel_named("hr").await.expect("reachable, but").history().await;
        assert!(matches!(nothing, Err(ViewError::NoChannelNamed(_))), "nothing was released to a general channel");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_and_its_messages_are_reclassified_by_who_may() {
        use crate::channel::ChannelError;
        use caelumora_model::actor::ActorId;
        use caelumora_model::confidentiality::{Compartment, Compartments, ConfidentialityRules, Confidentiality};
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::{User, UserId};

        let compartments = Compartments::new([(Compartment::new("hr"), vec![]), (Compartment::new("finance"), vec![])]).unwrap();
        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::with_rules(ConfidentialityRules::new(compartments)));
        let store = Arc::new(MemoryStore::with_security_manager(Arc::clone(&policy)));
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, default_commands(), Default::default(), Vec::new(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let manager = SessionManager::new(Arc::clone(&runtime));
        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new(), deleted: None };
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

        // A change to what was said, through the actor of whoever makes it
        // (issue 239).
        let (as_alice, as_bob) = (hr.user_actor(alice.id).unwrap(), hr.user_actor(bob.id).unwrap());
        let released = classified(Confidentiality::Internal, &["hr", "finance"]);
        as_bob.reclassify_message(bobs, released.clone()).await.expect("the author releases their message");
        let refused = as_bob.reclassify_message(alices, released.clone()).await.expect_err("not another's");
        assert!(matches!(refused.downcast_ref::<ChannelError>(), Some(ChannelError::Forbidden { .. })), "{refused}");
        let missing = as_alice.reclassify_message(caelumora_model::message::MessageId::new(), released.clone()).await.expect_err("no such message");
        assert!(matches!(missing.downcast_ref::<ChannelError>(), Some(ChannelError::MessageNotFound { .. })), "{missing}");
        classified_as(&hr, bobs, &released).await;

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
        use caelumora_model::confidentiality::{Clearance, Compartment, Compartments, ConfidentialityRules, Confidentiality, Roles};
        use caelumora_model::store::EntityStore;
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::{User, UserId};

        let compartments = Compartments::new([(Compartment::new("hr"), vec![])]).unwrap();
        let roles = Roles::new([
            ("staff".to_string(), vec![Clearance { level: Confidentiality::Internal, compartment: None }]),
            ("hr".to_string(), vec![Clearance { level: Confidentiality::Confidential, compartment: Some(Compartment::new("hr")) }]),
        ]).unwrap();
        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::with_rules(ConfidentialityRules::new(compartments).with_roles(roles)));
        let store = Arc::new(MemoryStore::with_security_manager(Arc::clone(&policy)));
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, default_commands(), Default::default(), Vec::new(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let user = |name: &str, roles: &[&str]| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: roles.iter().map(|r| r.to_string()).collect(), deleted: None };
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
        let listed: Vec<ChannelId> = manager.views(Viewer::new(alice_ctx.clone())).session(session.id()).await.unwrap().channels().await.iter().map(|c| c.id()).collect();
        assert!(!listed.contains(&hr.id()) && listed.contains(&team.id()), "listed without it");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn an_agent_is_cleared_only_as_far_as_its_owner_and_its_personality_are() {
        use crate::channel::ChannelError;
        use caelumora_model::actor::ActorId;
        use caelumora_model::agent::{Agent, Personality, PersonalityId, PersonalityKind};
        use caelumora_model::confidentiality::{Clearance, Compartment, Compartments, ConfidentialityRules, Confidentiality, Roles};
        use caelumora_model::name::ScopedName;
        use caelumora_model::scope::Scope;
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::store::EntityStore;
        use caelumora_model::user::{User, UserId};

        let compartments = Compartments::new([(Compartment::new("hr"), vec![])]).unwrap();
        let roles = Roles::new([
            ("staff".to_string(), vec![Clearance { level: Confidentiality::Internal, compartment: None }]),
            ("hr".to_string(), vec![Clearance { level: Confidentiality::Confidential, compartment: Some(Compartment::new("hr")) }]),
        ]).unwrap();
        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::with_rules(ConfidentialityRules::new(compartments).with_roles(roles)));
        let store = Arc::new(MemoryStore::with_security_manager(Arc::clone(&policy)));
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, default_commands(), Default::default(), Vec::new(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let system = SecurityContext::system();
        let user = |name: &str, roles: &[&str]| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: roles.iter().map(|r| r.to_string()).collect(), deleted: None };
        let (alice, bob) = (user("alice", &["staff", "hr"]), user("bob", &["staff"]));
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(bob.id));
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default());
        // An agent of `owner` whose personality is of `kind` and names `roles`.
        let agent = |name: &'static str, owner: ActorId, kind: PersonalityKind, roles: Option<&'static [&'static str]>| {
            let store = Arc::clone(&store);
            let system = system.clone();
            async move {
                let personality = Personality {
                    id: PersonalityId::new(), name: ScopedName::new(name, Default::default()).unwrap(), kind,
                    roles: roles.map(|roles| roles.iter().map(|r| r.to_string()).collect()),
                    description: String::new(), introduction: None, components: Default::default(),
                    security_attributes: Default::default(), reasoning_visibility: Default::default(),
                    deleted: None,
                };
                store.create_personality(&system, &personality).await.unwrap();
                let agent = Agent::new(ScopedName::new(name, Scope::GlobalScope).unwrap(), &personality, owner);
                store.create_agent(&system, &agent).await.unwrap();
                ActorId::Agent(agent.id)
            }
        };
        // Stored first: their roles are read from the store.
        let users = record_users(&manager, &[&alice, &bob]).await;
        let hers = agent("hers", alice_id, PersonalityKind::Internal, None).await;
        let limited = agent("limited", alice_id, PersonalityKind::Internal, Some(&["staff"])).await;
        let bobs = agent("bobs", bob_id, PersonalityKind::Internal, None).await;
        let remote = agent("remote", alice_id, PersonalityKind::External, None).await;
        let cleared_remote = agent("cleared", alice_id, PersonalityKind::External, Some(&["staff"])).await;

        let hr = Classification::new(Confidentiality::Confidential, [Compartment::new("hr")]).unwrap();
        let internal = Classification::general(Confidentiality::Internal);
        let cleared = |participant: ActorId, classification: &Classification| {
            let runtime = Arc::clone(&runtime);
            let (alice_ctx, classification) = (alice_ctx.clone(), classification.clone());
            async move {
                // A channel of only `participant` - and its approver, who is
                // cleared for anything.
                let ActorId::Agent(id) = participant else { unreachable!("agents only") };
                let channel = caelumora_model::channel::Channel::build(SessionId::new(), alice_id, caelumora_model::name::Name::new("probe").unwrap(),
                    Default::default(), classification, None, vec![caelumora_model::channel::Participant::Agent(id)]);
                crate::channel::require_cleared(&runtime, &alice_ctx, &channel).await.is_ok()
            }
        };
        assert!(cleared(hers, &hr).await, "an agent that names no roles is cleared as its owner");
        assert!(!cleared(limited, &hr).await, "its personality limits it below its owner");
        assert!(cleared(limited, &internal).await, "within its limit");
        assert!(!cleared(bobs, &hr).await, "its owner is not cleared");
        assert!(!cleared(remote, &internal).await, "an external agent that names no roles sees only what is public");
        assert!(cleared(remote, &Classification::PUBLIC).await);
        assert!(cleared(cleared_remote, &internal).await, "one that names its roles, within them");
        assert!(!cleared(cleared_remote, &hr).await);

        // The same question when a channel is created and when one is joined.
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes { owner_id: alice_id, contributors: vec![bob_id] };
        session.participants = users;
        session.participants.extend([hers, limited].map(|a| match a { ActorId::Agent(id) => caelumora_model::channel::Participant::Agent(id), _ => unreachable!() }));
        let session = create(&manager, &alice_ctx, session).await.unwrap();
        let with = |name: &str, participants| ChannelSpec { classification: Some(hr.clone()), ..spec(name, participants) };
        let refused = session.create_channel(&alice_ctx, with("hr", vec![limited])).await.expect_err("limited below HR");
        assert!(matches!(refused.downcast_ref::<ChannelError>(), Some(ChannelError::NotCleared { participant, .. }) if *participant == limited), "{refused}");
        let channel = session.create_channel(&alice_ctx, with("hr", vec![hers])).await.expect("cleared as alice");
        let joining = channel.add_participant(&alice_ctx, limited, caelumora_model::channel::HistoryAccess::All).await.expect_err("nor may it join");
        assert!(matches!(joining.downcast_ref::<ChannelError>(), Some(ChannelError::NotCleared { .. })), "{joining}");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    /// A model whose answer never comes.
    #[derive(Debug)]
    struct Silent;

    #[async_trait::async_trait]
    impl crate::llm::LLMClient for Silent {
        async fn chat_completion(&self, _: &[caelumora_model::message::Message], _: &[caelumora_model::tool::Tool])
            -> futures::stream::BoxStream<'static, crate::llm::LLMResult<caelumora_model::message::MessageEvent>>
        {
            Box::pin(futures::stream::pending())
        }

        fn profile(&self) -> caelumora_model::context::ModelProfile {
            Default::default()
        }
    }

    #[tokio::test]
    async fn a_member_s_answer_under_way_is_in_the_snapshot_of_its_channel() {
        use caelumora_model::actor::{ActorEvent, ActorId};
        use caelumora_model::agent::{Agent, Personality, PersonalityId, PersonalityKind};
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::scope::Scope;
        use caelumora_model::snapshot::Detail;
        use caelumora_model::store::EntityStore;
        use caelumora_model::user::{User, UserId};

        let store = Arc::new(MemoryStore::new());
        let mut registry = crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store));
        registry.register(crate::component::ComponentKind::new("silent").provides::<dyn crate::llm::LLMClient>(), |_descriptor, _context| {
            let mut facets = crate::component::ComponentFacets::empty();
            facets.insert::<dyn crate::llm::LLMClient>(Arc::new(Silent));
            Ok(facets)
        });
        let policy = Arc::new(DummySecurityManager::new());
        let secrets = Arc::new(SecretManager::new(vec![], Arc::<DummySecurityManager>::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, default_commands(), Default::default(), Vec::new(), registry).unwrap());
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let system = SecurityContext::system();

        let alice = User { id: UserId::new(), username: "alice".to_string(), email: String::new(), roles: Vec::new(), deleted: None };
        let participants = record_users(&manager, &[&alice]).await;
        let personality = Personality {
            id: PersonalityId::new(), name: ScopedName::new("thinker", Default::default()).unwrap(), kind: PersonalityKind::Internal,
            roles: None, description: String::new(), introduction: None,
            components: ComponentConfig { component_nodes: vec![ComponentDescriptor::new("model", "silent")] },
            security_attributes: Default::default(), reasoning_visibility: Default::default(),
            deleted: None,
        };
        store.create_personality(&system, &personality).await.unwrap();
        let thinker = Agent::new(ScopedName::new("thinker", Scope::GlobalScope).unwrap(), &personality, ActorId::User(alice.id));
        store.create_agent(&system, &thinker).await.unwrap();
        let agent = ActorId::Agent(thinker.id);

        let alice_ctx = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let mut session = make_session("thinking");
        session.participants = participants;
        session.participants.push(caelumora_model::session::Participant::Agent(thinker.id));
        let session = create(&manager, &alice_ctx, session).await.unwrap();
        let channel = session.create_channel(&alice_ctx, spec("talk", vec![ActorId::User(alice.id), agent])).await.unwrap();
        let mut events = channel.user_actor(alice.id).unwrap().watch().events;
        let shown = |events: &mut tokio::sync::broadcast::Receiver<caelumora_model::actor::Envelope>, wanted: &'static str| {
            let mut events = events.resubscribe();
            async move {
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    loop {
                        let envelope = events.recv().await.expect("the channel is running");
                        if envelope.context.sender_id == agent && envelope.event.kind() == wanted { break envelope.event }
                    }
                }).await.unwrap_or_else(|_| panic!("the agent sends {wanted}"))
            }
        };
        let member = || async {
            let view = manager.views(Viewer::new(alice_ctx.clone())).session(session.id()).await.unwrap().channel(channel.id()).await.unwrap();
            view.snapshot(Detail::MEMBERS).await.members.into_iter().find(|m| m.id == agent).expect("a member")
        };
        assert_eq!(member().await.turn_under_way, None, "nothing under way yet");

        let started = shown(&mut events, "ResponseStarted");
        channel.user_actor(alice.id).unwrap().send_message("hello".to_string(), None).await.unwrap();
        let ActorEvent::ResponseStarted { turn_id } = started.await else { unreachable!() };
        assert_eq!(member().await.turn_under_way, Some(turn_id), "the turn its start announced");

        let ended = shown(&mut events, "FullResponseGenerated");
        channel.cancel_actor(&alice_ctx, agent).await.unwrap();
        ended.await;
        assert_eq!(member().await.turn_under_way, None, "over once it ended");
        manager.stop_session(&system, session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn an_external_agent_is_told_who_is_in_its_channel_and_who_joined_since() {
        use crate::external::ExternalEvent;
        use crate::external::testing::ScriptedExternalAgent;
        use caelumora_model::actor::{ActorEvent, ActorId};
        use caelumora_model::agent::{Agent, Personality, PersonalityId, PersonalityKind};
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::scope::Scope;
        use caelumora_model::store::EntityStore;
        use caelumora_model::user::{User, UserId};

        let answer = |text: &str| vec![Ok(ExternalEvent::Message(text.to_string())), Ok(ExternalEvent::Completed)];
        let remote = Arc::new(ScriptedExternalAgent::answering(vec![answer("Hello."), answer("Hi, bob.")]));
        let store = Arc::new(MemoryStore::new());
        let mut registry = crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store));
        registry.register(
            crate::component::ComponentKind::new("test_remote").provides::<dyn crate::external::ExternalAgent>(),
            { let remote = Arc::clone(&remote); move |_descriptor, _context| {
                let mut facets = crate::component::ComponentFacets::empty();
                facets.insert::<dyn crate::external::ExternalAgent>(Arc::<ScriptedExternalAgent>::clone(&remote));
                Ok(facets)
            } },
        );
        let policy = Arc::new(DummySecurityManager::new());
        let secrets = Arc::new(SecretManager::new(vec![], Arc::<DummySecurityManager>::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, default_commands(), Default::default(), Vec::new(), registry).unwrap());
        let manager = SessionManager::new(Arc::clone(&runtime));
        let system = SecurityContext::system();

        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new(), deleted: None };
        let (alice, bob) = (user("alice"), user("bob"));
        let participants = record_users(&manager, &[&alice, &bob]).await;
        let mut members = ComponentDescriptor::new("members", "channel_members_prompt_provider");
        members.params = serde_json::json!({ "changes_only": true });
        let personality = Personality {
            id: PersonalityId::new(), name: ScopedName::new("remote", Default::default()).unwrap(), kind: PersonalityKind::External,
            roles: None, description: String::new(), introduction: None,
            components: ComponentConfig { component_nodes: vec![ComponentDescriptor::new("remote", "test_remote"), members] },
            security_attributes: Default::default(), reasoning_visibility: Default::default(),
            deleted: None,
        };
        store.create_personality(&system, &personality).await.unwrap();
        let hermes = Agent::new(ScopedName::new("hermes", Scope::GlobalScope).unwrap(), &personality, ActorId::User(alice.id));
        store.create_agent(&system, &hermes).await.unwrap();

        let alice_ctx = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let mut session = make_session("roster");
        session.participants = participants;
        session.participants.push(caelumora_model::session::Participant::Agent(hermes.id));
        let session = create(&manager, &alice_ctx, session).await.unwrap();
        let channel = session.create_channel(&alice_ctx, spec("talk", vec![ActorId::User(alice.id), ActorId::Agent(hermes.id)])).await.unwrap();
        let mut events = channel.user_actor(alice.id).unwrap().watch().events;
        let answered = |events: &mut tokio::sync::broadcast::Receiver<caelumora_model::actor::Envelope>| {
            let mut events = events.resubscribe();
            async move {
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    loop {
                        let envelope = events.recv().await.expect("the channel is running");
                        if envelope.context.sender_id == ActorId::Agent(hermes.id) && matches!(envelope.event, ActorEvent::FullResponseGenerated(_)) { break }
                    }
                }).await.expect("the agent answers")
            }
        };

        let waiting = answered(&mut events);
        channel.user_actor(alice.id).unwrap().send_message("hello".to_string(), None).await.unwrap();
        waiting.await;
        let waiting = answered(&mut events);
        channel.add_participant(&alice_ctx, ActorId::User(bob.id), caelumora_model::channel::HistoryAccess::All).await.unwrap();
        channel.user_actor(alice.id).unwrap().send_message("bob is here".to_string(), None).await.unwrap();
        waiting.await;

        let sent = remote.sent.lock().unwrap().clone();
        let first = &sent[0].1.prompts;
        assert_eq!(first.len(), 1, "{first:?}");
        assert!(first[0].starts_with("[Who is in this channel]\nYou are hermes (@hermes).\n"), "{}", first[0]);
        assert!(first[0].contains("- alice (@alice), a person.") && !first[0].contains("bob"), "{}", first[0]);
        let second = &sent[1].1.prompts;
        assert_eq!(second.len(), 1, "{second:?}");
        assert!(second[0].contains("- bob (@bob), a person.\n"), "the whole roster again: {}", second[0]);
        assert!(second[0].ends_with("Since you were last told who is here:\n- Joined: bob (@bob), a person."), "{}", second[0]);
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_deleted_session_keeps_its_state_until_it_is_purged_and_can_be_restored() {
        use caelumora_model::store::StateStore;
        let entities = Arc::new(MemoryStore::new());
        let states = Arc::new(MemoryStore::new());
        let secret_manager = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&states), Arc::<MemoryStore>::clone(&entities), secret_manager, Arc::new(DummySecurityManager::new()), default_commands(), Default::default(), Vec::new(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&entities))).unwrap());
        let manager = SessionManager::new(runtime);
        let ctx = SecurityContext::default();
        let session = make_session("two-stores");
        let (id, channel_id) = (session.id, session.channels[0]);
        create(&manager, &ctx, session).await.unwrap();
        assert!(states.get_channel_state(channel_id).await.is_ok(), "the channel keeps its state in the state store");

        manager.delete_session(&ctx, id).await.unwrap();

        use caelumora_model::store::EntityStore;
        assert!(entities.get_session(&ctx, id).await.is_err(), "not there");
        assert!(states.get_channel_state(channel_id).await.is_ok(), "but kept, with its channels' state (issue 164)");
        manager.restore_session(&ctx, id).await.expect("restored");
        manager.open_session(&ctx, id).await.expect("and opened as any stored session");
        assert!(manager.restore_session(&ctx, id).await.is_err(), "not deleted");
        assert!(manager.purge_session(&ctx, id).await.is_err(), "only what was deleted is purged");

        // A channel deleted from it before goes with it, too.
        let side = manager.open_session(&ctx, id).await.unwrap().create_channel(&ctx, spec("side", vec![])).await.unwrap().id();
        manager.open_session(&ctx, id).await.unwrap().delete_channel(&ctx, side).await.unwrap();
        assert!(states.get_channel_state(side).await.is_ok(), "kept with the deleted channel");
        manager.delete_session(&ctx, id).await.unwrap();
        manager.purge_session(&ctx, id).await.expect("purged");
        assert!(entities.get_deleted_session(&ctx, id).await.is_err(), "nothing left to restore");
        assert!(states.get_channel_state(channel_id).await.is_err(), "deleted from the state store, not only from the entity store");
        assert!(entities.get_channel(&ctx, channel_id).await.is_err(), "its channels go with it (issue 70)");
        assert!(entities.get_deleted_channel(&ctx, side).await.is_err(), "also those deleted from it before");
        assert!(states.get_channel_state(side).await.is_err(), "with their state");
    }

    #[tokio::test]
    async fn renaming_a_channel_is_stored_and_only_the_owner_may() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::store::EntityStore;
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::UserId;

        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(Arc::clone(&policy)));
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, default_commands(), Default::default(), Vec::new(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let manager = SessionManager::new(runtime);
        let (alice, bob) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        let as_user = |actor| SecurityContext::new(actor, Default::default(), Default::default());
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes { owner_id: alice, contributors: vec![bob] };
        let running = create(&manager, &as_user(alice), session).await.unwrap();
        let main = running.channel_by_name("main").unwrap();

        let refused = main.update_name(&as_user(bob), Name::new("bobs").unwrap()).await.expect_err("a contributor may not");
        assert!(crate::error::is_forbidden(&refused), "{refused}");
        assert_eq!(main.name(), "main", "unchanged");

        main.update_name(&as_user(alice), Name::new("lobby").unwrap()).await.expect("the owner may");
        assert_eq!(main.name(), "lobby");
        assert_eq!(store.get_channel(&as_user(alice), main.id()).await.unwrap().name, "lobby", "stored, not only in memory");
        manager.stop_session(&SecurityContext::system(), running.id()).await.unwrap();
    }

    /// Waits until `channel`'s history holds `count` messages.
    /// Waits until the message `id` of `channel` is classified `classification`.
    async fn classified_as(channel: &ChannelLoop, id: caelumora_model::message::MessageId, classification: &Classification) {
        for _ in 0..100 {
            if channel.message(id).await.is_some_and(|m| m.classification == *classification) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("message {id} was never classified {classification}");
    }

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
        let manager = SessionManager::new(Arc::clone(&runtime));
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
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let owner = ctx.initiator_id.as_user_id().unwrap();
        let within = |ended: tokio_util::sync::CancellationToken| async move {
            tokio::time::timeout(std::time::Duration::from_secs(5), ended.cancelled_owned()).await
        };

        // Leaving ends it.
        let channel = session.create_channel(&ctx, spec("side", vec![other])).await.unwrap();
        let theirs = channel.user_actor(other.as_user_id().unwrap()).unwrap().watch();
        let mine = channel.user_actor(owner).unwrap().watch();
        channel.remove_participant(&ctx, other).await.expect("removed");
        within(theirs.ended).await.expect("the watch of whoever left ends");
        assert!(!mine.ended.is_cancelled(), "everybody else's goes on");

        // So does deleting the channel, for everybody.
        session.delete_channel(&ctx, channel.id()).await.expect("deleted");
        within(mine.ended).await.expect("a deleted channel is watched by nobody");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_removed_participant_is_kept_as_a_former_one_and_announced_but_the_approver_stays() {
        use crate::channel::ChannelError;
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let channel = session.create_channel(&ctx, spec("side", vec![other])).await.unwrap();
        let mut rx = runtime.event_bus().subscribe_all();

        channel.remove_participant(&ctx, other).await.expect("removed");

        assert!(!channel.members().iter().any(|m| m.id == other), "no longer a member");
        let stored = runtime.entity_store().get_channel(&ctx, channel.id()).await.unwrap();
        assert!(!stored.participants.iter().any(|p| p.actor_id() == other), "stored, not only in memory");
        let former = stored.former.iter().find(|f| f.participant.actor_id() == other).expect("kept as a former participant (issue 164)");
        assert_eq!(former.left.by, ctx.caller_id, "with who removed it");
        let events = drain(&mut rx);
        assert!(events.iter().any(|e| matches!(e, GlobalEvent::UserChannelLeft(_, cid, _) if *cid == channel.id())),
                "the leave is announced: {events:?}");

        let refusal = |error: anyhow::Error| error.downcast::<ChannelError>().expect("a ChannelError");
        let gone = channel.remove_participant(&ctx, other).await.expect_err("not in it any more");
        assert!(matches!(refusal(gone), ChannelError::NotAParticipant { .. }));
        let approver = channel.members().into_iter().find(|m| m.kind == "approver").expect("an approver").id;
        let stays = channel.remove_participant(&ctx, approver).await.expect_err("the approver stays");
        assert!(matches!(refusal(stays), ChannelError::ApproverStays { .. }));

        // Added again at once, once its old actor has ended (issue 171).
        channel.add_participant(&ctx, other, caelumora_model::channel::HistoryAccess::All).await.expect("added again");
        assert!(channel.user_actor(other.as_user_id().unwrap()).is_some(), "with a new actor");
        let stored = runtime.entity_store().get_channel(&ctx, channel.id()).await.unwrap();
        assert!(stored.former.is_empty(), "and no longer a former participant");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn only_who_may_change_a_channel_adds_or_removes_its_participants() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::channel::HistoryAccess;
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::{User, UserId};

        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(Arc::clone(&policy)));
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, default_commands(), Default::default(), Vec::new(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let manager = SessionManager::new(runtime);
        let user = |name: &str| User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new(), deleted: None };
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
        let store = Arc::new(MemoryStore::with_security_manager(Arc::clone(&policy)));
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, default_commands(), Default::default(), Vec::new(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
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
