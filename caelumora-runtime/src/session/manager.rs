//! `SessionManager`: starting, stopping, deleting, restoring and purging sessions.

use super::{
    load_own, make_own, purge_channel_document, require_attachable, require_classification,
    SessionError, SessionLoop,
};
use crate::agent::AgentManager;
use crate::attachment::AttachmentManager;
use crate::event::EventBus;
use crate::group::{keep_participants, GroupManager};
use crate::runtime::RuntimeManager;
use crate::scoped::{purge_for_good, ScopeNotEmpty, ScopedEntities};
use crate::template::{
    resolve_participants, resolve_workspace, take_in_participants, TemplateParticipants,
};
use crate::view::{Viewer, Views};
use crate::workspace::reaper::CommandReaper;
use anyhow::{Error, Result};
use caelumora_model::actor::ActorId;
#[cfg(any(test, feature = "test-support"))]
use caelumora_model::channel::Participant;
use caelumora_model::channel::{Channel, ChannelField};
use caelumora_model::event::{Announcing, GlobalEvent};
use caelumora_model::group::{Group, GroupId};
use caelumora_model::project::Project;
use caelumora_model::removal::Removal;
use caelumora_model::scope::{Scope, ScopeField};
use caelumora_model::security::{
    AccessPolicy, AccessQuestions, ActionType, Enforcement, SecurityAttributes, SecurityContext,
    Subject, JOIN,
};
use caelumora_model::session;
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::store::query::{Filter, Query};
use caelumora_model::store::{
    Deleted, EntityStores, StateStore, StoreError, StoreResult, StoreResultExt, Stored,
};
use caelumora_model::template::{
    ScopeTemplate, SessionTemplate, SessionTemplateId, WorkspaceTemplate,
};
use caelumora_model::user::UserId;
use caelumora_model::value::language::Language;
use caelumora_model::value::name::Name;
use caelumora_model::value::time_zone::TimeZone;
use caelumora_model::workspace::WorkspaceChoice;
use dashmap::DashMap;
use futures::future::join_all;
use std::collections::HashSet;
use std::iter;
#[cfg(any(test, feature = "test-support"))]
use std::mem;
use std::ops::AsyncFnOnce;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time;
use tracing::{info, warn};

/// How long `SessionManager::shutdown` takes at most (issue 297): within
/// the ten seconds Docker leaves between SIGTERM and SIGKILL, with the
/// actors' `STOP_GRACE` and the time a command's kill waits.
const SHUTDOWN_BOUND: Duration = Duration::from_secs(8);

pub struct SessionManager {
    sessions: DashMap<SessionId, Arc<SessionLoop>>,
    handles: DashMap<SessionId, JoinHandle<()>>,
    /// One lock per session id, held while that session is being started,
    /// so that a session starts only once however many requests ask for it
    /// at the same time (issue 62): two loops of one session would run two
    /// actors per agent and channel, which overwrite each other's state.
    starts: DashMap<SessionId, Arc<Mutex<()>>>,
    /// Set once the sessions are shut down (`shutdown`): none is started
    /// any more.
    shutting_down: AtomicBool,
    /// What builds the loops of the sessions it starts.
    pub(super) runtime_manager: Arc<RuntimeManager>,
    /// The services it works with, taken from `runtime_manager` once (issue
    /// 229).
    access_policy: Arc<dyn AccessPolicy>,
    stores: EntityStores,
    state_store: Arc<dyn StateStore>,
    events: Arc<EventBus>,
    /// What ends the commands of a stopped session's channels (issue 254).
    command_reaper: Arc<CommandReaper>,
    /// What makes the agents a template names.
    agents: Arc<AgentManager>,
    /// What goes with a session deleted, restored or purged (issue 178).
    scoped: Arc<ScopedEntities>,
    /// What deletes and restores the groups of its participants with it
    /// (issue 348).
    groups: Arc<GroupManager>,
    /// What removes the attachments of the channels of a session purged (issue 282).
    attachments: Arc<AttachmentManager>,
}

/// A session as [`SessionManager::find`] found it for a context that may
/// read it: running or not, each asked about alike.
#[expect(clippy::large_enum_variant, reason = "a value returned once and taken apart at once")]
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

    /// The session's attributes as they are now, without the rest of it.
    fn attributes(&self) -> SecurityAttributes {
        match self {
            Found::Running(session_loop) => session_loop.session().security_attributes.clone(),
            Found::Stored(session) => session.security_attributes.clone(),
        }
    }
}

impl SessionManager {
    pub fn new(runtime_manager: Arc<RuntimeManager>) -> Self {
        Self {
            sessions: DashMap::new(),
            handles: DashMap::new(),
            starts: DashMap::new(),
            shutting_down: AtomicBool::new(false),
            access_policy: Arc::clone(runtime_manager.access_policy()),
            stores: runtime_manager.entity_stores().clone(),
            state_store: Arc::clone(runtime_manager.state_store()),
            events: Arc::clone(runtime_manager.event_bus()),
            command_reaper: Arc::clone(runtime_manager.command_reaper()),
            agents: Arc::clone(runtime_manager.agent_manager()),
            scoped: Arc::clone(runtime_manager.scoped_entities()),
            groups: Arc::clone(runtime_manager.group_manager()),
            attachments: Arc::clone(runtime_manager.attachment_manager()),
            runtime_manager,
        }
    }

    /// The lock held while session `id` is started.
    fn start_lock(&self, id: SessionId) -> Arc<Mutex<()>> {
        self.starts.entry(id).or_default().clone()
    }

    /// Build the runtime loop for an (already-persisted) session, register it, spawn its
    /// run task, and announce that the session has started running. Called
    /// with the session's start lock held.
    async fn spawn_session(
        &self,
        session: Session,
        channels: Vec<Channel>,
    ) -> Result<Arc<SessionLoop>> {
        self.refuse_while_shutting_down()?;
        let id = session.id;
        let attributes = session.security_attributes.clone();
        let session_loop =
            SessionLoop::new(session, channels, Arc::clone(&self.runtime_manager)).await?;
        self.sessions.insert(id, Arc::clone(&session_loop));

        let loop_for_task = Arc::clone(&session_loop);
        let handle = tokio::spawn(async move {
            if let Err(e) = loop_for_task.run().await {
                tracing::error!("Session loop for {} failed: {}", id, e);
            }
        });
        self.handles.insert(id, handle);

        self.events.publish(GlobalEvent::custom::<Session>(&id, session::STARTED, &()), attributes);
        Ok(session_loop)
    }

    /// Session `id` as `context` may read it: its loop if it runs, its
    /// document from the store if not. A running session is asked about
    /// like a stored one - `GetSession` with its attributes - so that it
    /// being in memory is no way around the policy (issue 11).
    ///
    /// # Errors
    /// `SessionError::NotFound` if there is no such session or `context`
    /// may not read it, alike whether it runs.
    pub(crate) async fn find(&self, context: &SecurityContext, id: SessionId) -> Result<Found> {
        let running = self.sessions.get(&id).map(|entry| entry.clone());
        match running {
            Some(session_loop) => {
                let attributes = session_loop.session().security_attributes.clone();
                self.access_policy
                    .require(context, ActionType::get(Session::KIND, &attributes), |_| {
                        SessionError::NotFound(id)
                    })
                    .await?;
                Ok(Found::Running(session_loop))
            }
            None => Ok(Found::Stored(
                self.stores.sessions.get(context, id).await.map_err(store_error(id))?,
            )),
        }
    }

    /// Creates a session from the template `template_id` on behalf of `context`, whose user owns it
    /// and is the template's "user", names it `name` - the template's name if `None` -, holds it in
    /// `language` and `time_zone` - the template's if `None` (issues 424, 425) - and starts it
    /// (issue 88). Its workspace is `workspace` where one is chosen (issue 266) - none, one of the
    /// user's own, or one made for it alone -, else the template's. One of the user's own is
    /// attached as the session is made. Once the session is made, one made for it alone or the
    /// template's is found by name in its scope, or made, and attached (issue 344), the template's
    /// agents are found or made alike and taken into it and their channels, and one of the user's
    /// own is shared with its participants as far as the choice says (issue 23), as `context`; if
    /// any of it fails, the session's creation is rolled back, with what was made in its scope or
    /// for it.
    ///
    /// # Errors
    /// `SessionError::OwnerNotAUser` if `context` acts for no user; a `StoreError` if the template,
    /// a user or a personality it names does not exist or may not be read; a `ValueError` if `name`
    /// is no name, `language` no language tag or `time_zone` no time zone; a `TemplateError` if the
    /// template cannot be made into a session; what attaching an existing workspace, making one or
    /// sharing one reports; `SessionError::ChannelUnavailable` if the components of one of its
    /// channels cannot be read in the owner's name (issue 463); whatever creating the session
    /// reports.
    pub async fn create_from_template(
        &self,
        context: &SecurityContext,
        template_id: SessionTemplateId,
        name: Option<&str>,
        language: Option<&str>,
        time_zone: Option<&str>,
        workspace: Option<WorkspaceChoice>,
    ) -> Result<Arc<SessionLoop>> {
        self.refuse_while_shutting_down()?;
        let owner = context
            .principal_id
            .as_user_id()
            .ok_or(SessionError::OwnerNotAUser(context.principal_id))?;
        // Before anything is found or made for it.
        let name = Name::optional(name)?;
        let language = Language::optional(language)?;
        let time_zone = TimeZone::optional(time_zone)?;
        let template =
            self.stores.session_templates.get(context, template_id).await.map_err(|e| match e {
                StoreError::NotFound { .. } => SessionError::TemplateNotFound(template_id).into(),
                e => Error::new(e),
            })?;
        let id = SessionId::new();
        // Its own verb before anything of others is asked or anything is
        // found or made for it (issue 312): who may not create the session
        // learns nothing of the agents, the workspace or anyone's clearance.
        self.access_policy
            .require(
                context,
                ActionType::create(
                    Session::KIND,
                    &Session::attributes_of(id, owner, &template.access),
                ),
                StoreError::PermissionDenied,
            )
            .await?;
        // One of the user's chosen is asked about before anything is made.
        let existing = match &workspace {
            Some(WorkspaceChoice::Existing { id: chosen, share }) => {
                require_attachable(
                    self.runtime_manager.workspace_manager(),
                    self.access_policy.as_ref(),
                    context,
                    *chosen,
                    id,
                )
                .await?;
                Some((*chosen, *share))
            }
            _ => None,
        };
        let (users, agents) =
            resolve_participants(&self.stores, context, &template, id, owner).await?;
        let name = name.unwrap_or_else(|| template.name.name.clone());
        // What is to be found or made: the template's, unless one was chosen.
        let wanted = match workspace {
            None => template.workspace.clone(),
            Some(WorkspaceChoice::New { storage, classification }) => Some(WorkspaceTemplate {
                name: name.clone(),
                scope: ScopeTemplate::SessionScope,
                storage,
                classification,
                description: None,
            }),
            Some(WorkspaceChoice::None | WorkspaceChoice::Existing { .. }) => None,
        };
        let (mut session, own, channels) =
            template.build(id, owner, name, language, time_zone, users.clone())?;
        // One chosen is attached as the session is made; one to be found or
        // made once the session is, so that one of its scope is made where
        // `context` may put it (issue 344).
        session.workspace = existing.map(|(chosen, _)| chosen);
        let session_loop = self.establish(context, session, &own, channels).await?;
        // A channel whose components the owner cannot read is left out as a
        // session opens (issue 463), but making it is the owner's own request,
        // refused as `create_channel` refuses it.
        let completed = async {
            session_loop.require_running()?;
            self.complete(
                context,
                owner,
                &template,
                &session_loop,
                wanted.as_ref(),
                (users, agents),
            )
            .await?;
            // Shared once the session it is shared with exists (issue 23).
            if let Some((chosen, Some(grant))) = existing {
                let controller =
                    self.runtime_manager.controller_manager().workspace_controller(chosen).await?;
                controller.share(context, Subject::Group(GroupId::of_session(id)), grant).await?;
            }
            Ok(())
        }
        .await;
        if let Err(e) = completed {
            self.roll_back_creation(id).await;
            return Err(e);
        }
        Ok(session_loop)
    }

    /// Completes `session` of `owner`, just made from `template` on behalf
    /// of `context`: the workspace `wanted` found or made and attached, then
    /// the template's agents found or made and taken in (issues 227, 344),
    /// and its users and agents - `participants`, as `resolve_participants`
    /// gives them - taken into the channels that name them as their
    /// founders (issue 420).
    /// A workspace made for it that is not in its scope - which goes with
    /// the session as its creation is rolled back - is deleted here if that
    /// fails.
    ///
    /// # Errors
    /// What finding or making the workspace or the agents reports, or
    /// taking them in.
    async fn complete(
        &self,
        context: &SecurityContext,
        owner: UserId,
        template: &SessionTemplate,
        session: &SessionLoop,
        wanted: Option<&WorkspaceTemplate>,
        participants: TemplateParticipants,
    ) -> Result<()> {
        let workspaces = self.runtime_manager.workspace_manager();
        let resolved = match wanted {
            Some(wanted) => {
                Some(resolve_workspace(workspaces, context, wanted, session.id(), owner).await?)
            }
            None => None,
        };
        let made = resolved.and_then(|(workspace, made)| made.then_some(workspace));
        let completed = async {
            if let Some((workspace, _)) = resolved {
                session.attach_workspace(context, Some(workspace)).await?;
            }
            let (users, agents) = participants;
            take_in_participants(
                self.stores.agents.as_ref(),
                &self.agents,
                context,
                template,
                session,
                users,
                agents,
            )
            .await
        }
        .await;
        if let (Err(_), Some(made)) = (&completed, made) {
            let system = SecurityContext::system();
            if let Err(cleanup) = async {
                workspaces.delete(&system, made).await?;
                workspaces.purge(&system, made).await
            }
            .await
            {
                warn!(workspace = %made, "The workspace made for a session that could not be created stays: {cleanup}");
            }
        }
        completed
    }

    /// Rolls back creating the session `id`, just made and not to be kept -
    /// not complete, or not put into the project it was made for (issue
    /// 467) -, in the runtime's name - making it was permitted: deleted and
    /// purged, with what was made in its scope. What cannot be is logged.
    pub async fn roll_back_creation(&self, id: SessionId) {
        let system = SecurityContext::system();
        if let Err(undo) = async {
            self.delete(&system, id).await?;
            self.purge(&system, id).await
        }
        .await
        {
            warn!(session = %id, "The session that could not be completed stays: {undo}");
        }
    }

    /// Creates a brand-new session with its `channels`, as a template would: asks to create it
    /// (`CreateSession`) before anything else. Its results go to a project of its own, made with
    /// it, whatever project it names (issue 467). The participants the channels name join them as
    /// their founders once the session runs, as a template's do (issue 420).
    ///
    /// # Errors
    /// `SessionError::OwnerNotAUser` if the session is not a user's; otherwise as `establish`.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn create(
        &self,
        context: &SecurityContext,
        mut session: Session,
        mut channels: Vec<Channel>,
    ) -> Result<Arc<SessionLoop>> {
        self.refuse_while_shutting_down()?;
        self.access_policy
            .require(
                context,
                ActionType::create(Session::KIND, &session.security_attributes),
                StoreError::PermissionDenied,
            )
            .await?;
        let own = session
            .new_own_project()
            .ok_or(SessionError::OwnerNotAUser(session.security_attributes.owner_id))?;
        session.project = own.id;
        let founders: Vec<(ChannelId, Vec<Participant>)> = channels
            .iter_mut()
            .map(|channel| (channel.id, mem::take(&mut channel.participants)))
            .collect();
        let session_loop = self.establish(context, session, &own, channels).await?;
        for (channel, participants) in founders {
            let channel = session_loop
                .channel(channel)
                .ok_or(SessionError::ChannelNotFound { session: session_loop.id(), channel })?;
            for participant in participants {
                channel.found(context, participant.actor_id()).await?;
            }
        }
        Ok(session_loop)
    }

    /// Establishes the brand-new `session` with its own project `own` and its `channels`, whose
    /// creation `context` was asked for (`CreateSession`) before anything was found or made for
    /// it: persists them (`persist`), announces their existence, then starts the session. The
    /// channels are empty (issue 420): their founders join once the session runs, each admitted as
    /// they join (`ChannelLoop::found`).
    ///
    /// # Errors
    /// As `persist`; whatever starting the session reports (`spawn_session`).
    async fn establish(
        &self,
        context: &SecurityContext,
        session: Session,
        own: &Project,
        channels: Vec<Channel>,
    ) -> Result<Arc<SessionLoop>> {
        let id = session.id;
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        self.persist(context, &session, own, &channels).await?;

        let event_bus = &self.events;
        event_bus
            .publish(GlobalEvent::created::<Session>(&id), session.security_attributes.clone());
        event_bus.announce(GlobalEvent::created::<Project>(&own.id), own);
        for channel in &channels {
            event_bus.announce(GlobalEvent::created::<Channel>(&(id, channel.id)), channel);
        }

        self.spawn_session(session, channels).await
    }

    /// Stores the brand-new `session` with its own project `own` and its `channels`, whose
    /// creation `context` was asked for (`CreateSession`): the channels as documents of their own
    /// (issue 70), the session and each channel with the group of its participants (issue 286),
    /// which it is read by, and the project in the runtime's name, as the groups are (issue 467).
    /// The classifications are validated first; the store asks to create the session again and
    /// each channel as it stores them. Nothing half-made stays behind: if a step fails once the
    /// session is stored, what was stored is purged again, in the runtime's name.
    ///
    /// # Errors
    /// `SessionError::ClassificationRefused` for a classification no channel may be given;
    /// whatever the store reports.
    pub(super) async fn persist(
        &self,
        context: &SecurityContext,
        session: &Session,
        own: &Project,
        channels: &[Channel],
    ) -> Result<()> {
        for channel in channels {
            require_classification(&self.runtime_manager, &channel.classification)?;
        }
        self.stores.sessions.create(context, session).await?;
        let made = async {
            keep_participants(self.stores.groups.as_ref(), Group::of_session(session)).await?;
            make_own(&self.stores, own).await?;
            for channel in channels {
                self.stores.channels.create(context, channel).await?;
                keep_participants(self.stores.groups.as_ref(), Group::of_channel(channel)).await?;
            }
            Ok::<_, StoreError>(())
        }
        .await;
        if let Err(error) = made {
            // The session's creation was permitted, so it is rolled back in the runtime's name -
            // what was not made yet is not found, which is no matter.
            for channel in channels {
                let _ = purge_channel_document(&self.stores, channel.id).await;
            }
            let _ = self.purge_own(own).await;
            let _ =
                purge_for_good(self.stores.groups.as_ref(), GroupId::of_session(session.id)).await;
            let _ = purge_for_good(self.stores.sessions.as_ref(), session.id).await;
            return Err(error.into());
        }
        Ok(())
    }

    /// The channels `session` lists, loaded in the runtime's name: starting a
    /// session the caller may read follows from an action already permitted,
    /// and it must not fail because the caller is not in one of its channels.
    async fn load_channels(&self, session: &Session) -> Result<Vec<Channel>> {
        let system = SecurityContext::system();
        let mut channels = Vec::with_capacity(session.channels.len());
        for id in &session.channels {
            channels.push(self.stores.channels.get(&system, *id).await?);
        }
        Ok(channels)
    }

    /// The session `id` for `context`: the running one, or loaded from the
    /// store and started - which announces it as `session::STARTED`, not
    /// as created, as the session exists already. A channel whose components
    /// cannot be read is left out as the session starts, and built again
    /// whenever the running session is opened (`SessionLoop::retry_unavailable`,
    /// issue 463).
    ///
    /// Started once, however many ask at the same time (issue 62): whoever
    /// comes second waits for the first and gets the session it started -
    /// if they may read it.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such session or `context` may
    /// not read it; whatever loading or starting it reports otherwise.
    pub async fn open(&self, context: &SecurityContext, id: SessionId) -> Result<Arc<SessionLoop>> {
        let start = self.start_lock(id);
        let starting = start.lock().await;
        let session_loop = match self.find(context, id).await? {
            Found::Running(session_loop) => session_loop,
            Found::Stored(session) => {
                let channels = self.load_channels(&session).await?;
                return self.spawn_session(session, channels).await;
            }
        };
        // Not under the lock: building a channel takes its time, and the
        // session runs already - whoever else opens, stops or deletes it does
        // not wait for it.
        drop(starting);
        session_loop.retry_unavailable().await;
        Ok(session_loop)
    }

    /// Takes `participant` - whom `context` acts for: themselves, or their
    /// own agent - into session `id` by their own act, and with `into` into
    /// that channel (issue 221, `SessionLoop::join`). The session is opened
    /// in the runtime's name if an entry of its access list lets `context`
    /// join it, or they may read it: who may join reads nothing of it before
    /// they did.
    ///
    /// # Errors
    /// `SessionError::NotFound` if there is no such session or `context` may
    /// neither join nor read it; as `SessionLoop::join` otherwise.
    pub async fn join(
        &self,
        context: &SecurityContext,
        id: SessionId,
        participant: ActorId,
        into: Option<ChannelId>,
    ) -> Result<Arc<SessionLoop>> {
        let not_found = || SessionError::NotFound(id);
        let system = SecurityContext::system();
        let attributes = self.find(&system, id).await.map_err(|_| not_found())?.attributes();
        let reachable = self
            .access_policy
            .check(context, ActionType::custom(Session::KIND, &JOIN, &attributes))
            .await
            || self.access_policy.may_read_session(context, &attributes).await;
        if !reachable {
            return Err(not_found().into());
        }
        let session_loop = self.open(&system, id).await?;
        session_loop.join(context, participant, into).await?;
        Ok(session_loop)
    }

    /// Starts again, as the process starts, every session in which an
    /// agent's work was under way when it last ended (issue 388) - shut down
    /// or crashed -, as the state store's index of the work under way lists
    /// them (`StateStore::channels_with_work_under_way`): the turns stored
    /// mid-way go on from the step they stood at, the remote tasks are
    /// followed again. Nobody asked for it, so the runtime opens them in its
    /// own name, as `join` does; what the agents then do is done in their
    /// own names, as at every start. One after another, not bounded
    /// otherwise. A session that is gone or deleted is passed over; the
    /// sessions with no work under way stay stopped until someone acts in
    /// them. Ends early once the sessions are shut down. Returns how many
    /// it started.
    pub async fn resume_interrupted(&self) -> usize {
        let channels = match self.state_store.channels_with_work_under_way().await {
            Ok(channels) => channels,
            Err(error) => {
                warn!("The work under way at the last end cannot be read, nothing is started again: {error}");
                return 0;
            }
        };
        let system = SecurityContext::system();
        let mut sessions = HashSet::new();
        for channel_id in channels {
            match self.stores.channels.get(&system, channel_id).await {
                Ok(channel) => {
                    sessions.insert(channel.session_id);
                }
                Err(error) => info!(
                    "Channel {channel_id}, in which work was under way, is passed over: {error}"
                ),
            }
        }
        let mut started = 0;
        for session_id in sessions {
            if self.refuse_while_shutting_down().is_err() {
                break;
            }
            match self.open(&system, session_id).await {
                Ok(_) => {
                    info!("Session {session_id} started again: work was under way in it");
                    started += 1;
                }
                Err(error) => info!("Session {session_id}, in which work was under way, is not started again: {error}"),
            }
        }
        started
    }

    /// Stops session `id` on behalf of `context` without deleting it: its
    /// channels and actors stop, for everybody in it (`StopSession`) - each
    /// actor keeps what it is in the middle of, to be resumed when the
    /// session runs again, and is given up to `STOP_GRACE` to store it
    /// (issue 377). Returns whether it ran.
    ///
    /// Waits for a start of the same session that is under way (issue 62):
    /// otherwise that start could register the session after it was stopped,
    /// and it would run on.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such session or `context` may
    /// not read it, `StoreError::PermissionDenied` if they may read but not
    /// stop it - whether it runs or not.
    pub async fn stop(&self, context: &SecurityContext, id: SessionId) -> Result<bool> {
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        let found = self.find(context, id).await?;
        let attributes = found.attributes();
        self.access_policy
            .require(context, ActionType::StopSession(&attributes), StoreError::PermissionDenied)
            .await?;
        if !matches!(found, Found::Running(_)) {
            return Ok(false);
        }
        let Some(stopped) = self.stop_running(id, SessionLoop::stop_actors).await else {
            return Ok(false);
        };
        self.end_commands_of(&stopped).await;
        Ok(true)
    }

    /// Shuts the sessions down as the process ends (issue 297): from now on
    /// none is started, and every running one is stopped as `stop` stops
    /// it, all at once - each actor keeping what it is in the middle of, to
    /// be resumed when its session runs again. Of the commands, only what
    /// does not outlive the process is ended (`CommandReaper::shutdown`):
    /// an Open Terminal instance keeps its commands, unlike at a `stop`.
    /// Returns within `SHUTDOWN_BOUND`, leaving whatever is still under way
    /// to end with the process: a shutdown that hangs is worse than none.
    pub async fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        let ending = async {
            // Those being started too: their start is waited for, as `stop`
            // waits for it, and what started is stopped.
            let ids: HashSet<SessionId> = self
                .starts
                .iter()
                .map(|entry| *entry.key())
                .chain(self.sessions.iter().map(|entry| *entry.key()))
                .collect();
            join_all(ids.into_iter().map(|id| async move {
                let start = self.start_lock(id);
                let _starting = start.lock().await;
                self.stop_running(id, SessionLoop::stop_actors).await;
            }))
            .await;
            self.command_reaper.shutdown().await;
        };
        match time::timeout(SHUTDOWN_BOUND, ending).await {
            Ok(()) => info!("Every session is shut down"),
            Err(_) => warn!("Shutting the sessions down took longer than {:?}; what is still under way ends with the process", SHUTDOWN_BOUND),
        }
    }

    /// `SessionError::ShuttingDown` once the sessions are shut down.
    fn refuse_while_shutting_down(&self) -> Result<(), SessionError> {
        match self.shutting_down.load(Ordering::SeqCst) {
            true => Err(SessionError::ShuttingDown),
            false => Ok(()),
        }
    }

    /// Ends every command started in a channel of `stopped`, with what it
    /// started: what stops a session ends what runs in its channels (issue
    /// 254).
    async fn end_commands_of(&self, stopped: &SessionLoop) {
        self.command_reaper.end_commands_in(&stopped.session().channels).await;
    }

    /// Stops session `id` if it runs. Called with its start lock held. Its
    /// actors are ended first by `end_actors`, told to stop
    /// (`SessionLoop::stop_actors`) or to leave (`SessionLoop::leave_actors`),
    /// and given `STOP_GRACE` to end by themselves before the session's
    /// task, and with it whoever did not, is aborted (issue 377). Nobody
    /// finds it running meanwhile.
    async fn stop_running(
        &self,
        id: SessionId,
        end_actors: impl AsyncFnOnce(&SessionLoop),
    ) -> Option<Arc<SessionLoop>> {
        let handle = self.handles.remove(&id).map(|(_, handle)| handle);
        let removed = self.sessions.remove(&id).map(|(_, s)| s);
        if let Some(session_loop) = &removed {
            end_actors(session_loop).await;
        }
        if let Some(handle) = handle {
            handle.abort();
        }
        if let Some(session_loop) = &removed {
            let session = Arc::clone(&session_loop.session());
            self.events.publish(
                GlobalEvent::custom::<Session>(&id, session::STOPPED, &()),
                session.security_attributes.clone(),
            );
        }
        removed
    }

    /// Stops and deletes a session. Deleting marks (issue 164): the session is kept, with its
    /// channels and their state, hidden until it is restored (`restore`) or purged (`purge`). What
    /// lies in its scope and in its own project's is deleted with it, in `context`'s name (issue
    /// 320), and then the groups of its participants and its channels', and its own project with
    /// the group of its members, in the runtime's (issues 348, 467) - all of it with the session's
    /// mark, by `context`'s caller (issue 469).
    ///
    /// Asked about before anything happens - a refused deletion must not stop the session on the
    /// way (issue 11).
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such session or `context` may not read it,
    /// `StoreError::PermissionDenied` if they may read but not delete it,
    /// `SessionError::ScopeNotEmpty` if something of its scope or its own project's is left that
    /// they may not delete - what they may is deleted, the session is kept; whatever else the store
    /// reports, the groups of its participants or its own project failing to go among it, which
    /// keeps the session too, with its groups and its project.
    pub async fn delete(&self, context: &SecurityContext, id: SessionId) -> Result<bool> {
        self.delete_with(context, id, &Removal::now(context.caller_id)).await
    }

    /// Deletes session `id` on behalf of `context` as `delete` does, marked with `removal`, the
    /// mark of a deletion it goes with: of the project it lies in (issue 469).
    ///
    /// # Errors
    /// As `delete`.
    pub(crate) async fn delete_with(
        &self,
        context: &SecurityContext,
        id: SessionId,
        removal: &Removal,
    ) -> Result<bool> {
        // Held until the session is gone from the store, so that no request
        // starts it again from there in the meantime (issue 62).
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        let session = self.find(context, id).await?.session();
        self.access_policy
            .require(
                context,
                ActionType::delete(Session::KIND, &session.security_attributes),
                StoreError::PermissionDenied,
            )
            .await?;
        // Captured before the runtime loop is torn down.
        // The events carry the attributes as they were: once deleted, the
        // session is nowhere to look them up (issue 39).
        let channels = self.load_channels(&session).await?;
        let own = load_own(&self.stores, id).await?;

        // Marked, not removed (issue 164): its channels and their state stay as they are, hidden
        // with it - a channel is reached only through its session - until it is purged. Marked
        // before what lies in its scope, so that nothing of it goes unless the session's deletion
        // was granted; all of it carries the session's mark and comes back with it (`restore`,
        // issues 178, 469).
        if !self.stores.sessions.delete(context, id, removal).await? {
            return Ok(false);
        }
        // What lies in its scope and in its channels' goes as far as `context` may delete it (issue
        // 320), and what lies in its own project's, which is the session's. Its agents take part in
        // no other session (`Scope::admits`): they leave nothing. Then the groups of its
        // participants and its channels' go, in the runtime's name (issue 348) - last, since its
        // participants reach it by them while it is kept: all of them or none -, and its own
        // project with the group of its members. If something is left, or the groups or the project
        // cannot go, the deletion is rolled back, its mark taken away again - in the runtime's
        // name: the deletion was permitted - and the session runs on if it ran, without what was
        // deleted, and `context` is told why.
        let scope = Scope::SessionScope { session_id: id };
        let holders = held_by(id, own.as_ref());
        let deleted = async {
            let left = self.scoped.delete_lying_in(context, &holders, removal).await?;
            ScopeNotEmpty::require_empty(&scope, left).map_err(SessionError::from)?;
            self.delete_kept(&holders, own.as_ref(), removal).await
        }
        .await;
        if let Err(error) = deleted {
            if let Err(undo) = self.stores.sessions.restore(&SecurityContext::system(), id).await {
                warn!("Session {} could not be deleted and not be unmarked again: {}", id, undo);
            }
            return Err(error);
        }

        // Its actors leave, as those of a deleted channel do (issue 377),
        // and the commands of its channels end.
        if let Some(stopped) = self.stop_running(id, SessionLoop::leave_actors).await {
            self.end_commands_of(&stopped).await;
        }

        let event_bus = &self.events;
        for channel in &channels {
            event_bus.announce(GlobalEvent::deleted::<Channel>(&(id, channel.id)), channel);
        }
        event_bus
            .publish(GlobalEvent::deleted::<Session>(&id), session.security_attributes.clone());
        self.starts.remove(&id);
        Ok(true)
    }

    /// Restores the deleted session `id` on behalf of `context` (issue 164): its mark taken away,
    /// with the channels it listed when it was deleted, the groups of its participants and theirs,
    /// and its own project with the group of its members, in the runtime's name (issues 348, 467);
    /// then what lay in its scope and its own project's, as far as `context` may restore it. It is
    /// not started; opening it starts it, as any stored session. A session that lies in a shared
    /// project is not restored while the project is deleted: it comes back with the project.
    ///
    /// # Errors
    /// `SessionError::NotFound` if there is no such deleted session or `context` may not see it
    /// (`ReadDeleted`, asked first, issue 304), `SessionError::ProjectDeleted` if the shared
    /// project it lies in is deleted, `StoreError::PermissionDenied` if they may but not restore
    /// it, or whatever the store reports as the groups of its participants or its own project are
    /// restored - the session stays deleted then.
    pub async fn restore(&self, context: &SecurityContext, id: SessionId) -> Result<Session> {
        // The lock of deleting and purging it: a purge never meets it half
        // restored.
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        // Its mark, read before it goes - as `context`, so that whoever may not see it learns
        // nothing more: what was deleted with it carries it and comes back with it (issues 178,
        // 469), its own project among it.
        let Deleted { entity: deleted, removal } =
            self.stores.sessions.get_deleted(context, id).await.map_err(store_error(id))?;
        let own = self
            .stores
            .projects
            .list_deleted_with(&SecurityContext::system(), removal.id, &lying_in(id))
            .await?
            .items
            .pop();
        // A session in a shared project lies where its project is, and comes back with it.
        let lies_in_own = own.as_ref().is_some_and(|own| own.id == deleted.project);
        if !lies_in_own
            && self
                .stores
                .projects
                .get(&SecurityContext::system(), deleted.project)
                .await
                .optional()?
                .is_none()
        {
            return Err(SessionError::ProjectDeleted(deleted.project).into());
        }
        let session = self.stores.sessions.restore(context, id).await.map_err(store_error(id))?;
        // The groups of its participants and its own project first, with it: whatever fails later,
        // its participants reach it (issue 348) and its results have their place (issue 467). If
        // they cannot come back, the restoring is rolled back: the session is marked again as it
        // was, in the runtime's name, as its restoring was permitted - so that a restoring tried
        // again finds what went with it (issue 469).
        let holders = held_by(id, own.as_ref());
        if let Err(error) = self.restore_kept(&holders, own.as_ref(), &removal).await {
            if let Err(undo) =
                self.stores.sessions.delete(&SecurityContext::system(), id, &removal).await
            {
                warn!(
                    "Session {} could not be restored and not be marked deleted again: {}",
                    id, undo
                );
            }
            return Err(error);
        }
        if let Err(e) = self.scoped.restore_lying_in(context, &holders, &removal).await {
            warn!(
                "Session {} was restored, but not what lies in its scope or its project's: {}",
                id, e
            );
        }
        self.events
            .publish(GlobalEvent::restored::<Session>(&id), session.security_attributes.clone());
        info!("Session {} restored by {}", id, context.caller_id);
        Ok(session)
    }

    /// Deletes what a session keeps with it, in the runtime's name, as it is deleted, marked with
    /// `removal`, its mark (issue 469): the groups that lie in `holders` - the scopes it holds
    /// (`held_by`) -, of its participants, its channels' and its own project's members, then `own`,
    /// its own project - all of them or, if one cannot go, none (issues 348, 467). The project's
    /// deletion is announced once all are deleted.
    ///
    /// # Errors
    /// Whatever the store reports.
    async fn delete_kept(
        &self,
        holders: &[Scope],
        own: Option<&Project>,
        removal: &Removal,
    ) -> Result<()> {
        self.groups.delete_participants(holders, removal).await?;
        let Some(own) = own else { return Ok(()) };
        if let Err(error) =
            self.stores.projects.delete(&SecurityContext::system(), own.id, removal).await
        {
            if let Err(undo) = self.groups.restore_participants(holders, removal).await {
                warn!(project = %own.id, "The groups of a session whose project could not be deleted could not be restored again: {undo}");
            }
            return Err(error.into());
        }
        self.events.announce(GlobalEvent::deleted::<Project>(&own.id), own);
        Ok(())
    }

    /// Restores what a session keeps with it, in the runtime's name, as it is restored: the groups
    /// that lie in `holders` and carry `removal`, its mark, then `own`, its own project, deleted
    /// with it - all of them or, if one cannot come back, none, those restored marked again with
    /// `removal` (issues 348, 467, 469). The project's restoring is announced once all are
    /// restored.
    ///
    /// # Errors
    /// Whatever the store reports.
    async fn restore_kept(
        &self,
        holders: &[Scope],
        own: Option<&Project>,
        removal: &Removal,
    ) -> Result<()> {
        self.groups.restore_participants(holders, removal).await?;
        let Some(own) = own else { return Ok(()) };
        match self.stores.projects.restore(&SecurityContext::system(), own.id).await {
            Ok(restored) => {
                self.events.announce(GlobalEvent::restored::<Project>(&own.id), &restored)
            }
            Err(error) => {
                if let Err(undo) = self.groups.delete_participants(holders, removal).await {
                    warn!(project = %own.id, "The groups of a session whose project could not be restored could not be deleted again: {undo}");
                }
                return Err(error.into());
            }
        }
        Ok(())
    }

    /// The projects deleted that lie in the scope of session `id` - its own, deleted with it -,
    /// read in the runtime's name.
    ///
    /// # Errors
    /// Whatever the store reports.
    async fn deleted_own(&self, id: SessionId) -> StoreResult<Vec<Deleted<Project>>> {
        Ok(self
            .stores
            .projects
            .list_deleted(&SecurityContext::system(), &lying_in(id))
            .await?
            .items)
    }

    /// Removes `own`, the own project of a session purged or whose making failed, for good with
    /// the group of its members, in the runtime's name; what lay in its scope is the caller's to
    /// have purged.
    ///
    /// # Errors
    /// Whatever the store reports.
    async fn purge_own(&self, own: &Project) -> StoreResult<()> {
        purge_for_good(self.stores.groups.as_ref(), GroupId::of_project(own.id)).await?;
        purge_for_good(self.stores.projects.as_ref(), own.id).await?;
        Ok(())
    }

    /// Removes the deleted session `id` for good on behalf of `context` (issue 164): its document,
    /// and - in the runtime's name, since the purge was permitted - the documents and the state of
    /// its channels, those it listed and those deleted from it before, the content of their
    /// attachments (issue 282), and its own project with what lies in its scope (issue 467). Only
    /// what was deleted is purged; the store asks `Purge`. Under the lock of deleting and restoring
    /// it, so that what is restored meanwhile is not purged.
    ///
    /// # Errors
    /// `SessionError::NotFound` if there is no such deleted session or `context` may not see it,
    /// `StoreError::PermissionDenied` if they may but not purge it.
    pub async fn purge(&self, context: &SecurityContext, id: SessionId) -> Result<()> {
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        let Deleted { entity: session, .. } =
            self.stores.sessions.get_deleted(context, id).await.map_err(store_error(id))?;
        if !self.stores.sessions.purge(context, id).await? {
            return Err(SessionError::NotFound(id).into());
        }
        let mut channels = session.channels.clone();
        let of_it = Query::filtered(Filter::eq(ChannelField::Session, id));
        channels.extend(
            self.stores
                .channels
                .list_deleted(&SecurityContext::system(), &of_it)
                .await?
                .items
                .into_iter()
                .map(|c| c.entity.id),
        );
        // The state lives in the `StateStore`, which may be another store
        // than the session's; the attachments its history carries go with it.
        for channel_id in channels {
            purge_channel_document(&self.stores, channel_id).await?;
            self.state_store.delete_channel_state(channel_id).await?;
            self.attachments.purge(channel_id).await?;
        }
        // And what lay in its scope and in its channels' (issue 178), the group of its
        // participants, as a channel's (issue 348), and its own project with what lay in its scope
        // (issue 467).
        let owns: Vec<Project> =
            self.deleted_own(id).await?.into_iter().map(|own| own.entity).collect();
        self.scoped.purge_lying_in(&held_by(id, &owns)).await?;
        purge_for_good(self.stores.groups.as_ref(), GroupId::of_session(id)).await?;
        for own in &owns {
            self.purge_own(own).await?;
        }
        self.starts.remove(&id);
        info!("Session {} purged by {}", id, context.caller_id);
        Ok(())
    }

    /// The root of the views (issue 163): every session `viewer` may
    /// read, and from there their channels and participants, running or not.
    pub fn views(self: &Arc<Self>, viewer: Viewer) -> Views {
        Views::new(&self.runtime_manager, Arc::<SessionManager>::clone(self), viewer)
    }

    /// The runtime it starts its sessions in - for what is made beside it, as the
    /// `ProjectManager` is.
    pub(crate) fn runtime(&self) -> &Arc<RuntimeManager> {
        &self.runtime_manager
    }

    /// The loop of session `id`, if it runs.
    pub(crate) fn running(&self, id: SessionId) -> Option<Arc<SessionLoop>> {
        self.sessions.get(&id).map(|entry| entry.clone())
    }
}

/// The scopes session `id` holds, and what lies in them goes with it: its own, with its channels',
/// and the scope of `own`, its own project, while it is there (issue 467).
fn held_by<'a>(id: SessionId, own: impl IntoIterator<Item = &'a Project>) -> Vec<Scope> {
    iter::once(Scope::SessionScope { session_id: id })
        .chain(own.into_iter().map(Project::holds))
        .collect()
}

/// `error` of the store about session `id`: that there is none - or none
/// the caller may see - is the session's own error, what the request named
/// and did not find (issue 323).
fn store_error(id: SessionId) -> impl FnOnce(StoreError) -> Error {
    move |error| match error {
        StoreError::NotFound { .. } => SessionError::NotFound(id).into(),
        error => error.into(),
    }
}

/// What lies in the scope of session `id`: of projects, its own.
fn lying_in(id: SessionId) -> Query<ScopeField> {
    Query::filtered(Filter::lying_within(&Scope::SessionScope { session_id: id }))
}

#[cfg(test)]
mod tests {
    use super::SHUTDOWN_BOUND;
    use crate::agent::AgentError;
    use crate::channel::STOP_GRACE;
    use crate::command::{ChannelCommandProvider, CommandExecutor};
    use crate::component::BUNDLE_REF_KIND;
    use crate::error::is_forbidden;
    use crate::project::ProjectManager;
    use crate::runtime::RuntimeManager;
    use crate::session::testing::*;
    use crate::session::SessionLoop;
    use crate::session::{SessionError, SessionManager};
    use crate::testing::TestRuntime;
    use crate::view::Viewer;
    use anyhow::Result;
    use caelumora_model::actor::ActorId;
    use caelumora_model::actor::{ActorEvent, ActorState, Envelope};
    use caelumora_model::agent::Agent;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::approval::ApprovalStrategy;
    use caelumora_model::channel::{self, moderation_components, Channel, InternalActor};
    use caelumora_model::component::{ComponentBundle, ComponentConfig, ComponentDescriptor};
    use caelumora_model::confidentiality::{Classification, ConfidentialityRules};
    use caelumora_model::event::EntityChange;
    use caelumora_model::group::GroupId;
    use caelumora_model::project::Project;
    use caelumora_model::removal::Removal;
    use caelumora_model::scope::Scope;
    use caelumora_model::security::{
        AllowAllPolicy, Grant, Secured, SecurityAttributes, SecurityContext, Subject,
    };
    use caelumora_model::session;
    use caelumora_model::session::ChannelId;
    use caelumora_model::session::Session;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::store::query::Query;
    use caelumora_model::store::StoreError;
    use caelumora_model::store::{Deleted, Stored};
    use caelumora_model::store::{Persistence, StateStore};
    use caelumora_model::template::{
        ChannelTemplate, InternalActorTemplate, ParticipantTemplate, SessionTemplate,
        SessionTemplateId,
    };
    use caelumora_model::turn::{GivenUpReason, TurnState};
    use caelumora_model::user::UserProfile;
    use caelumora_model::user::{User, UserId};
    use caelumora_model::value::language::Language;
    use caelumora_model::value::name::Name;
    use caelumora_model::value::name::ScopedName;
    use caelumora_model::value::time_zone::TimeZone;
    use caelumora_model::value::ValueError;
    use caelumora_model::workspace::{Workspace, WorkspaceChoice};
    use serde_json::Value;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;
    use std::time::Instant;
    use tokio::sync::broadcast::Receiver;
    use tokio::time;

    #[tokio::test]
    async fn create_session_emits_created_then_started() {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let mut rx = runtime.event_bus().subscribe_all();
        let ctx = SecurityContext::default();

        let session = make_session("s1", UserId::new());
        let session_id = session.id;
        let channel_id = session.channels[0];

        create(&manager, &ctx, session).await.unwrap();

        // The manager emits these synchronously, in order, before returning.
        let events = drain(&mut rx);
        let created = events
            .iter()
            .position(|e| e.about::<Session>() == Some((session_id, &EntityChange::Created)));
        let ch_created = events.iter().position(|e| {
            e.about::<Channel>() == Some(((session_id, channel_id), &EntityChange::Created))
        });
        let started = events.iter().position(|e| matches!(e.about::<Session>(), Some((id, EntityChange::Custom { name: session::STARTED, .. })) if id == session_id));

        assert!(created.is_some(), "expected SessionCreated, got {:?}", events);
        assert!(ch_created.is_some(), "expected the channel created, got {:?}", events);
        assert!(started.is_some(), "expected SessionStarted, got {:?}", events);
        assert!(created < ch_created && ch_created < started, "wrong order: {:?}", events);

        // It is now persisted and running.
        assert!(runtime.entity_stores().sessions.get(&ctx, session_id).await.is_ok());
        assert!(manager.sessions.contains_key(&session_id));

        manager.stop(&SecurityContext::system(), session_id).await.unwrap();
    }

    #[tokio::test]
    async fn delete_session_emits_stopped_then_deleted() {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let ctx = SecurityContext::default();

        let session = make_session("s2", UserId::new());
        let session_id = session.id;
        let channel_id = session.channels[0];
        create(&manager, &ctx, session).await.unwrap();

        // Subscribe after creation so we only observe the deletion events.
        let mut rx = runtime.event_bus().subscribe_all();
        assert!(manager.delete(&ctx, session_id).await.unwrap());

        let events = drain(&mut rx);
        let stopped = events.iter().position(|e| matches!(e.about::<Session>(), Some((id, EntityChange::Custom { name: session::STOPPED, .. })) if id == session_id));
        let ch_deleted = events.iter().position(|e| {
            e.about::<Channel>() == Some(((session_id, channel_id), &EntityChange::Deleted))
        });
        let deleted = events
            .iter()
            .position(|e| e.about::<Session>() == Some((session_id, &EntityChange::Deleted)));

        assert!(stopped.is_some(), "expected SessionStopped, got {:?}", events);
        assert!(ch_deleted.is_some(), "expected the channel deleted, got {:?}", events);
        assert!(deleted.is_some(), "expected SessionDeleted, got {:?}", events);
        assert!(stopped < ch_deleted && ch_deleted < deleted, "wrong order: {:?}", events);

        assert!(runtime.entity_stores().sessions.get(&ctx, session_id).await.is_err());
        assert!(!manager.sessions.contains_key(&session_id));
    }

    #[tokio::test]
    async fn opening_a_stored_session_emits_started_but_not_created() {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let ctx = SecurityContext::default();

        // Persist a session directly, without going through `create`.
        let session = make_session("s3", UserId::new());
        let session_id = session.id;
        let channels = [main_channel(&session)];
        persist(&manager, &ctx, session, &channels).await;

        let mut rx = runtime.event_bus().subscribe_all();
        manager.open(&ctx, session_id).await.unwrap();

        let events = drain(&mut rx);
        assert!(
            events.iter().any(|e| matches!(e.about::<Session>(), Some((id, EntityChange::Custom { name: session::STARTED, .. })) if id == session_id)),
            "expected SessionStarted, got {:?}", events
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.about::<Session>(), Some((_, EntityChange::Created)))),
            "load must not emit SessionCreated, got {:?}",
            events
        );

        manager.stop(&SecurityContext::system(), session_id).await.unwrap();
    }

    #[tokio::test]
    async fn a_session_asked_for_at_the_same_time_starts_once() {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let ctx = SecurityContext::default();
        let session = make_session("concurrent", UserId::new());
        let session_id = session.id;
        let channels = [main_channel(&session)];
        persist(&manager, &ctx, session, &channels).await;

        let mut rx = runtime.event_bus().subscribe_all();
        let (first, second, third) = tokio::join!(
            manager.open(&ctx, session_id),
            manager.open(&ctx, session_id),
            manager.open(&ctx, session_id),
        );
        let (first, second, third) = (first.unwrap(), second.unwrap(), third.unwrap());
        assert!(
            Arc::ptr_eq(&first, &second) && Arc::ptr_eq(&first, &third),
            "one loop for all of them"
        );

        let started = drain(&mut rx).into_iter().filter(|e| matches!(e.about::<Session>(), Some((id, EntityChange::Custom { name: session::STARTED, .. })) if id == session_id)).count();
        assert_eq!(started, 1, "the session started once");
        manager.stop(&SecurityContext::system(), session_id).await.unwrap();
    }

    #[tokio::test]
    async fn a_stop_waits_for_a_start_under_way_and_then_stops_what_it_started() {
        let runtime = test_runtime();
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let ctx = SecurityContext::default();
        let session = make_session("stopped", UserId::new());
        let session_id = session.id;
        let channels = [main_channel(&session)];
        let session = persist(&manager, &ctx, session, &channels).await;

        // A start under way holds the lock ...
        let start = manager.start_lock(session_id);
        let starting = start.lock().await;
        let stopping = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.stop(&SecurityContext::system(), session_id).await.unwrap() }
        });
        time::sleep(Duration::from_millis(50)).await;
        assert!(!stopping.is_finished(), "the stop waits for the start");

        // ... and registers the session before it lets go: the stop then
        // finds it, and nothing keeps running.
        manager.spawn_session(session, channels.to_vec()).await.unwrap();
        drop(starting);
        assert!(stopping.await.unwrap(), "the stop stopped what the start started");
        assert!(!manager.sessions.contains_key(&session_id));
    }

    #[tokio::test]
    async fn a_session_has_the_global_commands_and_its_own_but_no_name_twice() {
        use caelumora_model::command::HELP_COMMAND_KIND;
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        let runtime = test_runtime();
        let names = |executor: CommandExecutor<dyn ChannelCommandProvider>| async move {
            executor
                .visible(&SecurityContext::default(), &SecurityAttributes::default())
                .await
                .into_iter()
                .map(|c| c.name)
                .collect::<Vec<_>>()
        };

        let session = make_session("plain", UserId::new());
        let executor =
            runtime.new_command_executor(&session).await.expect("the global commands build");
        assert_eq!(names(executor).await, vec!["help", "nudge", "wakeup"]);

        // A second /help from the session's own configuration - with the same
        // component id, which is fine across the two configurations.
        let mut session = make_session("clashing", UserId::new());
        session.commands = ComponentConfig {
            component_nodes: vec![ComponentDescriptor {
                id: "help".to_string(),
                kind: HELP_COMMAND_KIND.to_string(),
                params: Value::Null,
                tags: Vec::new(),
                fail_policy: Default::default(),
            }],
        };
        let error = runtime.new_command_executor(&session).await.expect_err("/help twice");
        assert!(error.to_string().contains("/help"), "{error}");
    }

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn a_deleted_agent_rests_in_its_channels_keeps_its_name_and_comes_back_restored() {
        use caelumora_model::agent::{Personality, PersonalityId};
        use caelumora_model::channel::Participant;
        /// Waits until `holds` does, at most two seconds: a running channel
        /// follows the agent as the session learns of it.
        async fn until(holds: impl Fn() -> bool, what: &str) {
            let waited = async {
                while !holds() {
                    time::sleep(Duration::from_millis(10)).await
                }
            };
            time::timeout(Duration::from_secs(2), waited)
                .await
                .unwrap_or_else(|_| panic!("{what}"));
        }
        let runtime = test_runtime();
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let agents = runtime.agent_manager();
        let (system, ctx) = (SecurityContext::system(), SecurityContext::default());
        let personality = Personality {
            id: PersonalityId::new(),
            name: ScopedName::new("plain", Scope::GlobalScope).unwrap(),
            kind: Default::default(),
            roles: None,
            description: None,
            introduction: None,
            components: Default::default(),
            security_attributes: Default::default(),
            reasoning_visibility: Default::default(),
            hears_during_turn: Default::default(),
            deleted: None,
        };
        runtime.entity_stores().personalities.create(&system, &personality).await.unwrap();
        let helper = Agent::new(
            ScopedName::new("helper", Scope::GlobalScope).unwrap(),
            personality.id,
            ActorId::None,
        );
        runtime.entity_stores().agents.create(&system, &helper).await.unwrap();
        let actor = ActorId::Agent(helper.id);

        // In a running session's channel, and in a stopped one's.
        let mut running = make_session("running", UserId::new());
        running.participants = vec![Participant::Agent(helper.id)];
        let running = create(&manager, &ctx, running).await.unwrap();
        let talk = running.create_channel(&ctx, spec("talk", vec![actor])).await.unwrap();
        let mut stopped = make_session("stopped", UserId::new());
        stopped.participants = vec![Participant::Agent(helper.id)];
        let channels = [main_channel(&stopped)];
        let stopped = persist(&manager, &system, stopped, &channels).await;

        assert!(agents.delete(&ctx, helper.id).await.expect("deleted"));
        until(
            || !talk.participant_ids().contains(&actor),
            "its actor stops in the running channel",
        )
        .await;
        let rests = |channel: &Channel| {
            channel.participants.iter().any(|p| p.actor_id() == actor)
                && !channel.former.iter().any(|f| f.participant.actor_id() == actor)
        };
        assert!(rests(&talk.channel()), "it rests in the running channel");
        assert!(
            rests(
                &runtime.entity_stores().channels.get(&system, stopped.channels[0]).await.unwrap()
            ),
            "and in the stopped one"
        );
        assert!(
            runtime.entity_stores().agents.get(&system, helper.id).await.is_err(),
            "it is not there"
        );
        let views = manager.views(Viewer::new(ctx.clone()));
        let reached = views.channel_of(running.id(), talk.id()).await.unwrap().there().unwrap();
        let former = reached.participant(actor).await.unwrap();
        assert!(
            former.former() && former.name() == Some("helper"),
            "what it said keeps its sender"
        );
        assert_eq!(former.left().map(|left| left.by), Some(ctx.caller_id), "left as its mark says");
        assert!(!reached.participants().await.iter().any(|m| m.id() == actor), "no participant");
        assert!(
            reached.former_participants().await.iter().any(|m| m.id() == actor),
            "but a former one"
        );

        // A session that starts meanwhile leaves it out.
        let opened = manager.open(&ctx, stopped.id).await.unwrap();
        let main = opened.channel(stopped.channels[0]).unwrap();
        assert!(!main.participant_ids().contains(&actor), "not run where it rests");

        agents.restore(&ctx, helper.id).await.expect("restored");
        until(|| talk.participant_ids().contains(&actor), "it runs again in the running channel")
            .await;
        until(|| main.participant_ids().contains(&actor), "and in the one started meanwhile").await;
        let reached = views.channel_of(running.id(), talk.id()).await.unwrap().there().unwrap();
        assert!(!reached.participant(actor).await.unwrap().former(), "a participant again");
        assert!(agents.restore(&ctx, helper.id).await.is_err(), "not deleted");
        assert!(!agents.purge(&ctx, helper.id).await.unwrap(), "only what was deleted is purged");

        // Removed from a channel while it rests: restored, it is not back there.
        assert!(agents.delete(&ctx, helper.id).await.unwrap());
        until(|| !main.participant_ids().contains(&actor), "its actor stops").await;
        main.remove_participant(&ctx, actor).await.expect("removed while it rests");
        assert!(
            main.channel().former.iter().any(|f| f.participant.actor_id() == actor),
            "a former participant there"
        );

        // Its name taken meanwhile: not restored.
        let again = Agent::new(
            ScopedName::new("helper", Scope::GlobalScope).unwrap(),
            personality.id,
            ActorId::None,
        );
        runtime.entity_stores().agents.create(&system, &again).await.unwrap();
        let taken = agents.restore(&ctx, helper.id).await.expect_err("its name is taken");
        assert!(
            matches!(taken, AgentError::Store(StoreError::NameTaken { entity: "agent" })),
            "{taken}"
        );

        // Purged: a former participant still, not knowing when it left.
        assert!(agents.purge(&ctx, helper.id).await.expect("purged"));
        assert!(
            runtime.entity_stores().agents.get_deleted(&system, helper.id).await.is_err(),
            "nothing left to restore"
        );
        let reached = views.channel_of(running.id(), talk.id()).await.unwrap().there().unwrap();
        let purged = reached.participant(actor).await.unwrap();
        assert!(purged.former() && purged.left().is_none(), "former, without when");
        assert!(!agents.delete(&ctx, helper.id).await.unwrap(), "nothing to delete");
        manager.stop(&system, running.id()).await.unwrap();
        manager.stop(&system, opened.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_session_that_missed_an_agent_being_deleted_finds_out_as_it_looks_again() {
        use caelumora_model::agent::{Personality, PersonalityId};
        use caelumora_model::channel::Participant;
        let runtime = test_runtime();
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let (system, ctx) = (SecurityContext::system(), SecurityContext::default());
        let personality = Personality {
            id: PersonalityId::new(),
            name: ScopedName::new("plain", Scope::GlobalScope).unwrap(),
            kind: Default::default(),
            roles: None,
            description: None,
            introduction: None,
            components: Default::default(),
            security_attributes: Default::default(),
            reasoning_visibility: Default::default(),
            hears_during_turn: Default::default(),
            deleted: None,
        };
        runtime.entity_stores().personalities.create(&system, &personality).await.unwrap();
        let helper = Agent::new(
            ScopedName::new("helper", Scope::GlobalScope).unwrap(),
            personality.id,
            ActorId::None,
        );
        runtime.entity_stores().agents.create(&system, &helper).await.unwrap();
        let actor = ActorId::Agent(helper.id);
        let mut session = make_session("running", UserId::new());
        session.participants = vec![Participant::Agent(helper.id)];
        let session = create(&manager, &ctx, session).await.unwrap();
        let talk = session.create_channel(&ctx, spec("talk", vec![actor])).await.unwrap();

        // Deleted with nothing announced, as if the session lost the event.
        assert!(talk.participant_ids().contains(&actor));
        let mut rx = runtime.event_bus().subscribe_all();
        runtime
            .entity_stores()
            .agents
            .delete(&system, helper.id, &Removal::now(system.caller_id))
            .await
            .unwrap();
        session.follow_agents(None).await;
        assert!(!talk.participant_ids().contains(&actor), "looking again, it stops the actor");
        let announced = drain(&mut rx);
        assert!(announced.iter().any(|e| matches!(e.about::<Channel>(), Some((address, change)) if address == (session.id(), talk.id()) && change.data::<ActorId>(channel::LEFT) == Some(ActorId::Agent(helper.id)))), "as a leave: {announced:?}");

        runtime.entity_stores().agents.restore(&system, helper.id).await.unwrap();
        session.follow_agents(None).await;
        assert!(talk.participant_ids().contains(&actor), "and starts it again");
        let announced = drain(&mut rx);
        assert!(announced.iter().any(|e| matches!(e.about::<Channel>(), Some(((_, c), change)) if c == talk.id() && change.data::<ActorId>(channel::JOINED) == Some(ActorId::Agent(helper.id)))), "as a join: {announced:?}");
        manager.stop(&system, session.id()).await.unwrap();
    }

    /// The template "pairing", stored: one channel "main" for its "user", moderated by a broadcast
    /// and with an approver deciding by all. Returned with the user it is made for, stored too.
    async fn pairing(runtime: &RuntimeManager) -> (UserId, SessionTemplate) {
        let system = SecurityContext::system();
        let owner = UserId::new();
        let alice = User {
            id: owner,
            username: Name::new("alice").unwrap(),
            email: String::new(),
            roles: Vec::new(),
            deleted: None,
        };
        runtime.entity_stores().users.create(&system, &alice).await.unwrap();
        let template = SessionTemplate {
            access: Vec::new(),
            summary: None,
            briefing: None,
            language: None,
            time_zone: None,
            approval_rules: Vec::new(),
            workspace: None,
            id: SessionTemplateId::new(),
            name: ScopedName::new("pairing", Default::default()).unwrap(),
            description: None,
            participants: vec![ParticipantTemplate::User { name: "user".to_string() }],
            channels: vec![ChannelTemplate {
                access: Vec::new(),
                summary: None,
                briefing: None,
                language: None,
                approval_rules: Vec::new(),
                uses_workspace: true,
                name: Name::new("main").unwrap(),
                components: moderation_components(None, Some("broadcast_policy")),
                classification: Classification::CHANNEL_DEFAULT,
                participants: vec!["user".to_string()],
                internal_actors: vec![InternalActorTemplate::Approver {
                    strategy: ApprovalStrategy::All,
                }],
            }],
            security_attributes: Default::default(),
            commands: Default::default(),
            privacy: Default::default(),
            deleted: None,
        };
        runtime.entity_stores().session_templates.create(&system, &template).await.unwrap();
        (owner, template)
    }

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn a_session_is_created_from_a_template_for_its_owner() {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (owner, template) = pairing(&runtime).await;
        let hers =
            SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let mut profile = UserProfile::new(owner, None);
        profile.preferred_language = Some(Language::new("de").unwrap());
        runtime.entity_stores().user_profiles.create(&hers, &profile).await.unwrap();

        let session = manager
            .create_from_template(&hers, template.id, None, None, None, None)
            .await
            .expect("created and started");
        assert_eq!(session.name(), "pairing", "named after the template unless named");
        assert_eq!(session.session().security_attributes.owner_id, ActorId::User(owner));
        let main = session.channel_named("main").expect("the template's channel runs");
        assert!(main.user_actor(owner).is_some(), "the owner is the template's \"user\"");
        assert_eq!(
            (session.session().language.clone(), main.channel().language.clone()),
            (None, None),
            "not in the one the owner prefers: that is the frontend's to set (issue 331)"
        );
        let stored = runtime.entity_stores().channels.get(&hers, main.id()).await.unwrap();
        assert!(
            matches!(stored.internal_actors.as_slice(), [InternalActor::Approver(a)] if a.strategy == ApprovalStrategy::All),
            "an approver deciding as the template says"
        );
        assert!(
            stored.internal_actors.iter().all(|internal| main.actor(internal.actor_id()).is_some()),
            "and its actor runs"
        );
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();

        let missing = manager
            .create_from_template(&hers, SessionTemplateId::new(), None, None, None, None)
            .await
            .expect_err("no such template");
        assert!(
            matches!(
                missing.downcast_ref::<SessionError>(),
                Some(SessionError::TemplateNotFound(_))
            ),
            "{missing}"
        );
        let not_a_user = manager
            .create_from_template(&SecurityContext::system(), template.id, None, None, None, None)
            .await
            .expect_err("the system owns no session");
        assert!(
            matches!(
                not_a_user.downcast_ref::<SessionError>(),
                Some(SessionError::OwnerNotAUser(_))
            ),
            "{not_a_user}"
        );

        let asked = manager
            .create_from_template(&hers, template.id, Some("in French"), Some("fr"), None, None)
            .await
            .expect("held in the language asked for");
        assert_eq!(
            asked.session().language,
            Some(Language::new("fr").unwrap()),
            "the session, issue 425"
        );
        let stored = runtime.entity_stores().sessions.get(&hers, asked.id()).await.unwrap();
        assert_eq!(
            stored.language,
            Some(Language::new("fr").unwrap()),
            "stored, not only in memory"
        );
        let no_tag = manager
            .create_from_template(
                &hers,
                template.id,
                Some("in no language"),
                Some("no language"),
                None,
                None,
            )
            .await
            .expect_err("no language tag");
        assert!(no_tag.downcast_ref::<ValueError>().is_some(), "{no_tag}");
        let zoned = manager
            .create_from_template(
                &hers,
                template.id,
                Some("in Berlin"),
                None,
                Some("Europe/Berlin"),
                None,
            )
            .await
            .expect("held in the time zone asked for");
        assert_eq!(
            zoned.session().time_zone,
            Some(TimeZone::new("Europe/Berlin").unwrap()),
            "issue 424"
        );
        let no_zone = manager
            .create_from_template(
                &hers,
                template.id,
                Some("nowhere"),
                None,
                Some("Europe/Atlantis"),
                None,
            )
            .await
            .expect_err("no time zone");
        assert!(no_zone.downcast_ref::<ValueError>().is_some(), "{no_zone}");

        // A channel whose bundle she cannot read is refused, not left out
        // (issue 463): making it is her own request. Nothing of it stays.
        let gone = ComponentDescriptor {
            params: serde_json::json!({ "name": "gone" }),
            ..ComponentDescriptor::new("policy", BUNDLE_REF_KIND)
        };
        let channel = ChannelTemplate {
            components: ComponentConfig { component_nodes: vec![gone] },
            ..template.channels[0].clone()
        };
        let broken = SessionTemplate {
            id: SessionTemplateId::new(),
            name: ScopedName::new("broken", Default::default()).unwrap(),
            channels: vec![channel],
            ..template
        };
        runtime
            .entity_stores()
            .session_templates
            .create(&SecurityContext::system(), &broken)
            .await
            .unwrap();
        let refused = manager
            .create_from_template(&hers, broken.id, None, None, None, None)
            .await
            .expect_err("a bundle she cannot read");
        let Some(&SessionError::ChannelUnavailable { session, .. }) =
            refused.downcast_ref::<SessionError>()
        else {
            panic!("unavailable: {refused}")
        };
        let system = SecurityContext::system();
        assert!(
            runtime.entity_stores().sessions.get(&system, session).await.is_err()
                && runtime.entity_stores().sessions.get_deleted(&system, session).await.is_err(),
            "rolled back"
        );
    }

    /// Issue 476: a session given one of its owner's workspaces is not made if the workspace
    /// cannot be shared with its participants as asked, as one that cannot be completed is not.
    #[tokio::test]
    async fn a_session_whose_workspace_cannot_be_shared_is_not_made() {
        let (runtime, passing) = failing_runtime(Workspace::KIND);
        let stores = runtime.entity_stores().clone();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (owner, template) = pairing(&runtime).await;
        let hers =
            SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let system = SecurityContext::system();
        let docs = Workspace::new(
            ScopedName::new("docs", Scope::UserScope { user_id: owner }).unwrap(),
            "dev",
            Classification::default(),
            owner,
        );
        stores.workspaces.create(&system, &docs).await.unwrap();

        passing.store(0, Ordering::SeqCst);
        manager
            .create_from_template(
                &hers,
                template.id,
                None,
                None,
                None,
                Some(WorkspaceChoice::Existing { id: docs.id, share: Some(Grant::Read) }),
            )
            .await
            .expect_err("the workspace cannot be stored as shared");
        let made = stores.sessions.list(&system, &Query::all()).await.unwrap().items;
        let deleted = stores.sessions.list_deleted(&system, &Query::all()).await.unwrap().items;
        assert!(made.is_empty() && deleted.is_empty(), "rolled back: {made:?}, {deleted:?}");
    }

    #[tokio::test]
    async fn a_deleted_session_keeps_its_state_until_it_is_purged_and_can_be_restored() {
        let (entities, states) = (Arc::new(MemoryStore::new()), Arc::new(MemoryStore::new()));
        let store = Persistence::new(
            entities,
            Arc::<MemoryStore>::clone(&states),
            Arc::new(AllowAllPolicy::new()),
            Arc::new(ConfidentialityRules::default()),
        )
        .unwrap();
        let runtime = TestRuntime { store: store.clone(), ..Default::default() }.build();
        let manager = SessionManager::new(runtime);
        let ctx = SecurityContext::default();
        let session = make_session("two-stores", UserId::new());
        let (id, channel_id) = (session.id, session.channels[0]);
        create(&manager, &ctx, session).await.unwrap();
        assert!(
            states.get_channel_state(channel_id).await.is_ok(),
            "the channel keeps its state in the state store"
        );

        manager.delete(&ctx, id).await.unwrap();
        assert!(store.entity_stores().unwrap().sessions.get(&ctx, id).await.is_err(), "not there");
        assert!(
            states.get_channel_state(channel_id).await.is_ok(),
            "but kept, with its channels' state (issue 164)"
        );
        manager.restore(&ctx, id).await.expect("restored");
        manager.open(&ctx, id).await.expect("and opened as any stored session");
        assert!(manager.restore(&ctx, id).await.is_err(), "not deleted");
        assert!(manager.purge(&ctx, id).await.is_err(), "only what was deleted is purged");

        // A channel deleted from it before goes with it, too.
        let side = manager
            .open(&ctx, id)
            .await
            .unwrap()
            .create_channel(&ctx, spec("side", vec![]))
            .await
            .unwrap()
            .id();
        manager.open(&ctx, id).await.unwrap().delete_channel(&ctx, side).await.unwrap();
        assert!(states.get_channel_state(side).await.is_ok(), "kept with the deleted channel");
        manager.delete(&ctx, id).await.unwrap();
        manager.purge(&ctx, id).await.expect("purged");
        assert!(
            store.entity_stores().unwrap().sessions.get_deleted(&ctx, id).await.is_err(),
            "nothing left to restore"
        );
        assert!(
            states.get_channel_state(channel_id).await.is_err(),
            "deleted from the state store, not only from the entity store"
        );
        assert!(
            store.entity_stores().unwrap().channels.get(&ctx, channel_id).await.is_err(),
            "its channels go with it (issue 70)"
        );
        assert!(
            store.entity_stores().unwrap().channels.get_deleted(&ctx, side).await.is_err(),
            "also those deleted from it before"
        );
        assert!(states.get_channel_state(side).await.is_err(), "with their state");
    }

    /// Issue 467: a session is made with a project of its own, read by its
    /// participants, which goes, comes back and is purged with it, with the
    /// group of its members and what lies in its scope.
    #[tokio::test]
    async fn a_session_s_own_project_goes_and_comes_back_with_it() {
        use caelumora_model::security::AclEntry;

        let runtime = test_runtime();
        let stores = runtime.entity_stores().clone();
        let mut events = runtime.event_bus().subscribe_all();
        let manager = SessionManager::new(runtime);
        let (ctx, system) = (SecurityContext::default(), SecurityContext::system());
        let owner = UserId::new();
        let session = make_session("with-results", owner);
        let id = session.id;
        let project = create(&manager, &ctx, session).await.unwrap().session().project;

        let own = stores.projects.get(&system, project).await.expect("made with it");
        assert_eq!(
            (&own.name, own.owner()),
            (&Project::own_name(id), Some(owner)),
            "in its scope, its owner's"
        );
        let participants =
            AclEntry { subject: Subject::Group(GroupId::of_session(id)), grant: Grant::Read };
        assert!(own.security_attributes.acl.contains(&participants), "read by its participants");
        let members = GroupId::of_project(project);
        assert!(stores.groups.get(&system, members).await.is_ok(), "with the group of its members");
        assert!(
            drain(&mut events)
                .iter()
                .any(|e| e.about::<Project>() == Some((project, &EntityChange::Created))),
            "announced"
        );
        let bundle = ComponentBundle::new(
            ScopedName::new("tools", Scope::ProjectScope { project_id: project }).unwrap(),
            Default::default(),
            Default::default(),
        );
        stores.component_bundles.create(&system, &bundle).await.unwrap();

        manager.delete(&ctx, id).await.unwrap();
        assert!(stores.projects.get(&system, project).await.is_err(), "deleted with it");
        assert!(stores.groups.get(&system, members).await.is_err(), "its group too");
        assert!(
            stores.component_bundles.get(&system, bundle.id).await.is_err(),
            "and what lies in its scope"
        );
        assert!(
            drain(&mut events)
                .iter()
                .any(|e| e.about::<Project>() == Some((project, &EntityChange::Deleted))),
            "announced"
        );

        manager.restore(&ctx, id).await.unwrap();
        assert!(stores.projects.get(&system, project).await.is_ok(), "restored with it");
        assert!(stores.groups.get(&system, members).await.is_ok(), "its group too");
        assert!(
            stores.component_bundles.get(&system, bundle.id).await.is_ok(),
            "and what lies in its scope"
        );
        assert!(
            drain(&mut events)
                .iter()
                .any(|e| e.about::<Project>() == Some((project, &EntityChange::Restored))),
            "announced"
        );

        manager.delete(&ctx, id).await.unwrap();
        manager.purge(&ctx, id).await.unwrap();
        assert!(stores.projects.get_deleted(&system, project).await.is_err(), "purged with it");
        assert!(stores.groups.get_deleted(&system, members).await.is_err(), "its group too");
        assert!(
            stores.component_bundles.get_deleted(&system, bundle.id).await.is_err(),
            "and what lay in its scope"
        );
    }

    /// Issues 348, 467: what a session keeps with it - the groups of its participants and its
    /// channels', its own project with the group of its members - goes and comes back with it all
    /// or none: if its own project cannot go, or not come back, the session stays as it was, with
    /// its groups. Issue 469: a restoring that failed keeps its mark, and tried again, brings back
    /// all that went with it, what lay in its scope too.
    #[tokio::test]
    async fn a_session_keeps_its_groups_and_its_project_all_or_none() {
        let (runtime, passing) = failing_runtime(Project::KIND);
        let stores = runtime.entity_stores().clone();
        let manager = SessionManager::new(runtime);
        let (ctx, system) = (SecurityContext::default(), SecurityContext::system());
        let session = make_session("kept", UserId::new());
        let id = session.id;
        let project = create(&manager, &ctx, session).await.unwrap().session().project;
        let groups_there = async || {
            let session = stores.groups.get(&system, GroupId::of_session(id)).await;
            let project = stores.groups.get(&system, GroupId::of_project(project)).await;
            (session.is_ok(), project.is_ok())
        };

        passing.store(0, Ordering::SeqCst);
        manager.delete(&ctx, id).await.expect_err("its project cannot go");
        assert!(stores.sessions.get(&system, id).await.is_ok(), "the session is kept");
        assert!(stores.projects.get(&system, project).await.is_ok(), "with its project");
        assert_eq!(
            groups_there().await,
            (true, true),
            "and its groups, deleted before and restored again"
        );

        let bundle = ComponentBundle::new(
            ScopedName::new("tools", Scope::SessionScope { session_id: id }).unwrap(),
            Default::default(),
            Default::default(),
        );
        stores.component_bundles.create(&system, &bundle).await.unwrap();
        manager.delete(&ctx, id).await.expect("deleted now");
        assert_eq!(groups_there().await, (false, false));
        let mark = stores.sessions.get_deleted(&system, id).await.unwrap().removal;
        passing.store(0, Ordering::SeqCst);
        manager.restore(&ctx, id).await.expect_err("its project cannot come back");
        assert!(stores.sessions.get(&system, id).await.is_err(), "the session stays deleted");
        assert_eq!(
            groups_there().await,
            (false, false),
            "with its groups, restored before and deleted again"
        );
        assert!(stores.projects.get(&system, project).await.is_err(), "and its project");
        assert_eq!(
            stores.sessions.get_deleted(&system, id).await.unwrap().removal,
            mark,
            "marked as it was"
        );

        passing.store(usize::MAX, Ordering::SeqCst);
        manager.restore(&ctx, id).await.expect("restored when tried again");
        assert_eq!(groups_there().await, (true, true), "with its groups");
        assert!(stores.projects.get(&system, project).await.is_ok(), "its project");
        assert!(
            stores.component_bundles.get(&system, bundle.id).await.is_ok(),
            "and what lay in its scope"
        );
    }

    /// Issue 467: a session whose move into a project cannot be stored leaves the project as it
    /// was - not shared with its participants -, and lies where it lay.
    #[tokio::test]
    async fn a_session_that_cannot_be_put_into_a_project_leaves_it_unshared() {
        let (runtime, passing) = failing_runtime(Session::KIND);
        let stores = runtime.entity_stores().clone();
        let manager = Arc::new(SessionManager::new(runtime));
        let owner = UserId::new();
        let ctx =
            SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let session = create(&manager, &ctx, make_session("moving", owner)).await.unwrap();
        let own = session.session().project;
        let project = ProjectManager::new(Arc::clone(&manager))
            .create(&ctx, "report", Scope::UserScope { user_id: owner }, None)
            .await
            .unwrap();

        passing.store(0, Ordering::SeqCst);
        session
            .update_project(&ctx, Some(project.id))
            .await
            .expect_err("the session cannot be stored");
        assert_eq!(session.session().project, own, "it lies where it lay");
        let shared = stores
            .projects
            .get(&SecurityContext::system(), project.id)
            .await
            .unwrap()
            .security_attributes
            .acl;
        assert!(
            !shared
                .iter()
                .any(|entry| entry.subject == Subject::Group(GroupId::of_session(session.id()))),
            "the share is taken back: {shared:?}"
        );
    }

    #[tokio::test]
    async fn a_purged_session_takes_the_attachments_of_its_channels_along() {
        use crate::attachment::AttachmentError;
        use crate::testing::World;
        use bytes::Bytes;
        use futures::stream;

        let world = World::new().await;
        let session = world.shared_session(&[], &[]).await;
        let (alice, system) = (World::as_user(world.alice), SecurityContext::system());
        let main = world
            .manager
            .views(Viewer::new(alice.clone()))
            .session(session.id())
            .await
            .unwrap()
            .channel(session.channels()[0].id())
            .await
            .unwrap();
        let attachments = world.runtime.attachment_manager();
        let uploaded = attachments
            .create(
                &main,
                "a.txt",
                None,
                (1, Box::pin(stream::iter([Ok(Bytes::from_static(b"a"))]))),
            )
            .await
            .unwrap();

        world.manager.stop(&alice, session.id()).await.unwrap();
        world.manager.delete(&alice, session.id()).await.unwrap();
        assert!(
            attachments.content_of(&uploaded).await.is_ok(),
            "kept while the session can be restored"
        );
        world.manager.purge(&system, session.id()).await.unwrap();
        assert!(
            matches!(attachments.content_of(&uploaded).await, Err(AttachmentError::NotFound(_))),
            "gone with its channel's state"
        );
    }

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn what_lies_in_a_scope_goes_with_its_session_or_channel() {
        use caelumora_model::agent::{Personality, PersonalityId};
        use caelumora_model::user::UserId;

        let runtime = test_runtime();
        let personalities = Arc::clone(&runtime.entity_stores().personalities);
        let bundles = Arc::clone(&runtime.entity_stores().component_bundles);
        let agents = Arc::clone(&runtime.entity_stores().agents);
        let stores = runtime.entity_stores().clone();
        let manager = SessionManager::new(runtime);
        let (ctx, system) = (SecurityContext::default(), SecurityContext::system());
        let session = make_session("scoped", UserId::new());
        let id = session.id;
        let running = create(&manager, &ctx, session).await.unwrap();
        let side = running.create_channel(&ctx, spec("side", vec![])).await.unwrap().id();

        let personality = Personality {
            id: PersonalityId::new(),
            name: ScopedName::new("planner", Scope::SessionScope { session_id: id }).unwrap(),
            kind: Default::default(),
            roles: None,
            description: None,
            introduction: None,
            components: Default::default(),
            security_attributes: Default::default(),
            reasoning_visibility: Default::default(),
            hears_during_turn: Default::default(),
            deleted: None,
        };
        personalities.create(&system, &personality).await.unwrap();
        let bundle = ComponentBundle::new(
            ScopedName::new("tools", Scope::SessionScope { session_id: id }).unwrap(),
            Default::default(),
            Default::default(),
        );
        bundles.create(&system, &bundle).await.unwrap();
        let agent_in = |name: &str, scope| {
            Agent::new(ScopedName::new(name, scope).unwrap(), personality.id, ActorId::None)
        };
        let sessions = agent_in("helper", Scope::SessionScope { session_id: id });
        let sides = agent_in("aside", Scope::ChannelScope { session_id: id, channel_id: side });
        let users = agent_in("own", Scope::UserScope { user_id: UserId::new() });
        let earlier = agent_in("earlier", Scope::SessionScope { session_id: id });
        for agent in [&sessions, &sides, &users, &earlier] {
            agents.create(&system, agent).await.unwrap();
        }
        agents.delete(&system, earlier.id, &Removal::now(system.caller_id)).await.unwrap();
        let present = |agent: &Agent| {
            let (agents, id) = (Arc::clone(&agents), agent.id);
            async move { agents.get(&SecurityContext::system(), id).await.is_ok() }
        };

        running.delete_channel(&ctx, side).await.unwrap();
        assert!(!present(&sides).await, "the channel's agent goes with it");
        let mark = stores.channels.get_deleted(&system, side).await.unwrap().removal;
        let group =
            stores.groups.get_deleted(&system, GroupId::of_channel(side)).await.unwrap().removal;
        assert_eq!(
            (&agents.get_deleted(&system, sides.id).await.unwrap().removal, &group),
            (&mark, &mark),
            "with its mark (issue 469)"
        );
        assert!(
            present(&sessions).await && present(&users).await,
            "the session's and the user's stay"
        );
        running.restore_channel(&ctx, side).await.unwrap();
        assert!(present(&sides).await, "and comes back with it");

        manager.delete(&ctx, id).await.unwrap();
        assert!(
            !present(&sessions).await && !present(&sides).await,
            "the session's and its channels' go with it"
        );
        assert!(
            personalities.get(&system, personality.id).await.is_err(),
            "so do its personalities"
        );
        assert!(bundles.get(&system, bundle.id).await.is_err(), "and its component bundles");
        assert!(present(&users).await, "the user's stays");
        // One mark for all of it (issue 469): the session's, by whoever deleted it.
        let Deleted { entity: session, removal: mark } =
            stores.sessions.get_deleted(&system, id).await.unwrap();
        assert_eq!(mark.by, ctx.caller_id);
        let own = session.project;
        let marks = [
            agents.get_deleted(&system, sessions.id).await.unwrap().removal,
            agents.get_deleted(&system, sides.id).await.unwrap().removal,
            personalities.get_deleted(&system, personality.id).await.unwrap().removal,
            stores.groups.get_deleted(&system, GroupId::of_session(id)).await.unwrap().removal,
            stores.projects.get_deleted(&system, own).await.unwrap().removal,
        ];
        assert!(
            marks.iter().all(|marked| *marked == mark),
            "all with the session's mark: {marks:?}"
        );

        manager.restore(&ctx, id).await.unwrap();
        assert!(present(&sessions).await && present(&sides).await, "back with it");
        assert!(personalities.get(&system, personality.id).await.is_ok());
        assert!(bundles.get(&system, bundle.id).await.is_ok());
        assert!(!present(&earlier).await, "but not what was deleted before it");

        // A name taken meanwhile (issue 250): that one stays deleted, the
        // others come back.
        manager.delete(&ctx, id).await.unwrap();
        let taker = agent_in("Aside", Scope::ChannelScope { session_id: id, channel_id: side });
        agents.create(&system, &taker).await.unwrap();
        manager.restore(&ctx, id).await.unwrap();
        assert!(!present(&sides).await, "the one whose name is taken stays deleted");
        assert!(present(&sessions).await && present(&taker).await, "the others are back");

        manager.delete(&ctx, id).await.unwrap();
        manager.purge(&ctx, id).await.unwrap();
        for agent in [&sessions, &sides, &earlier, &taker] {
            assert!(
                agents.get_deleted(&system, agent.id).await.is_err(),
                "{} is purged with it",
                agent.name
            );
        }
        assert!(personalities.get_deleted(&system, personality.id).await.is_err());
        assert!(present(&users).await, "the user's is not");
    }

    #[tokio::test]
    async fn a_running_session_is_no_way_around_the_policy() {
        use caelumora_model::user::UserId;

        let (_, manager) = owned_runtime(Default::default());

        let as_user =
            |id| SecurityContext::new(ActorId::User(id), Default::default(), Default::default());
        let (alice, bob, carol) = (UserId::new(), UserId::new(), UserId::new());
        let mut session = make_session("hers", alice);
        session.security_attributes = session
            .security_attributes
            .clone()
            .granting(Subject::Actor(ActorId::User(bob)), Grant::Read);
        let id = session.id;
        create(&manager, &as_user(alice), session).await.expect("alice creates her own session");

        let refused = manager.open(&as_user(carol), id).await.expect_err("carol may not see it");
        assert!(
            matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::NotFound(_))),
            "not refused - absent: {refused}"
        );
        manager.open(&as_user(bob), id).await.expect("a contributor opens it");

        let refused =
            manager.delete(&as_user(bob), id).await.expect_err("a contributor may not delete it");
        assert!(is_forbidden(&refused), "{refused}");
        assert!(
            manager.sessions.contains_key(&id),
            "a refused deletion must not stop the session on the way"
        );

        let refused = manager.stop(&as_user(carol), id).await.expect_err("carol may not see it");
        assert!(
            matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::NotFound(_))),
            "not refused - absent: {refused}"
        );
        let refused =
            manager.stop(&as_user(bob), id).await.expect_err("a contributor may not stop it");
        assert!(is_forbidden(&refused), "{refused}");
        assert!(manager.sessions.contains_key(&id), "it runs on");
        assert!(manager.stop(&as_user(alice), id).await.unwrap(), "the owner stops it");
        assert!(!manager.stop(&as_user(alice), id).await.unwrap(), "stopped already");

        manager.delete(&as_user(alice), id).await.expect("the owner deletes it");
        assert!(!manager.sessions.contains_key(&id));
    }

    /// A running session of alice and an agent of hers that hangs in its
    /// answer to her hello (`Script::Hangs`): the manager, alice's context,
    /// the session, the agent and what its channel takes in from the start
    /// of the answer on.
    async fn with_a_hanging_answer(
    ) -> (Arc<SessionManager>, SecurityContext, Arc<SessionLoop>, ActorId, Receiver<Envelope>) {
        use crate::testing::{Script, ScriptedAgents};
        use caelumora_model::agent::{Personality, PersonalityId, PersonalityKind};

        let store = Persistence::in_memory();
        let runtime = TestRuntime {
            store: store.clone(),
            plugins: vec![&ScriptedAgents(Script::Hangs)],
            ..Default::default()
        }
        .build();
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let system = SecurityContext::system();
        let alice = user("alice");
        let participants = record_users(&manager, &[&alice]).await;
        let personality = Personality {
            id: PersonalityId::new(),
            name: ScopedName::new("thinker", Default::default()).unwrap(),
            kind: PersonalityKind::Internal,
            roles: None,
            description: None,
            introduction: None,
            components: Default::default(),
            security_attributes: Default::default(),
            reasoning_visibility: Default::default(),
            hears_during_turn: Default::default(),
            deleted: None,
        };
        store.entity_stores().unwrap().personalities.create(&system, &personality).await.unwrap();
        let thinker = Agent::new(
            ScopedName::new("thinker", Scope::GlobalScope).unwrap(),
            personality.id,
            ActorId::User(alice.id),
        );
        store.entity_stores().unwrap().agents.create(&system, &thinker).await.unwrap();
        let agent = ActorId::Agent(thinker.id);

        let alice_ctx =
            SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let mut session = make_session("thinking", UserId::new());
        session.participants = participants;
        session.participants.push(session::Participant::Agent(thinker.id));
        let session = create(&manager, &alice_ctx, session).await.unwrap();
        let channel = session
            .create_channel(&alice_ctx, spec("talk", vec![ActorId::User(alice.id), agent]))
            .await
            .unwrap();
        let mut events = channel.observe();
        channel
            .user_actor(alice.id)
            .unwrap()
            .send_message(Some("hello".to_string()), None, Vec::new())
            .await
            .unwrap();
        time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == agent
                    && matches!(envelope.event, ActorEvent::ResponseStarted { .. })
                {
                    break;
                }
            }
        })
        .await
        .expect("the agent begins its answer");
        (manager, alice_ctx, session, agent, events)
    }

    /// How the turns of `agent` ended among `events`, as far as they came.
    fn turns_given_up(events: &mut Receiver<Envelope>, agent: ActorId) -> Vec<GivenUpReason> {
        let mut given_up = Vec::new();
        while let Ok(envelope) = events.try_recv() {
            if let (true, ActorEvent::TurnGivenUp { reason, .. }) =
                (envelope.context.sender_id == agent, envelope.event)
            {
                given_up.push(reason);
            }
        }
        given_up
    }

    /// A session stopped (issue 377) has its actors stop, not leave: the
    /// answer under way is kept, not given up - and the stop waits for them
    /// to end by themselves, which takes less than the grace.
    #[tokio::test]
    async fn stopping_a_session_lets_its_agents_keep_their_turns() {
        let (manager, alice, session, agent, mut events) = with_a_hanging_answer().await;

        let stopping = Instant::now();
        assert!(manager.stop(&alice, session.id()).await.unwrap(), "it ran");
        assert!(stopping.elapsed() < STOP_GRACE, "the actors ended by themselves");
        assert_eq!(
            turns_given_up(&mut events, agent),
            Vec::<GivenUpReason>::new(),
            "nothing is given up"
        );
    }

    /// Shutting down (issue 297) stops every running session as a stop
    /// does - the answer under way is kept, within the bound - and starts
    /// none afterwards, neither one stored nor a new one.
    #[tokio::test]
    async fn a_shutdown_stops_every_session_keeping_its_turns_and_starts_none_after() {
        let (manager, alice, session, agent, mut events) = with_a_hanging_answer().await;
        let stored =
            create(&manager, &alice, make_session("idle", alice.caller_id.as_user_id().unwrap()))
                .await
                .unwrap()
                .id();
        manager.stop(&alice, stored).await.unwrap();
        let other =
            create(&manager, &alice, make_session("other", UserId::new())).await.unwrap().id();

        let shutting = Instant::now();
        manager.shutdown().await;
        assert!(shutting.elapsed() < SHUTDOWN_BOUND, "the actors ended by themselves");
        assert!(
            manager.running(session.id()).is_none() && manager.running(other).is_none(),
            "every session stopped"
        );
        assert_eq!(
            turns_given_up(&mut events, agent),
            Vec::<GivenUpReason>::new(),
            "nothing is given up"
        );

        let refused = |result: Result<Arc<SessionLoop>>| {
            matches!(
                result.err().and_then(|e| e.downcast::<SessionError>().ok()),
                Some(SessionError::ShuttingDown)
            )
        };
        assert!(refused(manager.open(&alice, stored).await), "a stored one is not started");
        assert!(refused(manager.open(&alice, session.id()).await), "nor one that ran");
        assert!(
            refused(create(&manager, &alice, make_session("new", UserId::new())).await),
            "nor a new one"
        );
    }

    /// After a restart (issue 388) the runtime starts again the sessions in
    /// which an agent's stored state has work under way, each once however
    /// many agents have, and no other: not one whose agents wait for a
    /// person or idle, not one deleted, nor what a channel that is gone
    /// leaves in the index. Once shut down, none.
    #[tokio::test]
    async fn a_restart_starts_again_the_sessions_with_work_under_way_and_no_other() {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let ctx = SecurityContext::default();
        let sessions = ["interrupted", "waiting", "idle", "deleted"]
            .map(|name| make_session(name, UserId::new()));
        for session in &sessions {
            persist(&manager, &ctx, session.clone(), &[main_channel(session)]).await;
        }
        let [interrupted, waiting, idle, deleted] =
            sessions.map(|session| (session.id, session.channels[0]));
        let in_turn = |processing_state| AgentChannelState {
            processing_state,
            turn: Some(TurnState::default()),
            ..AgentChannelState::default()
        };
        let states = runtime.state_store();
        for agent in [AgentId::new(), AgentId::new()] {
            states
                .put_agent_channel_state(agent, interrupted.1, &mut in_turn(ActorState::Planning))
                .await
                .unwrap();
        }
        states
            .put_agent_channel_state(
                AgentId::new(),
                waiting.1,
                &mut in_turn(ActorState::WaitingForUser),
            )
            .await
            .unwrap();
        states
            .put_agent_channel_state(AgentId::new(), idle.1, &mut AgentChannelState::default())
            .await
            .unwrap();
        states
            .put_agent_channel_state(
                AgentId::new(),
                deleted.1,
                &mut in_turn(ActorState::ExecutingTools),
            )
            .await
            .unwrap();
        states
            .put_agent_channel_state(
                AgentId::new(),
                ChannelId::new(),
                &mut in_turn(ActorState::Planning),
            )
            .await
            .unwrap();
        assert!(
            manager.delete(&ctx, deleted.0).await.unwrap(),
            "deleted while it does not run, its state kept"
        );

        assert_eq!(
            manager.resume_interrupted().await,
            1,
            "one session, though two of its agents had work under way"
        );
        assert!(
            manager.running(interrupted.0).is_some(),
            "the session with work under way runs again"
        );
        for (id, _) in [waiting, idle, deleted] {
            assert!(manager.running(id).is_none(), "no other");
        }

        manager.shutdown().await;
        assert_eq!(manager.resume_interrupted().await, 0, "none once shut down");
    }

    /// A session deleted (issue 377) has its actors leave, as a deleted
    /// channel does: the answer under way is given up, and the channel told
    /// before the session's task ends.
    #[tokio::test]
    async fn deleting_a_session_has_its_agents_leave_and_give_their_turns_up() {
        let (manager, alice, session, agent, mut events) = with_a_hanging_answer().await;

        assert!(manager.delete(&alice, session.id()).await.unwrap());
        assert_eq!(turns_given_up(&mut events, agent), [GivenUpReason::Left]);
    }
}
