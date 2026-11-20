//! `SessionManager`: the sessions of the process - those that run, and the building blocks that
//! create, start, stop, delete, restore and purge them, which the procedures of a session carry
//! out (`procedure::session`, issue 593).

use super::{
    load_own, make_own, purge_channel_document, purge_own, require_attachable,
    require_classification, SessionError, SessionLoop, SessionSpec,
};
use crate::agent::AgentManager;
use crate::attachment::AttachmentManager;
use crate::controller::ControllerManager;
use crate::event::EventBus;
use crate::group::{keep_participants, GroupManager};
use crate::kernel::Kernel;
#[cfg(any(test, feature = "test-support"))]
use crate::procedure::participant::{take_in, Entry};
use crate::procedure::session::move_to;
use crate::procedure::{Admitted, Compensation};
use crate::scoped::{purge_for_good, ScopeNotEmpty, ScopedEntities};
use crate::template::{
    resolve_participants, resolve_workspace, take_in_participants, TemplateParticipants,
};
use crate::view::{Viewer, Views};
use crate::workspace::reaper::CommandReaper;
use crate::workspace::WorkspaceManager;
use anyhow::{Error, Result};
#[cfg(any(test, feature = "test-support"))]
use caelumora_domain::channel::Participant;
use caelumora_domain::channel::{Channel, ChannelField};
#[cfg(any(test, feature = "test-support"))]
use caelumora_domain::channel::{ChannelId, HistoryAccess, ThreadId};
use caelumora_domain::event::{Announcing, GlobalEvent};
use caelumora_domain::group::{Group, GroupId};
use caelumora_domain::project::{Project, ProjectId};
use caelumora_domain::query::{Filter, Query};
use caelumora_domain::removal::Removal;
use caelumora_domain::scope::{Scope, ScopeField};
use caelumora_domain::security::{SecurityAttributes, Subject};
use caelumora_domain::session;
use caelumora_domain::session::{Session, SessionId};
use caelumora_domain::store::{StoreError, StoreResult, StoreResultExt, Stored};
use caelumora_domain::template::{
    ScopeTemplate, SessionTemplate, SessionTemplateId, WorkspaceTemplate,
};
use caelumora_domain::user::UserId;
use caelumora_domain::value::language::Language;
use caelumora_domain::value::name::Name;
use caelumora_domain::value::time_zone::TimeZone;
use caelumora_domain::workspace::WorkspaceChoice;
use caelumora_security::{
    AccessQuestions, ActionType, Enforcement, Policies, SecurityContext, JOIN,
};
use caelumora_store::sessions::{SessionHold, SessionLocks};
use caelumora_store::{purge_behind, Deleted, EntityStores, StateStore};
use dashmap::DashMap;
use std::collections::HashSet;
use std::iter;
#[cfg(any(test, feature = "test-support"))]
use std::mem;
use std::ops::AsyncFnOnce;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

pub struct SessionManager {
    sessions: DashMap<SessionId, Arc<SessionLoop>>,
    handles: DashMap<SessionId, JoinHandle<()>>,
    /// What holds a session for this process against the others that share the stores (issue
    /// 554), if any do; and the holds of the sessions that run here, let go as they stop.
    locks: Option<Arc<dyn SessionLocks>>,
    holds: DashMap<SessionId, SessionHold>,
    /// What builds the loops of the sessions it starts and the views of them, and what is made
    /// beside it runs in (`kernel`).
    kernel: Arc<Kernel>,
    /// The services it works with, taken from `kernel` once (issue
    /// 229).
    policies: Policies,
    stores: EntityStores,
    state_store: Arc<dyn StateStore>,
    events: Arc<EventBus>,
    controllers: Arc<ControllerManager>,
    /// What the workspaces of the sessions it makes are found or made by.
    workspaces: Arc<WorkspaceManager>,
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
            Found::Running(session_loop) => (*session_loop.session()).clone(),
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
    /// The session manager of `kernel`, which knows it from then on (`Kernel::sessions`).
    pub fn new(kernel: Arc<Kernel>) -> Arc<Self> {
        let manager = Arc::new(Self {
            sessions: DashMap::new(),
            handles: DashMap::new(),
            locks: kernel.session_locks().map(Arc::clone),
            holds: DashMap::new(),
            policies: kernel.policies().clone(),
            stores: kernel.entity_stores().clone(),
            state_store: Arc::clone(kernel.state_store()),
            events: Arc::clone(kernel.event_bus()),
            controllers: Arc::clone(kernel.controller_manager()),
            workspaces: Arc::clone(kernel.workspace_manager()),
            command_reaper: Arc::clone(kernel.command_reaper()),
            agents: Arc::clone(kernel.agent_manager()),
            scoped: Arc::clone(kernel.scoped_entities()),
            groups: Arc::clone(kernel.group_manager()),
            attachments: Arc::clone(kernel.attachment_manager()),
            kernel,
        });
        manager.kernel.know_sessions(&manager);
        manager
    }

    /// Build the runtime loop for an (already-persisted) session, register it, spawn its
    /// run task, and announce that the session has started running. For a procedure that holds
    /// the session exclusively, with `hold`, the session held for this process where others share
    /// the stores (`hold`, issue 554), which it keeps as long as the session runs.
    async fn spawn_session(
        &self,
        _: &Admitted<'_>,
        session: Session,
        channels: Vec<Channel>,
        hold: Option<SessionHold>,
    ) -> Result<Arc<SessionLoop>> {
        let id = session.id;
        let session_loop = SessionLoop::new(session, channels, Arc::clone(&self.kernel)).await?;
        self.sessions.insert(id, Arc::clone(&session_loop));
        if let Some(hold) = hold {
            self.holds.insert(id, hold);
        }

        let loop_for_task = Arc::clone(&session_loop);
        let handle = tokio::spawn(async move {
            let Err(e) = loop_for_task.run().await;
            tracing::error!("Session loop for {} failed: {}", id, e);
        });
        self.handles.insert(id, handle);

        self.events.announce(
            GlobalEvent::custom::<Session>(&id, session::STARTED, &()),
            &*session_loop.session(),
        );
        Ok(session_loop)
    }

    /// Session `id` held for this process against the others that share the stores (issue 554),
    /// for what a procedure that holds it does with it: `None` where it needs no hold of its own -
    /// it runs here, held as long as it runs, or no other process shares the stores.
    ///
    /// # Errors
    /// `SessionError::RunsElsewhere` if another process holds it; whatever asking reports.
    pub(crate) fn hold(&self, id: SessionId) -> Result<Option<SessionHold>> {
        let Some(locks) = &self.locks else { return Ok(None) };
        if self.holds.contains_key(&id) {
            return Ok(None);
        }
        match locks.try_hold(id)? {
            Some(hold) => Ok(Some(hold)),
            None => Err(SessionError::RunsElsewhere(id).into()),
        }
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
                self.policies
                    .access_policy()
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

    /// Creates session `id` from the template `template_id` as `spec` asks, on behalf of `context`,
    /// whose user owns it and is the template's "user", and starts it (issue 88), for
    /// `procedure::session::Create`, which holds it until it is complete and rolls its creation
    /// back if any of it fails. One of the user's own workspaces that `spec` chooses is attached as
    /// the session is made; the rest - a workspace to be found or made, the agents, the share of
    /// the chosen workspace, the project it goes to - is done once it is made (`complete`), as
    /// `context`.
    ///
    /// # Errors
    /// `SessionError::OwnerNotAUser` if `context` acts for no user; a `StoreError` if the template,
    /// a user or a personality it names does not exist or may not be read; a `ValueError` if the
    /// name `spec` asks for is no name, its language no language tag or its time zone no time zone;
    /// a `TemplateError` if the template cannot be made into a session; what attaching an existing
    /// workspace, making one or sharing one reports; `SessionError::ChannelUnavailable` if the
    /// components of one of its channels cannot be read in the owner's name (issue 463); what
    /// putting it into the project reports (`procedure::session::move_to`); whatever creating the
    /// session reports.
    pub(crate) async fn make_from_template(
        &self,
        admitted: &Admitted<'_>,
        log: &mut Compensation,
        context: &SecurityContext,
        id: SessionId,
        template_id: SessionTemplateId,
        spec: SessionSpec,
    ) -> Result<Arc<SessionLoop>> {
        let owner = context
            .principal_id
            .as_user_id()
            .ok_or(SessionError::OwnerNotAUser(context.principal_id))?;
        // Before anything is found or made for it.
        let name = Name::optional(spec.name.as_deref())?;
        let language = Language::optional(spec.language.as_deref())?;
        let time_zone = TimeZone::optional(spec.time_zone.as_deref())?;
        let template =
            self.stores.session_templates.get(context, template_id).await.map_err(|e| match e {
                StoreError::NotFound { .. } => SessionError::TemplateNotFound(template_id).into(),
                e => Error::new(e),
            })?;
        // Its own verb before anything of others is asked or anything is
        // found or made for it (issue 312): who may not create the session
        // learns nothing of the agents, the workspace or anyone's clearance.
        self.policies
            .access_policy()
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
        let existing = match &spec.workspace {
            Some(WorkspaceChoice::Existing { id: chosen, .. }) => {
                require_attachable(&self.workspaces, context, *chosen, id).await?;
                Some(*chosen)
            }
            _ => None,
        };
        let (users, agents) =
            resolve_participants(&self.stores, context, &template, id, owner).await?;
        let name = name.unwrap_or_else(|| template.name.name.clone());
        let (mut session, own, channels) =
            template.build(id, owner, name, language, time_zone, users.clone())?;
        // One chosen is attached as the session is made; one to be found or
        // made once the session is, so that one of its scope is made where
        // `context` may put it (issue 344).
        session.workspace = existing;
        let session_loop = self.establish(admitted, context, session, own, channels).await?;
        // A channel whose components the owner cannot read is left out as a
        // session opens (issue 463), but making it is the owner's own request,
        // refused as creating a channel refuses it (`run_channel_held`).
        session_loop.require_running()?;
        let session = &session_loop;
        let participants = (users, agents);
        self.complete(admitted, log, context, owner, &template, session, &spec, participants)
            .await?;
        Ok(session_loop)
    }

    /// Completes `session` of `owner`, just made from `template` as `spec` asks on behalf of
    /// `context`: the workspace to be found or made - one `spec` chooses to be made for it alone,
    /// else the template's - found or made and attached, then the template's agents found or made
    /// and taken in (issues 227, 344), and its users and agents - `participants`, as
    /// `resolve_participants` gives them - taken into the channels that name them as their founders
    /// (issue 420); then one of the user's own workspaces that `spec` chooses shared with its
    /// participants as far as it says (issue 23), and the session put into the shared project
    /// `spec` names by the one way into a project (`procedure::session::move_to`, issue 467). A
    /// workspace made for it that is not in its scope - which goes with the session as its creation
    /// is rolled back - is deleted here if any of that fails.
    ///
    /// # Errors
    /// What finding or making the workspace or the agents reports, or taking them in, sharing the
    /// workspace or putting the session into the project.
    #[expect(
        clippy::too_many_arguments,
        reason = "the proof of admission came to the steps of a creation, issue 593"
    )]
    async fn complete(
        &self,
        admitted: &Admitted<'_>,
        log: &mut Compensation,
        context: &SecurityContext,
        owner: UserId,
        template: &SessionTemplate,
        session: &Arc<SessionLoop>,
        spec: &SessionSpec,
        participants: TemplateParticipants,
    ) -> Result<()> {
        // What is to be found or made: the template's, unless one was chosen.
        let wanted = match &spec.workspace {
            None => template.workspace.clone(),
            Some(WorkspaceChoice::New { storage, classification }) => Some(WorkspaceTemplate {
                name: session.name(),
                scope: ScopeTemplate::SessionScope,
                storage: storage.clone(),
                classification: classification.clone(),
                description: None,
            }),
            Some(WorkspaceChoice::None | WorkspaceChoice::Existing { .. }) => None,
        };
        let resolved = match &wanted {
            Some(wanted) => Some(
                resolve_workspace(&self.workspaces, context, wanted, session.id(), owner).await?,
            ),
            None => None,
        };
        let made = resolved.and_then(|(workspace, made)| made.then_some(workspace));
        let completed = async {
            if let Some((workspace, _)) = resolved {
                session.attach_workspace(context, Some(workspace)).await?;
            }
            let agents = &self.agents;
            take_in_participants(admitted, log, agents, context, template, session, participants)
                .await?;
            // Shared once the session it is shared with exists (issue 23).
            if let Some(WorkspaceChoice::Existing { id, share: Some(grant) }) = &spec.workspace {
                let controller = self.controllers.workspace_controller(*id).await?;
                let group = Subject::Group(GroupId::of_session(session.id()));
                controller.share(context, group, *grant).await?;
            }
            if let Some(project) = spec.project {
                move_to(admitted, log, session, context, Some(project)).await?;
            }
            Ok(())
        }
        .await;
        if let (Err(_), Some(made)) = (&completed, made) {
            let system = SecurityContext::system();
            if let Err(cleanup) = async {
                self.workspaces.delete(&system, made).await?;
                self.workspaces.purge(&system, made).await
            }
            .await
            {
                warn!(workspace = %made, "The workspace made for a session that could not be created stays: {cleanup}");
            }
        }
        completed
    }

    /// Establishes the brand-new `session` with its own project `own` and its `channels`, whose
    /// creation `context` was asked for (`CreateSession`) before anything was found or made for
    /// it: persists them (`persist`), announces their existence, then starts the session. The
    /// channels are empty (issue 420): their founders join once the session runs, each admitted as
    /// they join (`procedure::participant::take_in`).
    ///
    /// # Errors
    /// As `persist`; whatever starting the session reports (`spawn_session`).
    async fn establish(
        &self,
        admitted: &Admitted<'_>,
        context: &SecurityContext,
        mut session: Session,
        mut own: Project,
        mut channels: Vec<Channel>,
    ) -> Result<Arc<SessionLoop>> {
        let id = session.id;
        // Held before anything is stored, so that what cannot hold it leaves nothing behind.
        let hold = self.hold(id)?;
        self.persist(context, &mut session, &mut own, &mut channels).await?;

        let event_bus = &self.events;
        event_bus.announce(GlobalEvent::created::<Session>(&id), &session);
        event_bus.announce(GlobalEvent::created::<Project>(&own.id), &own);
        for channel in &channels {
            event_bus.announce(GlobalEvent::created::<Channel>(&(id, channel.id)), channel);
        }

        self.spawn_session(admitted, session, channels, hold).await
    }

    /// Stores the brand-new `session` with its own project `own` and its `channels`, whose creation
    /// `context` was asked for (`CreateSession`): the channels as documents of their own (issue
    /// 70), the session and each channel with the group of its participants (issue 286), which it
    /// is read by, and the project in the kernel's name, as the groups are (issue 467). The
    /// classifications are validated first; the store asks to create the session again and each
    /// channel as it stores them. Nothing half-made stays behind: if a step fails once the session
    /// is stored, what was stored is purged again, in the kernel's name.
    ///
    /// # Errors
    /// `SessionError::ClassificationRefused` for a classification no channel may be given;
    /// whatever the store reports.
    pub(super) async fn persist(
        &self,
        context: &SecurityContext,
        session: &mut Session,
        own: &mut Project,
        channels: &mut [Channel],
    ) -> Result<()> {
        for channel in channels.iter() {
            require_classification(
                self.policies.confidentiality().as_ref(),
                &channel.classification,
            )?;
        }
        self.stores.sessions.create(context, session).await?;
        let made = async {
            keep_participants(self.stores.groups.as_ref(), Group::of_session(session)).await?;
            make_own(&self.stores, own).await?;
            for channel in channels.iter_mut() {
                self.stores.channels.create(context, channel).await?;
                keep_participants(self.stores.groups.as_ref(), Group::of_channel(channel)).await?;
            }
            Ok::<_, StoreError>(())
        }
        .await;
        if let Err(error) = made {
            // The session's creation was permitted, so it is rolled back in the kernel's name -
            // what was not made yet is not found, which is no matter.
            for channel in channels.iter() {
                let _ = purge_channel_document(&self.stores, channel.id).await;
            }
            let _ = purge_own(&self.stores, own.id).await;
            let _ =
                purge_for_good(self.stores.groups.as_ref(), GroupId::of_session(session.id)).await;
            let _ = purge_for_good(self.stores.sessions.as_ref(), session.id).await;
            return Err(error.into());
        }
        Ok(())
    }

    /// The channels `session` lists, loaded in the kernel's name: starting a session the caller may
    /// read follows from an action already permitted, and it must not fail because the caller is
    /// not in one of its channels.
    async fn load_channels(&self, session: &Session) -> Result<Vec<Channel>> {
        let system = SecurityContext::system();
        let mut channels = Vec::with_capacity(session.channels.len());
        for id in &session.channels {
            channels.push(self.stores.channels.get(&system, *id).await?);
        }
        Ok(channels)
    }

    /// Session `id`, to take someone into it on behalf of `context`, for a procedure that holds it
    /// as `open_held` says: opened in the kernel's name if an entry of its access list lets
    /// `context` join it, or they may read it - who may join reads nothing of it before they did.
    ///
    /// # Errors
    /// `SessionError::NotFound` if there is no such session or `context` may neither join nor read
    /// it; as `open_held` otherwise.
    pub(crate) async fn open_to_take_in(
        &self,
        admitted: &Admitted<'_>,
        context: &SecurityContext,
        id: SessionId,
    ) -> Result<Arc<SessionLoop>> {
        let not_found = || SessionError::NotFound(id);
        let system = SecurityContext::system();
        let attributes = self.find(&system, id).await.map_err(|_| not_found())?.attributes();
        let reachable = self
            .policies
            .access_policy()
            .check(context, ActionType::custom(Session::KIND, &JOIN, &attributes))
            .await
            || self.policies.access_policy().may_read_session(context, &attributes).await;
        if !reachable {
            return Err(not_found().into());
        }
        self.open_held(admitted, &system, id).await
    }

    /// Session `id` for `context`, for a procedure that holds it - exclusively where it does not
    /// run: the running one - whose channels left out as it started whoever opens it has built
    /// again (`procedure::channel::Start`, issue 463) -, or loaded from the store and started -
    /// which announces it as `session::STARTED`, not as created, as the session exists already.
    ///
    /// # Errors
    /// `SessionError::NotFound` if there is no such session or `context` may not read it,
    /// `SessionError::RunsElsewhere` if another process that shares the stores runs it (issue
    /// 554); whatever loading or starting it reports otherwise.
    pub(crate) async fn open_held(
        &self,
        admitted: &Admitted<'_>,
        context: &SecurityContext,
        id: SessionId,
    ) -> Result<Arc<SessionLoop>> {
        match self.find(context, id).await? {
            Found::Running(running) => Ok(running),
            Found::Stored(session) => {
                let hold = self.hold(id)?;
                let channels = self.load_channels(&session).await?;
                let started = self.spawn_session(admitted, session, channels, hold).await?;
                // An agent deleted or restored while its channels were built: the follower missed
                // it, as the session ran nowhere yet (`procedure::participant::Follow`).
                started.follow_agents_held(admitted, None).await;
                Ok(started)
            }
        }
    }

    /// Stops session `id` on behalf of `context` without deleting it, for a procedure that holds
    /// it exclusively, so that a start under way is not missed (issue 62): its channels and actors
    /// stop, for everybody in it (`StopSession`) - each actor keeps what it is in the middle of, to
    /// be resumed when the session runs again, and is given up to `STOP_GRACE` to store it (issue
    /// 377) -, and the commands of its channels end (issue 254). Answers whether it ran.
    ///
    /// # Errors
    /// `SessionError::NotFound` if there is no such session or `context` may not read it,
    /// `StoreError::PermissionDenied` if they may read but not stop it - whether it runs or not.
    pub(crate) async fn stop_held(
        &self,
        admitted: &Admitted<'_>,
        context: &SecurityContext,
        id: SessionId,
    ) -> Result<bool> {
        let found = self.find(context, id).await?;
        let attributes = found.attributes();
        self.policies
            .access_policy()
            .require(context, ActionType::StopSession(&attributes), StoreError::PermissionDenied)
            .await?;
        if !matches!(found, Found::Running(_)) {
            return Ok(false);
        }
        let Some(stopped) = self.stop_running(admitted, id, SessionLoop::stop_actors).await else {
            return Ok(false);
        };
        self.end_commands_of(&stopped).await;
        Ok(true)
    }

    /// The project session `id` lies in - running, stored or deleted -, read in the kernel's name,
    /// for a procedure to lock as it enumerates; `None` if there is no such session, or it cannot
    /// be read.
    pub(crate) async fn project_of(&self, id: SessionId) -> Option<ProjectId> {
        if let Some(running) = self.running(id) {
            return Some(running.session().project);
        }
        let system = SecurityContext::system();
        match self.stores.sessions.get(&system, id).await {
            Ok(session) => Some(session.project),
            Err(_) => {
                let deleted = self.stores.sessions.get_deleted(&system, id).await;
                deleted.ok().map(|deleted| deleted.entity.project)
            }
        }
    }

    /// The sessions in which an agent's work was under way when the process last ended (issue
    /// 388), as the state store's index of the work under way lists them
    /// (`StateStore::channels_with_work_under_way`), read in the kernel's name; a channel that is
    /// gone is passed over.
    pub(crate) async fn interrupted(&self) -> Vec<SessionId> {
        let channels = match self.state_store.channels_with_work_under_way().await {
            Ok(channels) => channels,
            Err(error) => {
                warn!("The work under way at the last end cannot be read, nothing is started again: {error}");
                return Vec::new();
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
        sessions.into_iter().collect()
    }

    /// The sessions that run.
    pub(crate) fn running_ids(&self) -> Vec<SessionId> {
        self.sessions.iter().map(|entry| *entry.key()).collect()
    }

    /// Ends, of the commands of every session, what does not outlive the process, as the sessions
    /// are shut down (`CommandReaper::shutdown`), for the procedure that holds them: an Open
    /// Terminal instance keeps its commands, unlike at a stop.
    pub(crate) async fn shut_down_commands(&self, _: &Admitted<'_>) {
        self.command_reaper.shutdown().await;
    }

    /// Ends every command started in a channel of `stopped`, with what it
    /// started: what stops a session ends what runs in its channels (issue
    /// 254).
    async fn end_commands_of(&self, stopped: &SessionLoop) {
        self.command_reaper.end_commands_in(&stopped.session().channels).await;
    }

    /// Stops session `id` if it runs, for a procedure that holds it exclusively. Its actors are
    /// ended first by `end_actors`, told to stop (`SessionLoop::stop_actors`) or to leave
    /// (`SessionLoop::leave_actors`), and given `STOP_GRACE` to end by themselves before the
    /// session's task, and with it whoever did not, is aborted (issue 377); once it has ended, and
    /// every channel's task is aborted with it, every channel it started is announced stopped, then
    /// the session (issue 593). Nobody finds it running meanwhile.
    pub(crate) async fn stop_running(
        &self,
        _: &Admitted<'_>,
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
            // Ended, so that no channel starts after it is announced stopped.
            let _ = handle.await;
        }
        self.holds.remove(&id);
        if let Some(session_loop) = &removed {
            session_loop.announce_channels_stopped();
            let session = session_loop.session();
            self.events
                .announce(GlobalEvent::custom::<Session>(&id, session::STOPPED, &()), &*session);
        }
        removed
    }

    /// Session `id`, to be deleted on behalf of `context` by a procedure that holds it
    /// exclusively until it is gone from the store, so that nothing starts it again from there
    /// meanwhile (issue 62): with the channels it lists and its own project (issue 467), as they
    /// are before it goes - the events carry the attributes as they were, as once deleted it is
    /// nowhere to look them up (issue 39). Asked before anything happens: a refused deletion must
    /// not stop the session on the way (issue 11). `None` if there is no such session or `context`
    /// may not read it, as every deletion answers (issue 510).
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if they may read but not delete it; whatever the store
    /// reports.
    pub(crate) async fn session_to_delete(
        &self,
        context: &SecurityContext,
        id: SessionId,
    ) -> Result<Option<(Session, Vec<Channel>, Option<Project>)>> {
        let session = match self.find(context, id).await {
            Ok(found) => found.session(),
            Err(error) if matches!(error.downcast_ref(), Some(SessionError::NotFound(_))) => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        self.policies
            .access_policy()
            .require(
                context,
                ActionType::delete(Session::KIND, &session.security_attributes),
                StoreError::PermissionDenied,
            )
            .await?;
        let channels = self.load_channels(&session).await?;
        let own = load_own(&self.stores, id).await?;
        Ok(Some((session, channels, own)))
    }

    /// The deleted session `id`, to be restored on behalf of `context` by a procedure that holds
    /// it exclusively, as deleting and purging it do: a purge never meets it half restored. Its
    /// mark - what was deleted with it carries it and comes back with it (issues 178, 469) -, with
    /// its own project deleted with it. A session that lies in a shared project is not restored
    /// while the project is deleted: it comes back with the project.
    ///
    /// # Errors
    /// `SessionError::NotFound` if there is no such deleted session, `context` may not see it
    /// (`ReadDeleted`, asked first, issue 304) or its purge is begun (issue 496),
    /// `SessionError::ProjectDeleted` if the shared project it lies in is deleted; whatever else
    /// the store reports.
    pub(crate) async fn session_to_restore(
        &self,
        context: &SecurityContext,
        id: SessionId,
    ) -> Result<(Removal, Option<Project>)> {
        // As `context`, so that whoever may not see it learns nothing more.
        let Deleted { entity: deleted, removal } =
            self.stores.sessions.get_deleted(context, id).await.map_err(store_error(id))?;
        // One whose purge is begun is the store's to refuse, as it would; refused before anything
        // else is asked, so that its own project, purged already, is not taken for a shared one
        // deleted.
        if deleted.purging() {
            return Err(SessionError::NotFound(id).into());
        }
        let own = self
            .stores
            .projects
            .list_deleted_with(&SecurityContext::system(), removal.id, &lying_in(id))
            .await?
            .items
            .pop();
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
        Ok((removal, own))
    }

    /// Marks session `id` deleted on behalf of `context`, with `removal` - its own, or the mark of
    /// a deletion it goes with: of the project it lies in (issue 469) -, for a procedure that holds
    /// it exclusively; as it is deleted, or its restoring undone in the kernel's name. Marked, not
    /// removed (issue 164): its channels and their state stay as they are, hidden with it - a
    /// channel is reached only through its session - until it is restored or purged. Marked before
    /// what lies in its scope, so that nothing of it goes unless its deletion was granted, and
    /// nothing is put into its scope afterwards (`ScopeHolders`, issue 458). `false` if the store
    /// finds nothing of it to mark.
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::PermissionDenied` among it.
    pub(crate) async fn mark_session_deleted_held(
        &self,
        _: &Admitted<'_>,
        context: &SecurityContext,
        id: SessionId,
        removal: &Removal,
    ) -> Result<bool> {
        Ok(self.stores.sessions.delete(context, id, removal).await?)
    }

    /// Takes the mark of the deleted session `id` away on behalf of `context`, for a procedure that
    /// holds it exclusively: as it is restored, or its deletion undone in the kernel's name. It is
    /// not started; opening it starts it, as any stored session.
    ///
    /// # Errors
    /// `SessionError::NotFound` if there is no such deleted session or `context` may not see it,
    /// `StoreError::PermissionDenied` if they may see but not restore it; whatever else the store
    /// reports.
    pub(crate) async fn unmark_session_held(
        &self,
        _: &Admitted<'_>,
        context: &SecurityContext,
        id: SessionId,
    ) -> Result<Session> {
        self.stores.sessions.restore(context, id).await.map_err(store_error(id))
    }

    /// Marks `own`, the own project of a session, deleted with `removal`, in the kernel's name, for
    /// a procedure that holds the session exclusively, as the session is deleted: a holder before
    /// its scope, as the session is (issues 467, 458).
    ///
    /// # Errors
    /// Whatever the store reports.
    pub(crate) async fn mark_own_deleted_held(
        &self,
        _: &Admitted<'_>,
        own: ProjectId,
        removal: &Removal,
    ) -> Result<()> {
        self.stores.projects.delete(&SecurityContext::system(), own, removal).await?;
        Ok(())
    }

    /// Takes the mark of `own`, the own project of a session, away in the kernel's name, for a
    /// procedure that holds the session exclusively: as the session is restored, or its deletion
    /// undone.
    ///
    /// # Errors
    /// Whatever the store reports.
    pub(crate) async fn unmark_own_held(
        &self,
        _: &Admitted<'_>,
        own: ProjectId,
    ) -> Result<Project> {
        Ok(self.stores.projects.restore(&SecurityContext::system(), own).await?)
    }

    /// Deletes what lies in the scope of session `id`, in its channels' and in `own`'s, its own
    /// project's, as far as `context` may delete it (issue 320), marked with `removal`, for a
    /// procedure that holds the session exclusively, as it is deleted. Its agents take part in no
    /// other session (`Scope::admits`): they leave nothing. What is deleted stays deleted, also if
    /// something is left.
    ///
    /// # Errors
    /// `SessionError::ScopeNotEmpty` if something is left that `context` may not delete; whatever
    /// else the store reports.
    pub(crate) async fn delete_scope_held(
        &self,
        _: &Admitted<'_>,
        context: &SecurityContext,
        id: SessionId,
        own: Option<&Project>,
        removal: &Removal,
    ) -> Result<()> {
        let scope = Scope::SessionScope { session_id: id };
        let left = self.scoped.delete_lying_in(context, &held_by(id, own), removal).await?;
        ScopeNotEmpty::require_empty(&scope, left).map_err(SessionError::from)?;
        Ok(())
    }

    /// Restores what lay in the scope of session `id`, in its channels' and in `own`'s, its own
    /// project's, and carries `removal`, as far as `context` may restore it, for a procedure that
    /// holds the session exclusively, as it is restored. What cannot be is logged and stays
    /// deleted: the session comes back all the same.
    pub(crate) async fn restore_scope_held(
        &self,
        _: &Admitted<'_>,
        context: &SecurityContext,
        id: SessionId,
        own: Option<&Project>,
        removal: &Removal,
    ) {
        if let Err(e) = self.scoped.restore_lying_in(context, &held_by(id, own), removal).await {
            warn!(
                "Session {} was restored, but not what lies in its scope or its project's: {}",
                id, e
            );
        }
    }

    /// Deletes the groups of the participants of session `id`, of its channels' and of the members
    /// of `own`, its own project, marked with `removal`, in the kernel's name (issue 348) - all of
    /// them or none -, for a procedure that holds the session exclusively: as it is deleted - last,
    /// since its participants reach it by them while it is kept -, or its restoring undone.
    ///
    /// # Errors
    /// Whatever the store reports.
    pub(crate) async fn delete_participant_groups_held(
        &self,
        _: &Admitted<'_>,
        id: SessionId,
        own: Option<&Project>,
        removal: &Removal,
    ) -> Result<()> {
        self.groups.delete_participants(&held_by(id, own), removal).await?;
        Ok(())
    }

    /// Restores the groups of the participants of session `id`, of its channels' and of the
    /// members of `own`, its own project, that carry `removal`, in the kernel's name (issue 348) -
    /// all of them or none -, for a procedure that holds the session exclusively: as it is
    /// restored, first, so that whatever fails later its participants reach it, or its deletion
    /// undone.
    ///
    /// # Errors
    /// Whatever the store reports.
    pub(crate) async fn restore_participant_groups_held(
        &self,
        _: &Admitted<'_>,
        id: SessionId,
        own: Option<&Project>,
        removal: &Removal,
    ) -> Result<()> {
        self.groups.restore_participants(&held_by(id, own), removal).await?;
        Ok(())
    }

    /// Stops session `id` if it runs, as it is deleted, for a procedure that holds it exclusively:
    /// its actors leave, as those of a deleted channel do (issue 377), and the commands of its
    /// channels end (issue 254).
    pub(crate) async fn stop_deleted_held(&self, admitted: &Admitted<'_>, id: SessionId) {
        if let Some(stopped) = self.stop_running(admitted, id, SessionLoop::leave_actors).await {
            self.end_commands_of(&stopped).await;
        }
    }

    /// Announces `session` deleted by `context`'s caller, with what went with it: `own`, its own
    /// project, then `channels`, those it listed, then the session.
    pub(crate) fn announce_deleted(
        &self,
        session: &Session,
        channels: &[Channel],
        own: Option<&Project>,
        context: &SecurityContext,
    ) {
        if let Some(own) = own {
            self.events.announce(GlobalEvent::deleted::<Project>(&own.id), own);
        }
        for channel in channels {
            self.events
                .announce(GlobalEvent::deleted::<Channel>(&(session.id, channel.id)), channel);
        }
        self.events.announce(GlobalEvent::deleted::<Session>(&session.id), session);
        info!(session = %session.id, by = %context.caller_id, "Session deleted");
    }

    /// Announces `session` restored by `context`'s caller, with `own`, its own project, first.
    pub(crate) fn announce_restored(
        &self,
        session: &Session,
        own: Option<&Project>,
        context: &SecurityContext,
    ) {
        if let Some(own) = own {
            self.events.announce(GlobalEvent::restored::<Project>(&own.id), own);
        }
        self.events.announce(GlobalEvent::restored::<Session>(&session.id), session);
        info!(session = %session.id, by = %context.caller_id, "Session restored");
    }

    /// The projects deleted that lie in the scope of session `id` - its own, deleted with it -,
    /// read in the kernel's name.
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

    /// Removes the deleted session `id` for good on behalf of `context` (issue 164), in the order
    /// of every purge of what has something behind it (`purge_behind`): its purge begun, then - in
    /// the kernel's name, since the purge was permitted - what lies behind it (`remove_behind`),
    /// then its document. Only what was deleted is purged; the store asks `Purge`. A step that
    /// fails leaves the session deleted, its purge begun, so that the next purge removes what is
    /// left (issue 496). For a procedure that holds it exclusively, as deleting and restoring it
    /// do, so that a purge never meets it half deleted or half restored. `false` if there was
    /// nothing deleted the caller could see, as every purge answers (issue 510).
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if they may see it but not purge it,
    /// `SessionError::RunsElsewhere` if another process that shares the stores holds it (issue
    /// 554), or whatever the stores report.
    pub(crate) async fn purge_held(
        &self,
        _: &Admitted<'_>,
        context: &SecurityContext,
        id: SessionId,
    ) -> Result<bool> {
        // What the caller may not see, or what is not deleted, is not there to purge: asked before
        // whether another process holds it, which they learn of only for what they see.
        if self.stores.sessions.get_deleted(context, id).await.optional()?.is_none() {
            return Ok(false);
        }
        let held = self.hold(id)?;
        let purged = purge_behind(self.stores.sessions.as_ref(), context, id, async |session| {
            self.remove_behind(session).await
        })
        .await?;
        if let (true, Some(locks)) = (purged, &self.locks) {
            if let Err(e) = locks.forget(id) {
                warn!(session = %id, "What held a session purged is left: {e}");
            }
        }
        drop(held);
        if purged {
            info!(session = %id, by = %context.caller_id, "Session purged");
        }
        Ok(purged)
    }

    /// Removes what lies behind `session` once its purge is begun, in the kernel's name: the state
    /// of its channels, those it lists and those deleted from it before, the content of their
    /// attachments (issue 282) and then their documents; what lay in its scope and in its channels'
    /// (issue 178), the group of its participants (issue 348), and its own project with what lies
    /// in its scope (issue 467). What a purge that failed before removed already is passed over.
    ///
    /// # Errors
    /// Whatever the stores report.
    async fn remove_behind(&self, session: &Session) -> Result<()> {
        let id = session.id;
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
        // The state lives in the `StateStore`, which may be another store than the session's; the
        // attachments its history carries go with it. Both before the document, which is what
        // finds them.
        for channel_id in channels {
            self.state_store.delete_channel_state(channel_id).await?;
            self.attachments.purge(channel_id).await?;
            purge_channel_document(&self.stores, channel_id).await?;
        }
        let owns: Vec<Project> =
            self.deleted_own(id).await?.into_iter().map(|own| own.entity).collect();
        self.scoped.purge_lying_in(&held_by(id, &owns)).await?;
        purge_for_good(self.stores.groups.as_ref(), GroupId::of_session(id)).await?;
        for own in &owns {
            purge_own(&self.stores, own.id).await?;
        }
        Ok(())
    }

    /// The root of the views (issue 163): every session `viewer` may
    /// read, and from there their channels and participants, running or not.
    pub fn views(self: &Arc<Self>, viewer: Viewer) -> Views {
        Views::new(&self.kernel, Arc::<SessionManager>::clone(self), viewer)
    }

    /// The kernel it starts its sessions in - for what is made beside it, as the `ProjectManager`
    /// is.
    pub(crate) fn kernel(&self) -> &Arc<Kernel> {
        &self.kernel
    }

    /// The loop of session `id`, if it runs.
    pub(crate) fn running(&self, id: SessionId) -> Option<Arc<SessionLoop>> {
        self.sessions.get(&id).map(|entry| entry.clone())
    }
}

/// How the tests make a session past a template: from its document and its channels, as a
/// template would.
#[cfg(any(test, feature = "test-support"))]
impl SessionManager {
    /// Creates the brand-new `session` with its `channels`, as a template would, for the procedure
    /// of the tests that holds it (`session::testing`): asks to create it (`CreateSession`) before
    /// anything else. Its results go to a project of its own, made with it, whatever project it
    /// names (issue 467). The participants the channels name join them as their founders once the
    /// session runs, as a template's do (issue 420).
    ///
    /// # Errors
    /// `SessionError::OwnerNotAUser` if the session is not a user's; otherwise as `establish`.
    pub(crate) async fn make(
        &self,
        admitted: &Admitted<'_>,
        log: &mut Compensation,
        context: &SecurityContext,
        mut session: Session,
        mut channels: Vec<Channel>,
    ) -> Result<Arc<SessionLoop>> {
        self.policies
            .access_policy()
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
        let session_loop = self.establish(admitted, context, session, own, channels).await?;
        for (channel, participants) in founders {
            let channel = session_loop
                .channel(channel)
                .ok_or(SessionError::ChannelNotFound { session: session_loop.id(), channel })?;
            let into = Some((&channel, HistoryAccess::None, ThreadId::line(channel.id())));
            for participant in participants {
                let founder = participant.actor_id();
                take_in(admitted, log, &session_loop, context, founder, Entry::Founding, into)
                    .await?;
            }
        }
        Ok(session_loop)
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
    use crate::agent::AgentError;
    use crate::channel::STOP_GRACE;
    use crate::command::CommandExecutor;
    use crate::component::BUNDLE_REF_KIND;
    use crate::error::is_forbidden;
    use crate::kernel::Kernel;
    use crate::procedure::follower::AgentFollower;
    use crate::procedure::participant::Follow;
    use crate::procedure::session::SHUTDOWN_BOUND;
    use crate::procedure::{
        Admitted, Compensation, Deferred, Key, Locks, Procedure, ProcedureError,
    };
    use crate::project::{ProjectManager, ProjectSpec};
    use crate::session::testing::*;
    use crate::session::{load_own, purge_own, SessionLoop};
    use crate::session::{SessionError, SessionManager, SessionSpec};
    use crate::testing::TestKernel;
    use crate::view::Viewer;
    use anyhow::Result;
    use async_trait::async_trait;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::actor::{ActorEvent, ActorState, Envelope};
    use caelumora_domain::agent::testing::make_personality;
    use caelumora_domain::agent::Agent;
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use caelumora_domain::approval::ApprovalStrategy;
    use caelumora_domain::channel::testing::main_channel;
    use caelumora_domain::channel::{
        self, moderation_components, Channel, ChannelId, InternalActor, Participant,
    };
    use caelumora_domain::component::{ComponentBundle, ComponentConfig, ComponentDescriptor};
    use caelumora_domain::confidentiality::Classification;
    use caelumora_domain::event::{EntityChange, GlobalEvent};
    use caelumora_domain::group::GroupId;
    use caelumora_domain::project::Project;
    use caelumora_domain::query::Query;
    use caelumora_domain::removal::Removal;
    use caelumora_domain::scope::Scope;
    use caelumora_domain::security::{Grant, Secured, SecurityAttributes, Subject};
    use caelumora_domain::session;
    use caelumora_domain::session::testing::make_session;
    use caelumora_domain::session::Session;
    use caelumora_domain::store::{StoreError, Stored};
    use caelumora_domain::template::testing::{make_channel_template, make_template};
    use caelumora_domain::template::{
        ChannelTemplate, InternalActorTemplate, SessionTemplate, SessionTemplateId,
    };
    use caelumora_domain::turn::{GivenUpReason, TurnState};
    use caelumora_domain::user::testing::make_user;
    use caelumora_domain::user::UserProfile;
    use caelumora_domain::user::{User, UserId};
    use caelumora_domain::value::language::Language;
    use caelumora_domain::value::name::{Name, ScopedName};
    use caelumora_domain::value::time_zone::TimeZone;
    use caelumora_domain::value::ValueError;
    use caelumora_domain::workspace::{Workspace, WorkspaceChoice};
    use caelumora_security::SecurityContext;
    use caelumora_store::memory::MemoryStore;
    use caelumora_store::testing::record_users;
    use caelumora_store::testing::stored_agent;
    use caelumora_store::{Deleted, Persistence, StateStore};
    use serde_json::Value;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;
    use std::time::Instant;
    use tokio::sync::broadcast::Receiver;
    use tokio::sync::Semaphore;
    use tokio::time;

    #[tokio::test]
    async fn create_session_emits_created_then_started() {
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let mut rx = kernel.event_bus().subscribe_all();
        let ctx = SecurityContext::default();

        let session = make_session("s1", UserId::new());
        let session_id = session.id;
        let channel_id = session.channels[0];

        create(&manager, &ctx, session).await.unwrap();

        // The manager emits these synchronously, in order, before returning.
        let events = rx.drain().await;
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
        assert!(kernel.entity_stores().sessions.get(&ctx, session_id).await.is_ok());
        assert!(manager.running(session_id).is_some());

        manager.stop(&SecurityContext::system(), session_id).await.unwrap();
    }

    #[tokio::test]
    async fn delete_session_emits_stopped_then_deleted() {
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let ctx = SecurityContext::default();

        let session = make_session("s2", UserId::new());
        let session_id = session.id;
        let channel_id = session.channels[0];
        create(&manager, &ctx, session).await.unwrap();

        // Subscribe after creation so we only observe the deletion events.
        let mut rx = kernel.event_bus().subscribe_all();
        assert!(manager.delete(&ctx, session_id).await.unwrap());

        let events = rx.drain().await;
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

        assert!(kernel.entity_stores().sessions.get(&ctx, session_id).await.is_err());
        assert!(!manager.running(session_id).is_some());
    }

    /// Issue 593: a session's stop announces every channel it started stopped, as it announced it
    /// started, and then the session - its channels no longer stop by themselves.
    #[tokio::test]
    async fn stopping_a_session_announces_its_channels_stopped_then_itself() {
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let session = make_session("s3", UserId::new());
        let session_id = session.id;
        let channel_id = session.channels[0];
        let mut rx = kernel.event_bus().subscribe_all();
        let is_channel = |event: &GlobalEvent, wanted: &str| {
            matches!(event.about::<Channel>(), Some(((s, c), EntityChange::Custom { name, .. }))
                if s == session_id && c == channel_id && *name == wanted)
        };
        create(&manager, &SecurityContext::default(), session).await.unwrap();
        let started = time::timeout(Duration::from_secs(5), async {
            while !rx.recv().await.is_some_and(|event| is_channel(&event, channel::STARTED)) {}
        });
        started.await.expect("its channel is started");

        manager.stop(&SecurityContext::system(), session_id).await.unwrap();

        let events = rx.drain().await;
        let ch_stopped = events.iter().position(|e| is_channel(e, channel::STOPPED));
        let stopped = events.iter().position(|e| {
            matches!(e.about::<Session>(), Some((id, EntityChange::Custom { name, .. }))
                if id == session_id && *name == session::STOPPED)
        });
        assert!(ch_stopped.is_some(), "expected the channel stopped, got {events:?}");
        assert!(ch_stopped < stopped, "the channel, then the session: {events:?}");
    }

    #[tokio::test]
    async fn opening_a_stored_session_emits_started_but_not_created() {
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let ctx = SecurityContext::default();

        // Persist a session directly, without going through `create`.
        let session = make_session("s3", UserId::new());
        let session_id = session.id;
        let mut channels = [main_channel(&session)];
        persist(&manager, &ctx, session, &mut channels).await;

        let mut rx = kernel.event_bus().subscribe_all();
        manager.open(&ctx, session_id).await.unwrap();

        let events = rx.drain().await;
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
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let ctx = SecurityContext::default();
        let session = make_session("concurrent", UserId::new());
        let session_id = session.id;
        let mut channels = [main_channel(&session)];
        persist(&manager, &ctx, session, &mut channels).await;

        let mut rx = kernel.event_bus().subscribe_all();
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

        let started = rx.drain().await.into_iter().filter(|e| matches!(e.about::<Session>(), Some((id, EntityChange::Custom { name: session::STARTED, .. })) if id == session_id)).count();
        assert_eq!(started, 1, "the session started once");
        manager.stop(&SecurityContext::system(), session_id).await.unwrap();
    }

    #[tokio::test]
    async fn a_stop_waits_for_a_start_under_way_and_then_stops_what_it_started() {
        /// A start of `session`, which holds it exclusively as `Open` does and registers it
        /// once `go` lets it go on.
        struct Starting {
            manager: Arc<SessionManager>,
            session: Session,
            channels: Vec<Channel>,
            go: Arc<Semaphore>,
        }

        #[async_trait]
        impl Procedure for Starting {
            type Plan = ();
            type Output = ();
            type Error = anyhow::Error;

            async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
                locks.exclusive(Key::Session(self.session.id))
            }

            async fn carry_out(
                self,
                (): (),
                admitted: &Admitted<'_>,
                _: &mut Compensation,
            ) -> Result<()> {
                self.go.acquire().await?.forget();
                self.manager.spawn_session(admitted, self.session, self.channels, None).await?;
                Ok(())
            }
        }

        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let ctx = SecurityContext::default();
        let session = make_session("stopped", UserId::new());
        let session_id = session.id;
        let mut channels = [main_channel(&session)];
        let session = persist(&manager, &ctx, session, &mut channels).await;

        // A start under way holds the session ...
        let go = Arc::new(Semaphore::new(0));
        let channels = channels.to_vec();
        let start =
            Starting { manager: Arc::clone(&manager), session, channels, go: Arc::clone(&go) };
        let starting = tokio::spawn({
            let kernel = Arc::clone(&kernel);
            async move { kernel.procedures().run(start).await.unwrap() }
        });
        time::sleep(Duration::from_millis(50)).await;
        let stopping = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.stop(&SecurityContext::system(), session_id).await.unwrap() }
        });
        time::sleep(Duration::from_millis(50)).await;
        assert!(!stopping.is_finished(), "the stop waits for the start");

        // ... and registers the session before it lets go: the stop then
        // finds it, and nothing keeps running.
        go.add_permits(1);
        starting.await.unwrap();
        assert!(stopping.await.unwrap(), "the stop stopped what the start started");
        assert!(!manager.running(session_id).is_some());
    }

    #[tokio::test]
    async fn a_session_has_the_global_commands_and_its_own_but_no_name_twice() {
        use caelumora_domain::command::HELP_KIND;
        use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
        let kernel = test_kernel();
        let names = |executor: CommandExecutor| async move {
            executor
                .visible(&SecurityContext::default(), &SecurityAttributes::systems())
                .await
                .into_iter()
                .map(|c| c.name.to_string())
                .collect::<Vec<_>>()
        };

        let session = make_session("plain", UserId::new());
        let executor =
            kernel.new_command_executor(&session).await.expect("the global commands build");
        assert_eq!(
            names(executor).await,
            vec![
                "agent create",
                "agent delete",
                "agent list",
                "agent usage",
                "agent wakeup",
                "channel briefing",
                "channel create",
                "channel delete",
                "channel describe",
                "channel join",
                "channel leave",
                "channel list",
                "channel notes",
                "channel rename",
                "channel summary",
                "help",
                "participant add",
                "participant list",
                "participant nudge",
                "participant remove"
            ]
        );

        // A second /help from the session's own configuration - with the same
        // component id, which is fine across the two configurations.
        let mut session = make_session("clashing", UserId::new());
        session.commands = ComponentConfig {
            component_nodes: vec![ComponentDescriptor {
                id: "help".to_string(),
                kind: HELP_KIND.to_string(),
                params: Value::Null,
                tags: Vec::new(),
                fail_policy: Default::default(),
            }],
        };
        let error = kernel.new_command_executor(&session).await.expect_err("/help twice");
        assert!(error.to_string().contains("/help"), "{error}");
    }

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn a_deleted_agent_rests_in_its_channels_keeps_its_name_and_comes_back_restored() {
        use caelumora_domain::channel::Participant;
        /// Waits until `holds` does, at most two seconds: a running channel
        /// follows the agent as the follower of the agents learns of it.
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
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        // Followed as the programs follow them (issue 593).
        let _followed = AgentFollower::spawn(&manager);
        let agents = kernel.agent_manager();
        let (system, ctx) = (SecurityContext::system(), SecurityContext::default());
        let helper = stored_agent(
            kernel.entity_stores(),
            "helper",
            make_personality("plain"),
            ActorId::User(UserId::new()),
        )
        .await;
        let actor = ActorId::Agent(helper.id);

        // In a running session's channel, and in a stopped one's.
        let mut running = make_session("running", UserId::new());
        running.participants = vec![Participant::Agent(helper.id)];
        let running = create(&manager, &ctx, running).await.unwrap();
        let talk = running.create_channel(&ctx, spec("talk", vec![actor])).await.unwrap();
        let mut stopped = make_session("stopped", UserId::new());
        stopped.participants = vec![Participant::Agent(helper.id)];
        let mut channels = [main_channel(&stopped)];
        let stopped = persist(&manager, &system, stopped, &mut channels).await;

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
                &kernel.entity_stores().channels.get(&system, stopped.channels[0]).await.unwrap()
            ),
            "and in the stopped one"
        );
        assert!(
            kernel.entity_stores().agents.get(&system, helper.id).await.is_err(),
            "it is not there"
        );
        let views = manager.views(Viewer::new(ctx.clone()));
        let reached = views.channel_of(running.id(), talk.id()).await.unwrap().there().unwrap();
        let former = reached.participant(actor).await.unwrap();
        assert!(
            former.former() && former.name().map(Name::as_str) == Some("helper"),
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
        let resting = main.leave(&ctx, actor).await.expect_err("its owner cannot take it out");
        assert!(is_forbidden(&resting), "it cannot be resolved (issue 562): {resting}");
        main.remove_participant(&ctx, actor).await.expect("removed while it rests");
        assert!(
            main.channel().former.iter().any(|f| f.participant.actor_id() == actor),
            "a former participant there"
        );

        // Its name taken meanwhile: not restored.
        let mut again = Agent::new(
            ScopedName::new("helper", Scope::GlobalScope).unwrap(),
            helper.personality_id,
            ActorId::User(UserId::new()),
        );
        kernel.entity_stores().agents.create(&system, &mut again).await.unwrap();
        let taken = agents.restore(&ctx, helper.id).await.expect_err("its name is taken");
        assert!(
            matches!(taken, AgentError::Store(StoreError::NameTaken { entity: "agent" })),
            "{taken}"
        );

        // Purged: a former participant still, not knowing when it left.
        assert!(agents.purge(&ctx, helper.id).await.expect("purged"));
        assert!(
            kernel.entity_stores().agents.get_deleted(&system, helper.id).await.is_err(),
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
    async fn an_agent_deleted_unheard_of_is_found_out_as_the_session_is_followed_again() {
        use caelumora_domain::channel::Participant;
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let (system, ctx) = (SecurityContext::system(), SecurityContext::default());
        let helper = stored_agent(
            kernel.entity_stores(),
            "helper",
            make_personality("plain"),
            ActorId::User(UserId::new()),
        )
        .await;
        let actor = ActorId::Agent(helper.id);
        let mut session = make_session("running", UserId::new());
        session.participants = vec![Participant::Agent(helper.id)];
        let session = create(&manager, &ctx, session).await.unwrap();
        let talk = session.create_channel(&ctx, spec("talk", vec![actor])).await.unwrap();

        // Deleted with nothing announced, as if the session lost the event.
        assert!(talk.participant_ids().contains(&actor));
        let mut rx = kernel.event_bus().subscribe_all();
        kernel
            .entity_stores()
            .agents
            .delete(&system, helper.id, &Removal::now(system.caller_id))
            .await
            .unwrap();
        let following = Follow::session(&manager, session.id());
        kernel.procedures().run(following).await.unwrap();
        assert!(!talk.participant_ids().contains(&actor), "looking again, it stops the actor");
        let announced = rx.drain().await;
        assert!(announced.iter().any(|e| matches!(e.about::<Channel>(), Some((address, change)) if address == (session.id(), talk.id()) && change.data::<ActorId>(channel::LEFT) == Some(ActorId::Agent(helper.id)))), "as a leave: {announced:?}");

        kernel.entity_stores().agents.restore(&system, helper.id).await.unwrap();
        let following = Follow::session(&manager, session.id());
        kernel.procedures().run(following).await.unwrap();
        assert!(talk.participant_ids().contains(&actor), "and starts it again");
        let announced = rx.drain().await;
        assert!(announced.iter().any(|e| matches!(e.about::<Channel>(), Some(((_, c), change)) if c == talk.id() && change.data::<ActorId>(channel::JOINED) == Some(ActorId::Agent(helper.id)))), "as a join: {announced:?}");
        manager.stop(&system, session.id()).await.unwrap();
    }

    /// The template "pairing", stored: one channel "main" for its "user", moderated by a broadcast
    /// and with an approver deciding by all. Returned with the user it is made for, stored too.
    async fn pairing(kernel: &Kernel) -> (UserId, SessionTemplate) {
        let system = SecurityContext::system();
        let owner = UserId::new();
        record_users(kernel.entity_stores(), &[&User { id: owner, ..make_user("alice") }]).await;
        let mut template = SessionTemplate {
            channels: vec![ChannelTemplate {
                components: moderation_components(None, Some("broadcast_policy")),
                internal_actors: vec![InternalActorTemplate::Approver {
                    strategy: ApprovalStrategy::All,
                }],
                ..make_channel_template("main")
            }],
            ..make_template("pairing")
        };
        kernel.entity_stores().session_templates.create(&system, &mut template).await.unwrap();
        (owner, template)
    }

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn a_session_is_created_from_a_template_for_its_owner() {
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let (owner, template) = pairing(&kernel).await;
        let hers =
            SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let mut profile = UserProfile::new(owner, None);
        profile.preferred_language = Some(Language::new("de").unwrap());
        kernel.entity_stores().user_profiles.create(&hers, &mut profile).await.unwrap();

        let session = manager
            .create_from_template(&hers, template.id, SessionSpec::default())
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
        let stored = kernel.entity_stores().channels.get(&hers, main.id()).await.unwrap();
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
            .create_from_template(&hers, SessionTemplateId::new(), SessionSpec::default())
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
            .create_from_template(&SecurityContext::system(), template.id, SessionSpec::default())
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
            .create_from_template(
                &hers,
                template.id,
                SessionSpec {
                    name: Some("in French".to_string()),
                    language: Some("fr".to_string()),
                    ..SessionSpec::default()
                },
            )
            .await
            .expect("held in the language asked for");
        assert_eq!(
            asked.session().language,
            Some(Language::new("fr").unwrap()),
            "the session, issue 425"
        );
        let stored = kernel.entity_stores().sessions.get(&hers, asked.id()).await.unwrap();
        assert_eq!(
            stored.language,
            Some(Language::new("fr").unwrap()),
            "stored, not only in memory"
        );
        let no_tag = manager
            .create_from_template(
                &hers,
                template.id,
                SessionSpec {
                    name: Some("in no language".to_string()),
                    language: Some("no language".to_string()),
                    ..SessionSpec::default()
                },
            )
            .await
            .expect_err("no language tag");
        assert!(no_tag.downcast_ref::<ValueError>().is_some(), "{no_tag}");
        let zoned = manager
            .create_from_template(
                &hers,
                template.id,
                SessionSpec {
                    name: Some("in Berlin".to_string()),
                    time_zone: Some("Europe/Berlin".to_string()),
                    ..SessionSpec::default()
                },
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
                SessionSpec {
                    name: Some("nowhere".to_string()),
                    time_zone: Some("Europe/Atlantis".to_string()),
                    ..SessionSpec::default()
                },
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
        let mut broken = SessionTemplate {
            id: SessionTemplateId::new(),
            name: ScopedName::new("broken", Default::default()).unwrap(),
            channels: vec![channel],
            ..template
        };
        kernel
            .entity_stores()
            .session_templates
            .create(&SecurityContext::system(), &mut broken)
            .await
            .unwrap();
        let refused = manager
            .create_from_template(&hers, broken.id, SessionSpec::default())
            .await
            .expect_err("a bundle she cannot read");
        let Some(&SessionError::ChannelUnavailable { session, .. }) =
            refused.downcast_ref::<SessionError>()
        else {
            panic!("unavailable: {refused}")
        };
        let system = SecurityContext::system();
        assert!(
            kernel.entity_stores().sessions.get(&system, session).await.is_err()
                && kernel.entity_stores().sessions.get_deleted(&system, session).await.is_err(),
            "rolled back"
        );
    }

    /// Issue 476: a session given one of its owner's workspaces is not made if the workspace
    /// cannot be shared with its participants as asked, as one that cannot be completed is not.
    #[tokio::test]
    async fn a_session_whose_workspace_cannot_be_shared_is_not_made() {
        let (kernel, passing) = failing_kernel(Workspace::KIND);
        let stores = kernel.entity_stores().clone();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let (owner, template) = pairing(&kernel).await;
        let hers =
            SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let system = SecurityContext::system();
        let mut docs = Workspace::new(
            ScopedName::new("docs", Scope::UserScope { user_id: owner }).unwrap(),
            "dev",
            Classification::default(),
            owner,
        );
        stores.workspaces.create(&system, &mut docs).await.unwrap();

        passing.store(0, Ordering::SeqCst);
        manager
            .create_from_template(
                &hers,
                template.id,
                SessionSpec {
                    workspace: Some(WorkspaceChoice::Existing {
                        id: docs.id,
                        share: Some(Grant::Read),
                    }),
                    ..SessionSpec::default()
                },
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
        let store = Persistence::new(entities, Arc::<MemoryStore>::clone(&states)).unwrap();
        let kernel = TestKernel { store, ..Default::default() }.build();
        let manager = SessionManager::new(kernel);
        let stores = manager.kernel().entity_stores();
        let ctx = SecurityContext::default();
        let session = make_session("two-stores", UserId::new());
        let (id, channel_id) = (session.id, session.channels[0]);
        create(&manager, &ctx, session).await.unwrap();
        assert!(
            states.get_channel_state(channel_id).await.is_ok(),
            "the channel keeps its state in the state store"
        );

        assert!(manager.delete(&ctx, id).await.unwrap());
        assert!(stores.sessions.get(&ctx, id).await.is_err(), "not there");
        assert!(
            states.get_channel_state(channel_id).await.is_ok(),
            "but kept, with its channels' state (issue 164)"
        );
        manager.restore(&ctx, id).await.expect("restored");
        manager.open(&ctx, id).await.expect("and opened as any stored session");
        assert!(manager.restore(&ctx, id).await.is_err(), "not deleted");
        assert!(!manager.purge(&ctx, id).await.unwrap(), "only what was deleted is purged");

        // A channel deleted from it before goes with it, too.
        let side = manager
            .open(&ctx, id)
            .await
            .unwrap()
            .create_channel(&ctx, spec("side", vec![]))
            .await
            .unwrap()
            .id();
        assert!(
            manager.open(&ctx, id).await.unwrap().delete_channel(&ctx, side).await.unwrap(),
            "deleted"
        );
        assert!(states.get_channel_state(side).await.is_ok(), "kept with the deleted channel");
        assert!(manager.delete(&ctx, id).await.unwrap());
        assert!(manager.purge(&ctx, id).await.expect("purged"));
        assert!(stores.sessions.get_deleted(&ctx, id).await.is_err(), "nothing left to restore");
        assert!(
            states.get_channel_state(channel_id).await.is_err(),
            "deleted from the state store, not only from the entity store"
        );
        assert!(
            stores.channels.get(&ctx, channel_id).await.is_err(),
            "its channels go with it (issue 70)"
        );
        assert!(
            stores.channels.get_deleted(&ctx, side).await.is_err(),
            "also those deleted from it before"
        );
        assert!(states.get_channel_state(side).await.is_err(), "with their state");
    }

    /// Issue 467: a session is made with a project of its own, read by its
    /// participants, which goes, comes back and is purged with it, with the
    /// group of its members and what lies in its scope.
    #[tokio::test]
    async fn a_session_s_own_project_goes_and_comes_back_with_it() {
        use caelumora_domain::security::AclEntry;

        let kernel = test_kernel();
        let stores = kernel.entity_stores().clone();
        let mut events = kernel.event_bus().subscribe_all();
        let manager = SessionManager::new(kernel);
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
            events
                .drain()
                .await
                .iter()
                .any(|e| e.about::<Project>() == Some((project, &EntityChange::Created))),
            "announced"
        );
        let mut bundle = ComponentBundle::new(
            ScopedName::new("tools", Scope::ProjectScope { project_id: project }).unwrap(),
            Default::default(),
            SecurityAttributes::systems(),
        );
        stores.component_bundles.create(&system, &mut bundle).await.unwrap();

        assert!(manager.delete(&ctx, id).await.unwrap());
        assert!(stores.projects.get(&system, project).await.is_err(), "deleted with it");
        assert!(stores.groups.get(&system, members).await.is_err(), "its group too");
        assert!(
            stores.component_bundles.get(&system, bundle.id).await.is_err(),
            "and what lies in its scope"
        );
        assert!(
            events
                .drain()
                .await
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
            events
                .drain()
                .await
                .iter()
                .any(|e| e.about::<Project>() == Some((project, &EntityChange::Restored))),
            "announced"
        );

        assert!(manager.delete(&ctx, id).await.unwrap());
        assert!(manager.purge(&ctx, id).await.unwrap());
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
        let (kernel, passing) = failing_kernel(Project::KIND);
        let stores = kernel.entity_stores().clone();
        let manager = SessionManager::new(kernel);
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

        let mut bundle = ComponentBundle::new(
            ScopedName::new("tools", Scope::SessionScope { session_id: id }).unwrap(),
            Default::default(),
            SecurityAttributes::systems(),
        );
        stores.component_bundles.create(&system, &mut bundle).await.unwrap();
        assert!(manager.delete(&ctx, id).await.expect("deleted now"));
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
        let (kernel, passing) = failing_kernel(Session::KIND);
        let stores = kernel.entity_stores().clone();
        let manager = SessionManager::new(kernel);
        let owner = UserId::new();
        let ctx =
            SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let session = create(&manager, &ctx, make_session("moving", owner)).await.unwrap();
        let own = session.session().project;
        let project = ProjectManager::new(Arc::clone(&manager))
            .create(&ctx, ProjectSpec::new("report", Scope::UserScope { user_id: owner }))
            .await
            .unwrap();

        passing.store(0, Ordering::SeqCst);
        manager
            .update_project(&ctx, session.id(), Some(project.id))
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

    /// Issue 593: a session taken out of a shared project into a new own - the one in its scope was
    /// lifted - that cannot be stored there lies where it lay, and the own made for it is purged
    /// again.
    #[tokio::test]
    async fn a_session_that_cannot_be_taken_into_a_new_own_leaves_none_behind() {
        let (kernel, passing) = failing_kernel(Session::KIND);
        let stores = kernel.entity_stores().clone();
        let manager = SessionManager::new(kernel);
        let owner = UserId::new();
        let ctx =
            SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let session = create(&manager, &ctx, make_session("moving", owner)).await.unwrap();
        let project = ProjectManager::new(Arc::clone(&manager))
            .create(&ctx, ProjectSpec::new("report", Scope::UserScope { user_id: owner }))
            .await
            .unwrap();
        manager.update_project(&ctx, session.id(), Some(project.id)).await.unwrap();
        // Its own gone, as lifting it takes it out of the session's scope.
        let own = load_own(&stores, session.id()).await.unwrap().expect("its own");
        purge_own(&stores, own.id).await.unwrap();

        passing.store(0, Ordering::SeqCst);
        manager
            .update_project(&ctx, session.id(), None)
            .await
            .expect_err("the session cannot be stored");
        assert_eq!(session.session().project, project.id, "it lies where it lay");
        let made = load_own(&stores, session.id()).await.unwrap();
        assert!(made.is_none(), "the own made for it is purged: {made:?}");
    }

    /// Issue 554: a session runs in one process of those that share the stores - held while it
    /// runs here, let go as it stops -, and is neither started, deleted, restored nor purged while
    /// another holds it; what held a session purged is forgotten.
    #[tokio::test]
    async fn a_session_another_process_holds_is_left_to_it() {
        let (kernel, locks) = shared_kernel();
        let manager = SessionManager::new(kernel);
        let ctx = SecurityContext::default();
        let session = make_session("shared", UserId::new());
        let id = session.id;
        let runs_elsewhere = |result: Result<_>| {
            let error = result.expect_err("refused");
            matches!(error.downcast_ref(), Some(SessionError::RunsElsewhere(it)) if *it == id)
        };

        create(&manager, &ctx, session).await.unwrap();
        assert!(locks.holds(id), "held while it runs here");
        assert!(manager.stop(&ctx, id).await.unwrap());
        assert!(!locks.holds(id), "let go as it stops");

        locks.elsewhere.lock().unwrap().insert(id);
        assert!(runs_elsewhere(manager.open(&ctx, id).await.map(|_| ())), "not started");
        assert!(runs_elsewhere(manager.delete(&ctx, id).await.map(|_| ())), "not deleted");
        locks.elsewhere.lock().unwrap().remove(&id);
        manager.open(&ctx, id).await.expect("started once let go");
        assert!(manager.delete(&ctx, id).await.unwrap(), "deleted while it runs here");
        assert!(!locks.holds(id), "stopped with it");

        locks.elsewhere.lock().unwrap().insert(id);
        assert!(runs_elsewhere(manager.restore(&ctx, id).await.map(|_| ())), "not restored");
        assert!(runs_elsewhere(manager.purge(&ctx, id).await.map(|_| ())), "not purged");
        locks.elsewhere.lock().unwrap().remove(&id);
        assert!(manager.purge(&ctx, id).await.unwrap());
        assert_eq!(*locks.forgotten.lock().unwrap(), vec![id], "nothing of it stays");
        assert!(!locks.holds(id), "and nothing holds it");
    }

    /// Issue 496: a purge that fails part of the way - here as the document of its channel is
    /// marked - leaves the session deleted, its purge begun, and the next purge removes what is
    /// left.
    #[tokio::test]
    async fn a_purge_that_fails_part_of_the_way_is_finished_by_the_next() {
        let (kernel, passing) = failing_kernel(Channel::KIND);
        let stores = kernel.entity_stores().clone();
        let states = Arc::clone(kernel.state_store());
        let manager = SessionManager::new(kernel);
        let (ctx, system) = (SecurityContext::default(), SecurityContext::system());
        let session = make_session("purged", UserId::new());
        let (id, channel_id) = (session.id, session.channels[0]);
        create(&manager, &ctx, session).await.unwrap();
        assert!(manager.delete(&ctx, id).await.unwrap());

        passing.store(0, Ordering::SeqCst);
        manager.purge(&ctx, id).await.expect_err("its channel cannot be marked");
        let left = stores.sessions.get_deleted(&system, id).await.expect("the session is left");
        assert!(left.entity.purging(), "its purge begun");
        assert!(stores.channels.get(&system, channel_id).await.is_ok(), "with its channel");
        assert!(
            states.get_channel_state(channel_id).await.is_err(),
            "whose state went before its document"
        );
        assert!(manager.restore(&ctx, id).await.is_err(), "not to be restored");

        passing.store(usize::MAX, Ordering::SeqCst);
        assert!(manager.purge(&ctx, id).await.expect("purged when tried again"));
        assert!(stores.sessions.get_deleted(&system, id).await.is_err(), "the session is gone");
        assert!(stores.channels.get(&system, channel_id).await.is_err(), "and its channel");
    }

    #[tokio::test]
    async fn a_purged_session_takes_the_attachments_of_its_channels_along() {
        use crate::testing::World;
        use bytes::Bytes;
        use caelumora_api::error::AttachmentError;
        use futures::stream;

        let world = World::new().await;
        let session = world.shared_session(&[], &[]).await;
        let (alice, system) = (World::as_user(world.alice), SecurityContext::system());
        let main = session.channels()[0].channel();
        let attachments = world.kernel.attachment_manager();
        let uploaded = attachments
            .create(
                &alice,
                &main,
                "a.txt",
                None,
                (1, Box::pin(stream::iter([Ok(Bytes::from_static(b"a"))]))),
            )
            .await
            .unwrap();

        world.manager.stop(&alice, session.id()).await.unwrap();
        assert!(world.manager.delete(&alice, session.id()).await.unwrap());
        assert!(
            attachments.content_of(&uploaded).await.is_ok(),
            "kept while the session can be restored"
        );
        assert!(world.manager.purge(&system, session.id()).await.unwrap());
        assert!(
            matches!(attachments.content_of(&uploaded).await, Err(AttachmentError::NotFound(_))),
            "gone with its channel's state"
        );
    }

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn what_lies_in_a_scope_goes_with_its_session_or_channel() {
        use caelumora_domain::agent::Personality;
        use caelumora_domain::user::UserId;

        let kernel = test_kernel();
        let personalities = Arc::clone(&kernel.entity_stores().personalities);
        let bundles = Arc::clone(&kernel.entity_stores().component_bundles);
        let agents = Arc::clone(&kernel.entity_stores().agents);
        let stores = kernel.entity_stores().clone();
        let manager = SessionManager::new(kernel);
        let (ctx, system) = (SecurityContext::default(), SecurityContext::system());
        let session = make_session("scoped", UserId::new());
        let id = session.id;
        let running = create(&manager, &ctx, session).await.unwrap();
        let side = running.create_channel(&ctx, spec("side", vec![])).await.unwrap().id();

        let mut personality = Personality {
            name: ScopedName::new("planner", Scope::SessionScope { session_id: id }).unwrap(),
            ..make_personality("planner")
        };
        personalities.create(&system, &mut personality).await.unwrap();
        let mut bundle = ComponentBundle::new(
            ScopedName::new("tools", Scope::SessionScope { session_id: id }).unwrap(),
            Default::default(),
            SecurityAttributes::systems(),
        );
        bundles.create(&system, &mut bundle).await.unwrap();
        let agent_in = |name: &str, scope| {
            Agent::new(
                ScopedName::new(name, scope).unwrap(),
                personality.id,
                ActorId::User(UserId::new()),
            )
        };
        let mut sessions = agent_in("helper", Scope::SessionScope { session_id: id });
        let mut sides = agent_in("aside", Scope::ChannelScope { session_id: id, channel_id: side });
        let mut users = agent_in("own", Scope::UserScope { user_id: UserId::new() });
        let mut earlier = agent_in("earlier", Scope::SessionScope { session_id: id });
        for agent in [&mut sessions, &mut sides, &mut users, &mut earlier] {
            agents.create(&system, agent).await.unwrap();
        }
        agents.delete(&system, earlier.id, &Removal::now(system.caller_id)).await.unwrap();
        let present = |agent: &Agent| {
            let (agents, id) = (Arc::clone(&agents), agent.id);
            async move { agents.get(&SecurityContext::system(), id).await.is_ok() }
        };

        assert!(running.delete_channel(&ctx, side).await.unwrap(), "deleted");
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

        assert!(manager.delete(&ctx, id).await.unwrap());
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
        assert!(manager.delete(&ctx, id).await.unwrap());
        let mut taker = agent_in("Aside", Scope::ChannelScope { session_id: id, channel_id: side });
        agents.create(&system, &mut taker).await.unwrap();
        manager.restore(&ctx, id).await.unwrap();
        assert!(!present(&sides).await, "the one whose name is taken stays deleted");
        assert!(present(&sessions).await && present(&taker).await, "the others are back");

        assert!(manager.delete(&ctx, id).await.unwrap());
        assert!(manager.purge(&ctx, id).await.unwrap());
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
    async fn the_agents_of_a_scope_are_announced_as_they_go_and_purged_with_their_state() {
        use caelumora_domain::agent::{AgentChannelState, PersonalityId};
        use caelumora_domain::channel::ChannelId;
        use caelumora_domain::work::Work;

        let kernel = test_kernel();
        let agents = Arc::clone(&kernel.entity_stores().agents);
        let states = Arc::clone(kernel.state_store());
        let mut events = kernel.event_bus().subscribe_all();
        let manager = SessionManager::new(kernel);
        let (ctx, system) = (SecurityContext::default(), SecurityContext::system());
        let session = make_session("scoped", UserId::new());
        let id = session.id;
        create(&manager, &ctx, session).await.unwrap();
        let agent_in = |scope| {
            Agent::new(
                ScopedName::new("helper", scope).unwrap(),
                PersonalityId::new(),
                ActorId::User(UserId::new()),
            )
        };
        let mut sessions = agent_in(Scope::SessionScope { session_id: id });
        let mut users = agent_in(Scope::UserScope { user_id: UserId::new() });
        let elsewhere = ChannelId::new();
        for agent in [&mut sessions, &mut users] {
            agents.create(&system, agent).await.unwrap();
            let mut state = AgentChannelState::default();
            state.begin_turn(Work::new(
                Default::default(),
                None,
                Default::default(),
                Default::default(),
            ));
            states.put_agent_channel_state(agent.id, elsewhere, &mut state).await.unwrap();
        }

        assert!(manager.delete(&ctx, id).await.unwrap());
        assert!(
            events
                .drain()
                .await
                .iter()
                .any(|e| e.about::<Agent>() == Some((sessions.id, &EntityChange::Deleted))),
            "the session's agent is announced as it goes"
        );
        manager.restore(&ctx, id).await.unwrap();
        assert!(
            events
                .drain()
                .await
                .iter()
                .any(|e| e.about::<Agent>() == Some((sessions.id, &EntityChange::Restored))),
            "and as it comes back"
        );

        assert!(manager.delete(&ctx, id).await.unwrap());
        assert!(manager.purge(&ctx, id).await.unwrap());
        let working = |agent: AgentId| {
            let states = Arc::clone(&states);
            async move { states.get_agent_channel_state(agent, elsewhere).await.unwrap().turn.is_some() }
        };
        assert!(!working(sessions.id).await, "its state is purged with it");
        assert!(working(users.id).await, "the user's agent keeps its own");
    }

    #[tokio::test]
    async fn a_running_session_is_no_way_around_the_policy() {
        use caelumora_domain::user::UserId;

        let (_, manager) = owned_kernel(Default::default());

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
            manager.running(id).is_some(),
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
        assert!(manager.running(id).is_some(), "it runs on");
        assert!(manager.stop(&as_user(alice), id).await.unwrap(), "the owner stops it");
        assert!(!manager.stop(&as_user(alice), id).await.unwrap(), "stopped already");

        assert!(manager.delete(&as_user(alice), id).await.expect("the owner deletes it"));
        assert!(!manager.running(id).is_some());
    }

    /// A running session of alice and an agent of hers that hangs in its
    /// answer to her hello (`Script::Hangs`): the manager, alice's context,
    /// the session, the agent and what its channel takes in from the start
    /// of the answer on.
    async fn with_a_hanging_answer(
    ) -> (Arc<SessionManager>, SecurityContext, Arc<SessionLoop>, ActorId, Receiver<Envelope>) {
        use crate::testing::{Script, ScriptedAgents};

        let store = Persistence::in_memory();
        let kernel = TestKernel {
            store: store.clone(),
            libraries: vec![&ScriptedAgents(Script::Hangs)],
            ..Default::default()
        }
        .build();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let alice = make_user("alice");
        let participants = record_users(kernel.entity_stores(), &[&alice]).await;
        let thinker = stored_agent(
            kernel.entity_stores(),
            "thinker",
            make_personality("thinker"),
            ActorId::User(alice.id),
        )
        .await;
        let agent = ActorId::Agent(thinker.id);

        let alice_ctx =
            SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let mut session = make_session("thinking", UserId::new());
        session.participants = participants;
        session.participants.push(Participant::Agent(thinker.id));
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
                Some(SessionError::Procedure(ProcedureError::ShuttingDown))
            )
        };
        assert!(refused(manager.open(&alice, stored).await), "a stored one is not started");
        assert!(refused(manager.open(&alice, session.id()).await), "nor one that ran");
        assert!(
            refused(create(&manager, &alice, make_session("new", UserId::new())).await),
            "nor a new one"
        );
    }

    /// After a restart (issue 388) the kernel starts again the sessions in which an agent's stored
    /// state has work under way, each once however many agents have, and no other: not one whose
    /// agents wait for a person or idle, not one deleted, nor what a channel that is gone leaves in
    /// the index. Once shut down, none.
    #[tokio::test]
    async fn a_restart_starts_again_the_sessions_with_work_under_way_and_no_other() {
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let ctx = SecurityContext::default();
        let sessions = ["interrupted", "waiting", "idle", "deleted"]
            .map(|name| make_session(name, UserId::new()));
        for session in &sessions {
            persist(&manager, &ctx, session.clone(), &mut [main_channel(session)]).await;
        }
        let [interrupted, waiting, idle, deleted] =
            sessions.map(|session| (session.id, session.channels[0]));
        let in_turn = |processing_state| AgentChannelState {
            processing_state,
            turn: Some(TurnState::default()),
            ..AgentChannelState::default()
        };
        let states = kernel.state_store();
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
