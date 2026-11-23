//! The procedures of a session's lifecycle (issue 593): creating, opening, stopping, deleting,
//! restoring and purging it, moving it into another project, and stopping every session as the
//! process ends.
//!
//! Each locks its session exclusively - opening one that runs locks it shared -, and the project
//! where it changes whether the session is in it or deleted. Each asks what the change asks and
//! carries it out by the `SessionManager` - which starts and stops the sessions that run here and
//! holds the lifecycles of sessions and their channels (`Lifecycle`), each change announced as it
//! is stored -, by the `ProjectManager`, which holds the lifecycle of their own projects, by the
//! session's loop and controller, and by the services of the kernel: the groups of the
//! participants, what lies in a scope, the commands, the state of the channels.

use super::{
    all_or_nothing, sessions_of, Compensation, Deferred, Key, Locks, Procedure, ProcedureExecutor,
};
use crate::channel::Ending;
use crate::group::GroupManager;
use crate::kernel::Kernel;
use crate::project;
use crate::scoped::ScopeNotEmpty;
use crate::session::{
    require_attachable, require_classification, store_error, Found, SessionError, SessionLoop,
    SessionManager, SessionSpec,
};
use crate::template::{
    resolve_participants, resolve_workspace, take_in_participants, TemplateParticipants,
};
use anyhow::{Error, Result};
use async_trait::async_trait;
use caelumora_domain::channel::{Channel, ChannelField, ChannelId};
use caelumora_domain::group::{Group, GroupId};
use caelumora_domain::project::{Project, ProjectId};
use caelumora_domain::query::{Filter, Query};
use caelumora_domain::removal::Removal;
use caelumora_domain::scope::{Scope, ScopeField};
use caelumora_domain::security::{Grant, Subject};
use caelumora_domain::session::{Session, SessionId};
use caelumora_domain::store::{StoreError, StoreResultExt, Stored};
use caelumora_domain::template::{
    ScopeTemplate, SessionTemplate, SessionTemplateId, WorkspaceTemplate,
};
use caelumora_domain::value::language::Language;
use caelumora_domain::value::name::Name;
use caelumora_domain::value::time_zone::TimeZone;
use caelumora_domain::workspace::WorkspaceChoice;
use caelumora_security::{AccessQuestions, ActionType, Enforcement, SecurityContext, JOIN};
use caelumora_store::Deleted;
use futures::future::join_all;
use std::collections::HashSet;
use std::iter;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// How long shutting the sessions down takes at most (issue 297): within the ten seconds Docker
/// leaves between SIGTERM and SIGKILL, with the actors' `STOP_GRACE` and the time a command's kill
/// waits.
pub(crate) const SHUTDOWN_BOUND: Duration = Duration::from_secs(8);

/// Creates a session from a template and starts it, holding it until it is complete: its
/// workspace, its participants and its project (issue 88). If any of it fails, its creation is
/// rolled back, with what was made in its scope or for it.
pub struct Create {
    context: SecurityContext,
    /// Chosen before it is built, so that it is held before anyone can find it.
    id: SessionId,
    template_id: SessionTemplateId,
    spec: SessionSpec,
}

impl Create {
    /// Creates a session from the template `template_id` as `spec` asks, on behalf of `context`,
    /// whose user owns it and is the template's "user" (`make_from_template`).
    pub fn from_template(
        context: &SecurityContext,
        template_id: SessionTemplateId,
        spec: SessionSpec,
    ) -> Self {
        Self { context: context.clone(), id: SessionId::new(), template_id, spec }
    }
}

#[async_trait]
impl Procedure for Create {
    type Plan = ();
    type Output = Arc<SessionLoop>;
    type Error = SessionError;

    async fn enumerate(&self, _: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        locks.exclusive(Key::Session(self.id))?;
        // The project it is put into, which must not go meanwhile.
        if let Some(project) = self.spec.project {
            locks.shared(Key::Project(project))?;
        }
        Ok(())
    }

    /// # Errors
    /// As `make_from_template`.
    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        log: &mut Compensation,
    ) -> Result<Arc<SessionLoop>, SessionError> {
        let Self { context, id, template_id, spec } = self;
        record_creation(log, sessions_of(kernel)?.kernel(), id);
        make_from_template(log, kernel, &context, id, template_id, spec).await
    }
}

/// Records in `log` how creating session `id` in `kernel` is rolled back (`roll_back_creation`) -
/// before it is made: a session not made yet is not there to delete or purge, and what storing it
/// made of it is taken back by storing it already (`persist`). For every procedure that creates a
/// session.
pub(crate) fn record_creation(log: &mut Compensation, kernel: &Arc<Kernel>, id: SessionId) {
    let kernel = Arc::clone(kernel);
    log.record("creating the session", async move { roll_back_creation(&kernel, id).await });
}

/// Rolls back creating session `id`, not to be kept - not complete -, in the kernel's name -
/// making it was permitted: deleted and purged, with what was made in its scope.
///
/// # Errors
/// What deleting or purging it reports.
async fn roll_back_creation(kernel: &Kernel, id: SessionId) -> Result<()> {
    let system = SecurityContext::system();
    let removal = Removal::now(system.caller_id);
    all_or_nothing("deleting a session not made", async |log: &mut Compensation| {
        delete(log, kernel, &system, id, &removal).await
    })
    .await?;
    purge(kernel, &system, id).await?;
    Ok(())
}

/// Creates session `id` from the template `template_id` as `spec` asks, on behalf of `context`,
/// whose user owns it and is the template's "user", and starts it (issue 88), for `Create`, which
/// holds it until it is complete and rolls its creation back if any of it fails. Asked first, in
/// the order of issue 312: what `spec` names is checked - its name, language and time zone -, the
/// template read, then creating the session (`CreateSession`) before anything of others is asked
/// or anything is found or made for it, so that who may not create it learns nothing of the
/// agents, the workspace or anyone's clearance; then one of the user's own workspaces that `spec`
/// chooses, which is attached as the session is made. Then the session is built and established
/// (`establish`) - a channel whose components the owner cannot read is left out as a session
/// opens (issue 463), but making it is the owner's own request, refused as creating a channel
/// refuses it -, and completed (`complete`): the workspace to be found or made - one `spec` chooses
/// to be made for it alone, else the template's - found or made and attached, once the session is,
/// so that one of its scope is made where `context` may put it (issue 344), and the rest. A
/// workspace made for it that is not in its scope - which goes with the session as its creation is
/// rolled back - is deleted here if completing it fails.
///
/// # Errors
/// `SessionError::OwnerNotAUser` if `context` acts for no user; a `StoreError` if the template, a
/// user or a personality it names does not exist or may not be read; a `ValueError` if the name
/// `spec` asks for is no name, its language no language tag or its time zone no time zone; a
/// `TemplateError` if the template cannot be made into a session; what attaching an existing
/// workspace, making one or sharing one reports; `SessionError::ChannelUnavailable` if the
/// components of one of its channels cannot be read in the owner's name (issue 463); as
/// `establish` and `complete`.
async fn make_from_template(
    log: &mut Compensation,
    kernel: &Kernel,
    context: &SecurityContext,
    id: SessionId,
    template_id: SessionTemplateId,
    spec: SessionSpec,
) -> Result<Arc<SessionLoop>, SessionError> {
    let owner = context
        .principal_id
        .as_user_id()
        .ok_or(SessionError::OwnerNotAUser(context.principal_id))?;
    // Before anything is found or made for it.
    let name = Name::optional(spec.name.as_deref()).map_err(Error::from)?;
    let language = Language::optional(spec.language.as_deref()).map_err(Error::from)?;
    let time_zone = TimeZone::optional(spec.time_zone.as_deref()).map_err(Error::from)?;
    let stores = kernel.entity_stores();
    let template =
        stores.session_templates.get(context, template_id).await.map_err(|e| match e {
            StoreError::NotFound { .. } => SessionError::TemplateNotFound(template_id),
            e => SessionError::Store(e),
        })?;
    let attributes = Session::attributes_of(id, owner, &template.access);
    let creating = ActionType::create(Session::KIND, &attributes);
    kernel.access_policy().require(context, creating, StoreError::PermissionDenied).await?;
    let workspaces = kernel.workspace_manager();
    let existing = match &spec.workspace {
        Some(WorkspaceChoice::Existing { id: chosen, .. }) => {
            require_attachable(workspaces, context, *chosen, id).await?;
            Some(*chosen)
        }
        _ => None,
    };
    let (users, agents) = resolve_participants(stores, context, &template, id, owner).await?;
    let name = name.unwrap_or_else(|| template.name.name.clone());
    let built = template.build(id, owner, name, language, time_zone, users.clone());
    let (mut session, own, channels) = built.map_err(Error::from)?;
    session.workspace = existing;
    let session_loop = establish(kernel, context, session, own, channels).await?;
    for (channel, run) in session_loop.listed_channels() {
        session_loop.running(channel.id, run)?;
    }
    let wanted = wanted_workspace(&template, &spec, &session_loop);
    let resolved = match &wanted {
        Some(wanted) => Some(resolve_workspace(workspaces, context, wanted, id, owner).await?),
        None => None,
    };
    let made = resolved.and_then(|(workspace, made)| made.then_some(workspace));
    let participants = (users, agents);
    let completed = async {
        if let Some((workspace, _)) = resolved {
            session_loop.attach_workspace(context, Some(workspace)).await?;
        }
        complete(log, kernel, context, &template, &session_loop, &spec, participants).await
    }
    .await;
    if let (Err(_), Some(made)) = (&completed, made) {
        let system = SecurityContext::system();
        let removing = async {
            workspaces.delete(&system, made).await?;
            workspaces.purge(&system, made).await
        };
        if let Err(cleanup) = removing.await {
            warn!(workspace = %made, "The workspace made for a session that could not be created stays: {cleanup}");
        }
    }
    completed?;
    Ok(session_loop)
}

/// The workspace to be found or made for `session`, made from `template` as `spec` asks: one
/// `spec` chooses to be made for it alone, in its scope, else the template's; none where `spec`
/// chooses none or one of the user's own.
fn wanted_workspace(
    template: &SessionTemplate,
    spec: &SessionSpec,
    session: &SessionLoop,
) -> Option<WorkspaceTemplate> {
    match &spec.workspace {
        None => template.workspace.clone(),
        Some(WorkspaceChoice::New { storage, classification }) => Some(WorkspaceTemplate {
            name: session.name(),
            scope: ScopeTemplate::SessionScope,
            storage: storage.clone(),
            classification: classification.clone(),
            description: None,
        }),
        Some(WorkspaceChoice::None | WorkspaceChoice::Existing { .. }) => None,
    }
}

/// Completes `session`, just made from `template` as `spec` asks on behalf of `context`, its
/// workspace attached: the template's agents found or made and taken in (issues 227, 344), and its
/// users and agents - `participants`, as `resolve_participants` gives them - taken into the
/// channels that name them as their founders (issue 420); then one of the user's own workspaces
/// that `spec` chooses shared with its participants as far as it says (issue 23), once the session
/// it is shared with exists, and the session put into the shared project `spec` names by the one
/// way into a project (`move_to`, issue 467).
///
/// # Errors
/// What finding or making the agents reports, or taking them in, sharing the workspace or putting
/// the session into the project.
async fn complete(
    log: &mut Compensation,
    kernel: &Kernel,
    context: &SecurityContext,
    template: &SessionTemplate,
    session: &Arc<SessionLoop>,
    spec: &SessionSpec,
    participants: TemplateParticipants,
) -> Result<(), SessionError> {
    take_in_participants(log, kernel, context, template, session, participants).await?;
    if let Some(WorkspaceChoice::Existing { id, share: Some(grant) }) = &spec.workspace {
        let controller = kernel.controller_manager().workspace_controller(*id).await?;
        let group = Subject::Group(GroupId::of_session(session.id()));
        controller.share(context, group, *grant).await.map_err(Error::from)?;
    }
    if let Some(project) = spec.project {
        move_to(log, kernel, session, context, Some(project)).await?;
    }
    Ok(())
}

/// Establishes the brand-new `session` with its own project `own` and its `channels`, whose
/// creation `context` was asked for (`CreateSession`) before anything was found or made for it,
/// for a procedure that holds it: held for this process before anything is stored, so that what
/// cannot hold it leaves nothing behind (issue 554), stored (`persist`), which announces each as it
/// is made, then started (`SessionManager::start`). The channels are empty (issue 420): their
/// founders join once the session runs, each admitted as they join
/// (`procedure::participant::take_in`).
///
/// # Errors
/// `SessionError::RunsElsewhere` if another process holds it; as `persist`; whatever starting the
/// session reports.
pub(crate) async fn establish(
    kernel: &Kernel,
    context: &SecurityContext,
    mut session: Session,
    mut own: Project,
    mut channels: Vec<Channel>,
) -> Result<Arc<SessionLoop>, SessionError> {
    let sessions = sessions_of(kernel)?;
    let hold = sessions.hold(session.id)?;
    persist(kernel, context, &mut session, &mut own, &mut channels).await?;
    Ok(sessions.start(session, channels, hold).await?)
}

/// Stores the brand-new `session` with its own project `own` and its `channels`, whose creation
/// `context` was asked for (`CreateSession`), all of it or nothing (`all_or_nothing`): the channels
/// as documents of their own (issue 70), the session and each channel with the group of its
/// participants (issue 286), which it is read by, and the project with the group of its members, in
/// the kernel's name, as the groups are (issue 467). The classifications are validated first; the
/// store asks to create the session again and each channel as it stores them. Each is announced as
/// it is made (`Lifecycle`), its group made before it, so that whoever the group lets read it hears
/// of it. If a step fails, what was stored is taken back, the last first, in the kernel's name -
/// the session's creation was permitted -: marked deleted again, announced so, and purged.
///
/// # Errors
/// `SessionError::ClassificationRefused` for a classification no channel may be given; whatever
/// the store reports.
pub(crate) async fn persist(
    kernel: &Kernel,
    context: &SecurityContext,
    session: &mut Session,
    own: &mut Project,
    channels: &mut [Channel],
) -> Result<(), SessionError> {
    let sessions = sessions_of(kernel)?;
    for channel in channels.iter() {
        require_classification(kernel.confidentiality().as_ref(), &channel.classification)?;
    }
    let groups = kernel.group_manager();
    all_or_nothing("storing a new session with its project and channels", async |log| {
        // Each group before what it lets be read, so that its members hear it made.
        groups.keep_participants(Group::of_session(session)).await?;
        record_discarding_group(log, groups, GroupId::of_session(session.id));
        sessions.session_lifecycle().create(context, session).await?;
        {
            let (sessions, made) = (Arc::clone(&sessions), session.clone());
            log.record("storing the session", async move {
                Ok(sessions.session_lifecycle().discard(&made).await?)
            });
        }
        let projects = kernel.project_manager();
        projects.make_own(own).await?;
        {
            let (projects, made) = (Arc::clone(projects), own.clone());
            log.record("making a session's own project", async move {
                Ok(projects.discard_own(&made).await?)
            });
        }
        for channel in channels.iter_mut() {
            groups.keep_participants(Group::of_channel(channel)).await?;
            record_discarding_group(log, groups, GroupId::of_channel(channel.id));
            sessions.channel_lifecycle().create(context, channel).await?;
            let (sessions, made) = (Arc::clone(&sessions), channel.clone());
            log.record("storing a session's channel", async move {
                Ok(sessions.channel_lifecycle().discard(&made).await?)
            });
        }
        Ok(())
    })
    .await
}

/// Records in `log` how keeping the group `id` is taken back: discarded, announced so, as it was
/// announced made (`GroupManager::discard_participants`).
pub(super) fn record_discarding_group(
    log: &mut Compensation,
    groups: &Arc<GroupManager>,
    id: GroupId,
) {
    let groups = Arc::clone(groups);
    log.record("keeping a group", async move { Ok(groups.discard_participants(id).await?) });
}

/// Opens session `id` for `context`: the running one, or loaded from the store and started - which
/// announces it as `session::STARTED`, not as created, as the session exists already. Started
/// once, however many ask at the same time (issue 62): who comes second waits for the first and
/// gets the session it started - if they may read it. A channel whose components cannot be read is
/// left out as the session starts, and built again whenever the running session is opened
/// (`channel::Start`, issue 463).
pub struct Open {
    context: SecurityContext,
    id: SessionId,
}

impl Open {
    /// Opens session `id` for `context`.
    pub fn new(context: &SecurityContext, id: SessionId) -> Self {
        Self { context: context.clone(), id }
    }
}

#[async_trait]
impl Procedure for Open {
    type Plan = ();
    type Output = Arc<SessionLoop>;
    type Error = SessionError;

    async fn enumerate(&self, kernel: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(kernel, locks, self.id)
    }

    /// # Errors
    /// As `open`.
    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        _: &mut Compensation,
    ) -> Result<Arc<SessionLoop>, SessionError> {
        open(kernel, &self.context, self.id).await
    }
}

/// Locks session `id` to open it, as every procedure that opens a session does (`Open`, and who
/// takes someone in or lets them go): shared where it runs - which keeps it running -, exclusively
/// where it is to be started.
///
/// # Errors
/// `Deferred` if it is held otherwise.
pub(super) fn lock_to_open(
    kernel: &Kernel,
    locks: &mut Locks<'_>,
    id: SessionId,
) -> Result<(), Deferred> {
    let key = Key::Session(id);
    let runs = || kernel.sessions().is_some_and(|sessions| sessions.running(id).is_some());
    if runs() {
        locks.shared(key)?;
        if runs() {
            return Ok(());
        }
    }
    locks.exclusive(key)
}

/// Session `id` for `context`, for a procedure that holds it as `lock_to_open` locks it -
/// exclusively where it does not run: the running one - whose channels left out as it started
/// whoever opens it has built again (`procedure::channel::Start`, issue 463) -, or loaded from the
/// store and started (`SessionManager::start`) - which announces it as `session::STARTED`, not as
/// created, as the session exists already -, its channels loaded in the kernel's name: starting a
/// session the caller may read follows from an action already permitted, and it must not fail
/// because the caller is not in one of its channels. An agent deleted or restored while its
/// channels were built the follower missed, as the session ran nowhere yet: its channels follow
/// their agents as it starts (`SessionLoop::follow_agents`).
///
/// # Errors
/// `SessionError::NotFound` if there is no such session or `context` may not read it,
/// `SessionError::RunsElsewhere` if another process that shares the stores runs it (issue 554);
/// whatever loading or starting it reports otherwise.
pub(crate) async fn open(
    kernel: &Kernel,
    context: &SecurityContext,
    id: SessionId,
) -> Result<Arc<SessionLoop>, SessionError> {
    let sessions = sessions_of(kernel)?;
    let session = match sessions.find(context, id).await? {
        Found::Running(running) => return Ok(running),
        Found::Stored(session) => session,
    };
    let hold = sessions.hold(id)?;
    let system = SecurityContext::system();
    let mut channels = Vec::with_capacity(session.channels.len());
    for channel in &session.channels {
        channels.push(kernel.entity_stores().channels.get(&system, *channel).await?);
    }
    let started = sessions.start(session, channels, hold).await?;
    started.follow_agents(None).await;
    Ok(started)
}

/// Session `id`, to take someone into it on behalf of `context`, for a procedure that holds it as
/// `open` says: opened in the kernel's name if an entry of its access list lets `context` join it,
/// or they may read it - who may join reads nothing of it before they did.
///
/// # Errors
/// `SessionError::NotFound` if there is no such session or `context` may neither join nor read
/// it; as `open` otherwise.
pub(crate) async fn open_to_take_in(
    kernel: &Kernel,
    context: &SecurityContext,
    id: SessionId,
) -> Result<Arc<SessionLoop>, SessionError> {
    let not_found = || SessionError::NotFound(id);
    let system = SecurityContext::system();
    let found = sessions_of(kernel)?.find(&system, id).await.map_err(|_| not_found())?;
    let attributes = found.attributes();
    let access = kernel.access_policy();
    let reachable =
        access.check(context, ActionType::custom(Session::KIND, &JOIN, &attributes)).await
            || access.may_read_session(context, &attributes).await;
    if !reachable {
        return Err(not_found());
    }
    open(kernel, &system, id).await
}

/// Stops session `id` on behalf of `context` without deleting it: its channels and actors stop,
/// for everybody in it (`StopSession`) - each actor keeps what it is in the middle of, to be
/// resumed when the session runs again, and is given up to `STOP_GRACE` to store it (issue 377) -,
/// and the commands of its channels end (issue 254). Answers whether it ran.
pub struct Stop {
    context: SecurityContext,
    id: SessionId,
}

impl Stop {
    /// Stops session `id` on behalf of `context`.
    pub fn new(context: &SecurityContext, id: SessionId) -> Self {
        Self { context: context.clone(), id }
    }
}

#[async_trait]
impl Procedure for Stop {
    type Plan = ();
    type Output = bool;
    type Error = SessionError;

    /// Exclusively, so that a start under way is not missed (issue 62).
    async fn enumerate(&self, _: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        locks.exclusive(Key::Session(self.id))
    }

    /// Asked whether it runs or not.
    ///
    /// # Errors
    /// `SessionError::NotFound` if there is no such session or `context` may not read it,
    /// `StoreError::PermissionDenied` if they may read but not stop it.
    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        _: &mut Compensation,
    ) -> Result<bool, SessionError> {
        let Self { context, id } = self;
        let sessions = sessions_of(kernel)?;
        let found = sessions.find(&context, id).await?;
        let stopping = ActionType::StopSession(&found.attributes());
        kernel.access_policy().require(&context, stopping, StoreError::PermissionDenied).await?;
        if !matches!(found, Found::Running(_)) {
            return Ok(false);
        }
        let Some(stopped) = sessions.stop_running(id, Ending::Stop).await else { return Ok(false) };
        kernel.command_reaper().end_commands_in(&stopped.session().channels).await;
        Ok(true)
    }
}

/// Stops and deletes session `id` on behalf of `context` (`delete`): marked, with what goes with
/// it, until it is restored or purged. Answers `false` if there is no such session or `context` may
/// not read it, as every deletion answers (issue 510).
pub struct Delete {
    context: SecurityContext,
    id: SessionId,
}

impl Delete {
    /// Deletes session `id` on behalf of `context`.
    pub fn new(context: &SecurityContext, id: SessionId) -> Self {
        Self { context: context.clone(), id }
    }
}

#[async_trait]
impl Procedure for Delete {
    type Plan = ();
    type Output = bool;
    type Error = SessionError;

    async fn enumerate(&self, kernel: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_with_its_project(kernel, locks, self.id).await
    }

    /// # Errors
    /// As `delete`.
    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        log: &mut Compensation,
    ) -> Result<bool, SessionError> {
        let Self { context, id } = self;
        let removal = Removal::now(context.caller_id);
        delete(log, kernel, &context, id, &removal).await
    }
}

/// Restores the deleted session `id` on behalf of `context` (`restore`, issue 164); it is not
/// started.
pub struct Restore {
    context: SecurityContext,
    id: SessionId,
}

impl Restore {
    /// Restores session `id` on behalf of `context`.
    pub fn new(context: &SecurityContext, id: SessionId) -> Self {
        Self { context: context.clone(), id }
    }
}

#[async_trait]
impl Procedure for Restore {
    type Plan = ();
    type Output = Session;
    type Error = SessionError;

    async fn enumerate(&self, kernel: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_with_its_project(kernel, locks, self.id).await
    }

    /// # Errors
    /// As `restore`.
    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        log: &mut Compensation,
    ) -> Result<Session, SessionError> {
        restore(log, kernel, &self.context, self.id).await
    }
}

/// Stops and deletes session `id` on behalf of `context`, marked with `removal` - its own, or the
/// mark of a deletion it goes with: of the project it lies in (issue 469) -, for a procedure that
/// holds it exclusively until it is gone from the store, so that nothing starts it again from there
/// meanwhile (issue 62). Asked before anything happens (`to_delete`): a refused deletion must not
/// stop the session on the way (issue 11). Deleting marks (issue 164): the session is kept, with
/// its channels and their state, hidden until it is restored or purged - a channel is reached only
/// through its session, so they are not announced deleted. Marked before what lies in its scope, so
/// that nothing of it goes unless its deletion was granted, and nothing is put into its scope
/// afterwards (`ScopeHolders`, issue 458). Its own project is marked with it, in the kernel's name
/// (issues 467, 458); then what lies in its scope and in its own project's is deleted, as far as
/// `context` may (issue 320), and then the groups of its participants, its channels' and its
/// project's members, in the kernel's (issue 348) - last, since its participants reach it by them
/// while it is kept -, all of it with the session's mark, by `context`'s caller (issue 469). If
/// something is left, or the project or the groups cannot go, `log` takes the marks away again, in
/// the kernel's name - the deletion was permitted -, and the session runs on if it ran, without
/// what was deleted. Once all of it went, its actors leave, as those of a deleted channel do (issue
/// 377), and the commands of its channels end (issue 254). Each mark is announced as it is set or
/// taken away (`SessionManager::session_lifecycle`, `ProjectManager::lifecycle`). `false` if there
/// is no such session or `context` may not read it, as every deletion answers (issue 510).
///
/// # Errors
/// As `to_delete`; `SessionError::RunsElsewhere` if another process that shares the stores runs it
/// (issue 554), `SessionError::ScopeNotEmpty` if something of its scope or its own project's is
/// left that `context` may not delete; whatever else the stores report.
pub(crate) async fn delete(
    log: &mut Compensation,
    kernel: &Kernel,
    context: &SecurityContext,
    id: SessionId,
    removal: &Removal,
) -> Result<bool, SessionError> {
    let sessions = sessions_of(kernel)?;
    let Some((session, own)) = to_delete(kernel, &sessions, context, id).await? else {
        return Ok(false);
    };
    // Held against the other processes until the procedure - or its part - is done, undone too
    // (issue 554).
    log.keep(sessions.hold(id)?);
    if !sessions.session_lifecycle().delete(context, &session, removal).await? {
        return Ok(false);
    }
    {
        let sessions = Arc::clone(&sessions);
        log.record("marking the session deleted", async move {
            let system = SecurityContext::system();
            Ok(sessions.session_lifecycle().restore(&system, id).await.map(drop)?)
        });
    }
    if let Some(own) = &own {
        let projects = kernel.project_manager();
        projects.lifecycle().delete(&SecurityContext::system(), own, removal).await?;
        let (projects, own) = (Arc::clone(projects), own.id);
        log.record("marking the session's own project deleted", async move {
            let system = SecurityContext::system();
            Ok(projects.lifecycle().restore(&system, own).await.map(drop)?)
        });
    }
    let holders = held_by(id, own.as_ref());
    let left = kernel.scoped_entities().delete_lying_in(context, &holders, removal).await?;
    ScopeNotEmpty::require_empty(&Scope::SessionScope { session_id: id }, left)?;
    kernel.group_manager().delete_participants(&holders, removal).await?;
    if let Some(stopped) = sessions.stop_running(id, Ending::Leave).await {
        kernel.command_reaper().end_commands_in(&stopped.session().channels).await;
    }
    info!(session = %id, by = %context.caller_id, "Session deleted");
    Ok(true)
}

/// Session `id`, to be deleted on behalf of `context` (`delete`), with its own project (issue 467),
/// as both are before they go - the events carry the attributes as they were, as once deleted they
/// are nowhere to look up (issue 39) -, once `context` may delete it. `None` if there is no such
/// session or `context` may not read it, as every deletion answers (issue 510).
///
/// # Errors
/// `StoreError::PermissionDenied` if they may read but not delete it; whatever the store reports.
async fn to_delete(
    kernel: &Kernel,
    sessions: &SessionManager,
    context: &SecurityContext,
    id: SessionId,
) -> Result<Option<(Session, Option<Project>)>, SessionError> {
    let session = match sessions.find(context, id).await {
        Ok(found) => found.session(),
        Err(error) if matches!(error.downcast_ref(), Some(SessionError::NotFound(_))) => {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    let deleting = ActionType::delete(Session::KIND, &session.security_attributes);
    kernel.access_policy().require(context, deleting, StoreError::PermissionDenied).await?;
    let own = kernel.project_manager().load_own(id).await?;
    Ok(Some((session, own)))
}

/// Restores the deleted session `id` on behalf of `context`, for a procedure that holds it
/// exclusively, as deleting and purging it do: a purge never meets it half restored (`to_restore`,
/// issue 164). The groups of its participants and theirs first, in the kernel's name (issues 348,
/// 467), so that whoever they let read the session hears it restored; then its mark taken away, and
/// its own project's, so that whatever fails later its participants reach it and its results have
/// their place; if something cannot come back, `log` takes the groups away and marks the session
/// again as it was, in the kernel's name, so that a restoring tried again finds what went with it
/// (issue 469). Then what lay in its scope and its own project's comes back, as far as `context`
/// may restore it; what cannot be is logged and stays deleted - the session comes back all the
/// same. Each mark is announced as it is taken away or set again
/// (`SessionManager::session_lifecycle`, `ProjectManager::lifecycle`). It is not started; opening
/// it starts it, as any stored session.
///
/// # Errors
/// As `to_restore`; `SessionError::RunsElsewhere` if another process that shares the stores holds
/// it (issue 554); `SessionError::NotFound` if there is no such deleted session or `context` may
/// not see it as its mark is taken away, `StoreError::PermissionDenied` if they may see but not
/// restore it; whatever the store reports as the groups of its participants or its own project are
/// restored.
pub(crate) async fn restore(
    log: &mut Compensation,
    kernel: &Kernel,
    context: &SecurityContext,
    id: SessionId,
) -> Result<Session, SessionError> {
    let sessions = sessions_of(kernel)?;
    let (removal, own) = to_restore(kernel, context, id).await?;
    // Held against the other processes until the procedure - or its part - is done, undone too
    // (issue 554).
    log.keep(sessions.hold(id)?);
    // The groups first, so that whoever they let read the session and its own project hears them
    // restored.
    let holders = held_by(id, own.as_ref());
    let groups = kernel.group_manager();
    groups.restore_participants(&holders, &removal).await?;
    {
        let (groups, holders, again) = (Arc::clone(groups), holders.clone(), removal.clone());
        log.record("restoring the groups of the session's participants", async move {
            Ok(groups.delete_participants(&holders, &again).await?)
        });
    }
    let restoring = sessions.session_lifecycle().restore(context, id);
    let session = restoring.await.map_err(store_error(id))?;
    {
        let (sessions, restored, again) = (Arc::clone(&sessions), session.clone(), removal.clone());
        log.record("taking the session's mark away", async move {
            let system = SecurityContext::system();
            Ok(sessions.session_lifecycle().delete(&system, &restored, &again).await.map(drop)?)
        });
    }
    // The last step that can fail: nothing after it is undone.
    if let Some(own) = &own {
        let projects = kernel.project_manager().lifecycle();
        projects.restore(&SecurityContext::system(), own.id).await?;
    }
    if let Err(e) = kernel.scoped_entities().restore_lying_in(context, &holders, &removal).await {
        warn!(
            "Session {} was restored, but not what lies in its scope or its project's: {}",
            id, e
        );
    }
    info!(session = %id, by = %context.caller_id, "Session restored");
    Ok(session)
}

/// The deleted session `id`, to be restored on behalf of `context` (`restore`): its mark - what was
/// deleted with it carries it and comes back with it (issues 178, 469) -, with its own project
/// deleted with it, once `context` may restore it. A session that lies in a shared project is not
/// restored while the project is deleted: it comes back with the project.
///
/// # Errors
/// `SessionError::NotFound` if there is no such deleted session, `context` may not see it
/// (`ReadDeleted`, asked first, issue 304) or its purge is begun (issue 496),
/// `SessionError::ProjectDeleted` if the shared project it lies in is deleted,
/// `StoreError::PermissionDenied` if they may see but not restore it (`Restore`, asked last);
/// whatever else the store reports.
async fn to_restore(
    kernel: &Kernel,
    context: &SecurityContext,
    id: SessionId,
) -> Result<(Removal, Option<Project>), SessionError> {
    let stores = kernel.entity_stores();
    let system = SecurityContext::system();
    // As `context`, so that whoever may not see it learns nothing more.
    let Deleted { entity: deleted, removal } =
        stores.sessions.get_deleted(context, id).await.map_err(store_error(id))?;
    // One whose purge is begun is the store's to refuse, as it would; refused before anything else
    // is asked, so that its own project, purged already, is not taken for a shared one deleted.
    if deleted.purging() {
        return Err(SessionError::NotFound(id));
    }
    let own = stores.projects.list_deleted_with(&system, removal.id, &lying_in(id)).await?;
    let own = own.items.into_iter().last();
    let lies_in_own = own.as_ref().is_some_and(|own| own.id == deleted.project);
    if !lies_in_own && stores.projects.get(&system, deleted.project).await.optional()?.is_none() {
        return Err(SessionError::ProjectDeleted(deleted.project));
    }
    // Asked ahead, as the store asks again as it restores it: the groups of its participants come
    // back before it, and are not to come back for whoever may not restore it.
    let restoring = ActionType::restore(Session::KIND, &deleted.security_attributes);
    kernel.access_policy().require(context, restoring, StoreError::PermissionDenied).await?;
    Ok((removal, own))
}

/// Removes the deleted session `id` for good on behalf of `context` (`purge`, issue 164). Answers
/// `false` if there was nothing deleted the caller could see, as every purge answers (issue 510).
pub struct Purge {
    context: SecurityContext,
    id: SessionId,
}

impl Purge {
    /// Purges session `id` on behalf of `context`.
    pub fn new(context: &SecurityContext, id: SessionId) -> Self {
        Self { context: context.clone(), id }
    }
}

#[async_trait]
impl Procedure for Purge {
    type Plan = ();
    type Output = bool;
    type Error = SessionError;

    async fn enumerate(&self, kernel: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_with_its_project(kernel, locks, self.id).await
    }

    /// # Errors
    /// As `purge`.
    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        _: &mut Compensation,
    ) -> Result<bool, SessionError> {
        purge(kernel, &self.context, self.id).await
    }
}

/// Removes the deleted session `id` for good on behalf of `context` (issue 164), for a procedure
/// that holds it exclusively, as deleting and restoring it do, so that a purge never meets it half
/// deleted or half restored - and the project it lies in, as purging a project purges the sessions
/// in it (`procedure::project::Purge`). In the order of every purge of what has something behind it
/// (`Lifecycle::purge`): its purge begun, then - in the kernel's name, since the purge was
/// permitted - what lies behind it (`remove_behind`), then its document. Only what was deleted is
/// purged; the store asks `Purge`. A step that fails leaves the session deleted, its purge begun,
/// so that the next purge removes what is left (issue 496). `false` if there was nothing deleted
/// the caller could see, as every purge answers (issue 510).
///
/// # Errors
/// `StoreError::PermissionDenied` if they may see it but not purge it,
/// `SessionError::RunsElsewhere` if another process that shares the stores holds it (issue 554),
/// or whatever the stores report.
pub(crate) async fn purge(
    kernel: &Kernel,
    context: &SecurityContext,
    id: SessionId,
) -> Result<bool, SessionError> {
    let sessions = sessions_of(kernel)?;
    // What the caller may not see, or what is not deleted, is not there to purge: asked before
    // whether another process holds it, which they learn of only for what they see.
    let deleted = kernel.entity_stores().sessions.get_deleted(context, id).await.optional()?;
    if deleted.is_none() {
        return Ok(false);
    }
    let held = sessions.hold(id)?;
    let removing = async |session: &Session| remove_behind(kernel, &sessions, session).await;
    let purged = sessions.session_lifecycle().purge(context, id, removing).await?;
    if let (true, Some(locks)) = (purged, kernel.session_locks()) {
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

/// Removes what lies behind `session` once its purge is begun, in the kernel's name: the state of
/// its channels, those it lists and those deleted from it before, the content of their attachments
/// (issue 282) and then their documents; what lay in its scope and in its channels' (issue 178),
/// the group of its participants (issue 348), and its own project with what lies in its scope
/// (issue 467). What a purge that failed before removed already is passed over.
///
/// # Errors
/// Whatever the stores report.
async fn remove_behind(
    kernel: &Kernel,
    sessions: &SessionManager,
    session: &Session,
) -> Result<()> {
    let id = session.id;
    let system = SecurityContext::system();
    let stores = kernel.entity_stores();
    let mut channels: Vec<ChannelId> = session.channels.clone();
    let of_it = Query::filtered(Filter::eq(ChannelField::Session, id));
    let deleted = stores.channels.list_deleted(&system, &of_it).await?.items;
    channels.extend(deleted.into_iter().map(|channel| channel.entity.id));
    // The state lives in the `StateStore`, which may be another store than the session's; the
    // attachments its history carries go with it. Both before the document, which is what finds
    // them.
    for channel in channels {
        kernel.state_store().delete_channel_state(channel).await?;
        kernel.attachment_manager().purge(channel).await?;
        sessions.purge_channel(channel).await?;
    }
    let owns = stores.projects.list_deleted(&system, &lying_in(id)).await?.items;
    let owns: Vec<Project> = owns.into_iter().map(|own| own.entity).collect();
    kernel.scoped_entities().purge_lying_in(&held_by(id, &owns)).await?;
    kernel.group_manager().purge_participants(GroupId::of_session(id)).await?;
    for own in &owns {
        kernel.project_manager().purge_own(own.id).await?;
    }
    Ok(())
}

/// Locks session `id` exclusively, and the project it lies in - deleted or not - shared, as read
/// once the session is locked (`project_of`): deleting, restoring and purging a project enumerate
/// the sessions in it, so that every procedure that changes whether a session is in a project, or
/// whether it is deleted, locks that project (`procedure::project`). A project that cannot be read
/// is not locked: carrying the procedure out then fails at the session.
async fn lock_with_its_project(
    kernel: &Kernel,
    locks: &mut Locks<'_>,
    id: SessionId,
) -> Result<(), Deferred> {
    locks.exclusive(Key::Session(id))?;
    if let Some(project) = project_of(kernel, id).await {
        locks.shared(Key::Project(project))?;
    }
    Ok(())
}

/// The project session `id` lies in - running, stored or deleted -, read in the kernel's name;
/// `None` if there is no such session, or it cannot be read.
async fn project_of(kernel: &Kernel, id: SessionId) -> Option<ProjectId> {
    if let Some(running) = kernel.sessions().and_then(|sessions| sessions.running(id)) {
        return Some(running.session().project);
    }
    let system = SecurityContext::system();
    let sessions = &kernel.entity_stores().sessions;
    match sessions.get(&system, id).await {
        Ok(session) => Some(session.project),
        Err(_) => {
            sessions.get_deleted(&system, id).await.ok().map(|deleted| deleted.entity.project)
        }
    }
}

/// The scopes session `id` holds, and what lies in them goes with it: its own, with its channels',
/// and the scope of `own`, its own project, while it is there (issue 467).
fn held_by<'a>(id: SessionId, own: impl IntoIterator<Item = &'a Project>) -> Vec<Scope> {
    iter::once(Scope::SessionScope { session_id: id })
        .chain(own.into_iter().map(Project::holds))
        .collect()
}

/// What lies in the scope of session `id`: of projects, its own.
fn lying_in(id: SessionId) -> Query<ScopeField> {
    Query::filtered(Filter::lying_within(&Scope::SessionScope { session_id: id }))
}

/// Puts session `id` into the shared project `project` on behalf of `context`, or with `None` into
/// a project of its own again (`move_to`, issue 467) - starting it, if it does not run. Locks the
/// project it goes to shared, so that it is not deleted meanwhile: deleting a project enumerates
/// the sessions that lie in it, and one put into it meanwhile would be missed. The project it
/// leaves needs no lock: deleting that one locks the session, and enumerates anew once it is moved.
pub struct Move {
    context: SecurityContext,
    id: SessionId,
    project: Option<ProjectId>,
}

impl Move {
    /// Puts session `id` into `project` on behalf of `context`, or into its own with `None`.
    pub fn new(context: &SecurityContext, id: SessionId, project: Option<ProjectId>) -> Self {
        Self { context: context.clone(), id, project }
    }
}

#[async_trait]
impl Procedure for Move {
    type Plan = ();
    type Output = ();
    type Error = SessionError;

    async fn enumerate(&self, _: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        locks.exclusive(Key::Session(self.id))?;
        if let Some(project) = self.project {
            locks.shared(Key::Project(project))?;
        }
        Ok(())
    }

    /// # Errors
    /// As `open`, and as `move_to`.
    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        log: &mut Compensation,
    ) -> Result<(), SessionError> {
        let Self { context, id, project } = self;
        let session = open(kernel, &context, id).await?;
        move_to(log, kernel, &session, &context, project).await
    }
}

/// Puts `session` into the shared project `project` on behalf of `context`, or with `None` takes it
/// out into its own (issue 467), for a procedure that holds the session exclusively and the project
/// it goes to shared - `Move`, and the creation of a session (issue 593). Asked first
/// (`project_to_move_to`); nothing where it lies already. The project it goes to is shared with its
/// participants for reading, or its new own made, before the session is stored as lying there
/// (`SessionController::update_project`, which asks `UpdateSession`), so that it never lies where
/// its participants read nothing - taken back by `log` if storing it fails, a new own marked
/// deleted again, announced so as it was announced made, and purged -; then the shared project it
/// left has the entry of its participants taken away, in the kernel's name - what cannot be is
/// logged. Nothing moves: what went to the project before stays there, only what comes now goes to
/// the other.
///
/// # Errors
/// As `project_to_move_to`; whatever making, sharing or storing reports.
pub(crate) async fn move_to(
    log: &mut Compensation,
    kernel: &Kernel,
    session: &Arc<SessionLoop>,
    context: &SecurityContext,
    project: Option<ProjectId>,
) -> Result<(), SessionError> {
    let found = project_to_move_to(kernel, session, context, project).await?;
    let Some(mut target) = found else {
        return Ok(());
    };
    let controllers = kernel.controller_manager();
    let system = SecurityContext::system();
    let lying_in = session.session().project;
    let left = kernel.entity_stores().projects.get(&system, lying_in).await.optional()?;
    let left = left.filter(|left| !left.is_own());
    match &mut target {
        MoveTarget::NewOwn(own) => {
            let projects = kernel.project_manager();
            projects.make_own(own).await?;
            let (projects, own) = (Arc::clone(projects), own.clone());
            log.record("making a session's own project", async move {
                Ok(projects.discard_own(&own).await?)
            });
        }
        MoveTarget::Shared(shared) => {
            let participants = Subject::Group(GroupId::of_session(session.id()));
            let controller = controllers.project_controller(shared.id).await?;
            let sharing = controller.share(&system, participants, Grant::Read);
            sharing.await.map_err(Error::from)?;
            log.record("sharing a project with a session's participants", async move {
                Ok(controller.unshare(&SecurityContext::system(), participants).await?)
            });
        }
        MoveTarget::Own(_) => {}
    }
    let to = target.project().id;
    session.controller().update_project(context, to).await?;
    if let Some(left) = left {
        let participants = Subject::Group(GroupId::of_session(session.id()));
        let unsharing = async {
            let controller = controllers.project_controller(left.id).await?;
            Ok::<_, anyhow::Error>(controller.unshare(&system, participants).await?)
        };
        if let Err(e) = unsharing.await {
            warn!(session = %session.id(), project = %left.id, "The project a session left is still shared with its participants: {e:#}");
        }
    }
    info!(session = %session.id(), project = %to, by = %context.caller_id, "Session's project changed");
    Ok(())
}

/// The project a session is to be moved into (`project_to_move_to`): what moving it there does
/// besides storing it as lying there differs by it.
#[derive(Debug)]
enum MoveTarget {
    /// A shared project, shared with the session's participants for reading first.
    Shared(Project),
    /// Its own, which its participants read already.
    Own(Project),
    /// A new own project, made first - the one in its scope was lifted (issue 467).
    NewOwn(Project),
}

impl MoveTarget {
    /// The project it names.
    fn project(&self) -> &Project {
        match self {
            MoveTarget::Shared(project)
            | MoveTarget::Own(project)
            | MoveTarget::NewOwn(project) => project,
        }
    }
}

/// The project `session` is to be put into on behalf of `context` - the shared project `project`,
/// or with `None` its own (issue 467): the one in its scope, or a new one if that was lifted -,
/// once it is asked whether `context` may change the session (`UpdateSession`) and reach the
/// project (`GetProject`); `None` where it lies already. Stores nothing.
///
/// # Errors
/// `StoreError::PermissionDenied` if `context` may not change the session,
/// `ProjectError::NotFound` if there is no project they may read,
/// `SessionError::ProjectOutOfScope` for another session's own, `SessionError::OwnerNotAUser` if a
/// new own is needed and the session is no user's; whatever reading reports.
async fn project_to_move_to(
    kernel: &Kernel,
    session: &SessionLoop,
    context: &SecurityContext,
    project: Option<ProjectId>,
) -> Result<Option<MoveTarget>> {
    let document = session.session();
    let changing = ActionType::update(Session::KIND, &document.security_attributes);
    kernel.access_policy().require(context, changing, StoreError::PermissionDenied).await?;
    let stores = kernel.entity_stores();
    let target = match project {
        Some(id) => {
            let target =
                stores.projects.get(context, id).await.map_err(project::store_error(id))?;
            let theirs = Scope::SessionScope { session_id: session.id() };
            match target {
                own if own.is_own() && own.name.scope != theirs => {
                    return Err(SessionError::ProjectOutOfScope(id).into());
                }
                own if own.is_own() => MoveTarget::Own(own),
                shared => MoveTarget::Shared(shared),
            }
        }
        None => match kernel.project_manager().load_own(session.id()).await? {
            Some(own) => MoveTarget::Own(own),
            None => MoveTarget::NewOwn(
                document
                    .new_own_project()
                    .ok_or(SessionError::OwnerNotAUser(document.security_attributes.owner_id))?,
            ),
        },
    };
    Ok((target.project().id != document.project).then_some(target))
}

/// Stops every session that runs as the process ends (issue 297), as `Stop` stops one, all at
/// once - each actor keeping what it is in the middle of, to be resumed when its session runs
/// again -, then ends, of the commands, only what does not outlive the process
/// (`CommandReaper::shutdown`): an Open Terminal instance keeps its commands, unlike at a stop.
/// The last procedure of the executor (`shut_down`).
struct ShutDown;

#[async_trait]
impl Procedure for ShutDown {
    type Plan = Vec<SessionId>;
    type Output = ();
    type Error = SessionError;

    /// Every session that runs: none starts meanwhile, as the executor admits nothing any more and
    /// waited for what was under way.
    async fn enumerate(
        &self,
        kernel: &Kernel,
        locks: &mut Locks<'_>,
    ) -> Result<Vec<SessionId>, Deferred> {
        let Some(sessions) = kernel.sessions() else { return Ok(Vec::new()) };
        let ids = sessions.running_ids();
        for id in &ids {
            locks.exclusive(Key::Session(*id))?;
        }
        Ok(ids)
    }

    /// Once the session manager is gone, nothing is stopped - its sessions are nowhere to reach -
    /// and that is logged.
    async fn carry_out(
        self,
        ids: Vec<SessionId>,
        kernel: &Kernel,
        _: &mut Compensation,
    ) -> Result<(), SessionError> {
        match kernel.sessions() {
            Some(sessions) => {
                join_all(ids.into_iter().map(|id| sessions.stop_running(id, Ending::Stop))).await;
            }
            None => warn!("The session manager is gone: no session is stopped as the process ends"),
        }
        kernel.command_reaper().shutdown().await;
        Ok(())
    }
}

/// Shuts the sessions down as the process ends (issue 297): `procedures` admits nothing any more,
/// and every session that runs is stopped (`ShutDown`). Returns within `SHUTDOWN_BOUND`, leaving
/// whatever is still under way to end with the process: a shutdown that hangs is worse than none.
pub async fn shut_down(procedures: &ProcedureExecutor) {
    match procedures.shut_down(ShutDown, SHUTDOWN_BOUND).await {
        Some(Ok(())) => info!("Every session is shut down"),
        Some(Err(error)) => warn!("The sessions were shut down, but: {error}"),
        None => warn!(
            "Shutting the sessions down took longer than {:?}; what is still under way ends with \
             the process",
            SHUTDOWN_BOUND
        ),
    }
}

/// Starts again, as the process starts, every session in which an agent's work was under way when
/// it last ended (issue 388) - shut down or crashed -, as the state store's index of the work under
/// way lists them (`interrupted`): the turns stored mid-way go on from the step
/// they stood at, the remote tasks are followed again. Nobody asked for it, so the kernel opens
/// them in its own name (`Open`); what the agents then do is done in their own names, as at every
/// start. One after another, not bounded otherwise. A session that is gone or deleted is passed
/// over; the sessions with no work under way stay stopped until someone acts in them. Ends early
/// once the kernel's executor shuts down. Returns how many it started.
pub async fn resume_interrupted(kernel: &Kernel) -> usize {
    let system = SecurityContext::system();
    let mut started = 0;
    for id in interrupted(kernel).await {
        match kernel.procedures().run(Open::new(&system, id)).await {
            Ok(_) => {
                info!("Session {id} started again: work was under way in it");
                started += 1;
            }
            Err(SessionError::Procedure(error)) => {
                info!("Sessions are no longer started again: {error}");
                break;
            }
            Err(error) => {
                info!("Session {id}, in which work was under way, is not started again: {error}")
            }
        }
    }
    started
}

/// The sessions in which an agent's work was under way when the process last ended (issue 388), as
/// the state store's index of the work under way lists them
/// (`StateStore::channels_with_work_under_way`), read in the kernel's name; a channel that is gone
/// is passed over.
async fn interrupted(kernel: &Kernel) -> Vec<SessionId> {
    let channels = match kernel.state_store().channels_with_work_under_way().await {
        Ok(channels) => channels,
        Err(error) => {
            warn!("The work under way at the last end cannot be read, nothing is started again: {error}");
            return Vec::new();
        }
    };
    let system = SecurityContext::system();
    let mut sessions = HashSet::new();
    for channel_id in channels {
        match kernel.entity_stores().channels.get(&system, channel_id).await {
            Ok(channel) => {
                sessions.insert(channel.session_id);
            }
            Err(error) => {
                info!("Channel {channel_id}, in which work was under way, is passed over: {error}")
            }
        }
    }
    sessions.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::{Delete, Move, Open, Purge, Restore, SHUTDOWN_BOUND};
    use crate::agent::AgentError;
    use crate::channel::STOP_GRACE;
    use crate::command::CommandExecutor;
    use crate::component::BUNDLE_REF_KIND;
    use crate::error::is_forbidden;
    use crate::kernel::Kernel;
    use crate::procedure::follower::AgentFollower;
    use crate::procedure::participant::Follow;
    use crate::procedure::testing::{waits_for, Held};
    use crate::procedure::{Compensation, Deferred, Key, Locks, Procedure, ProcedureError};
    use crate::project::ProjectSpec;
    use crate::session::testing::*;
    use crate::session::SessionLoop;
    use crate::session::{SessionError, SessionManager, SessionSpec};
    use crate::testing::TestKernel;
    use crate::testing::World;
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
    use caelumora_domain::channel::ThreadId;
    use caelumora_domain::channel::{
        self, moderation_components, Channel, ChannelId, InternalActor, Participant,
    };
    use caelumora_domain::component::{ComponentBundle, ComponentConfig, ComponentDescriptor};
    use caelumora_domain::confidentiality::Classification;
    use caelumora_domain::event::{EntityChange, GlobalEvent};
    use caelumora_domain::group::{Group, GroupId};
    use caelumora_domain::project::Project;
    use caelumora_domain::project::ProjectId;
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

    /// A shared project of alice's.
    async fn project_of_alice(world: &World) -> ProjectId {
        let spec = ProjectSpec::new("report", Scope::UserScope { user_id: world.alice });
        world.kernel.project_manager().create(&World::as_user(world.alice), spec).await.unwrap().id
    }

    /// Opening a session that runs holds it shared, beside others that read it; opening one that
    /// does not run holds it exclusively, as it starts it.
    #[tokio::test]
    async fn opening_holds_a_running_session_shared_and_one_it_starts_exclusively() {
        let world = World::new().await;
        let executor = Arc::clone(world.kernel.procedures());
        let alice = World::as_user(world.alice);
        let id = world.shared_session(&[], &[]).await.id();

        let held = Held::key(&executor, Key::Session(id), false).await;
        executor.run(Open::new(&alice, id)).await.expect("opened beside it");
        held.release().await;

        world.manager.stop(&alice, id).await.unwrap();
        let held = Held::key(&executor, Key::Session(id), false).await;
        let opening = Open::new(&alice, id);
        waits_for(held, &executor, opening).await.expect("started once it is let go");
    }

    /// Deleting, restoring and purging a session hold the project it lies in, whose deletion
    /// enumerates the sessions in it.
    #[tokio::test]
    async fn deleting_restoring_and_purging_a_session_hold_its_project() {
        let world = World::new().await;
        let executor = Arc::clone(world.kernel.procedures());
        let alice = World::as_user(world.alice);
        let id = world.shared_session(&[], &[]).await.id();
        let project = project_of_alice(&world).await;
        world.manager.update_project(&alice, id, Some(project)).await.unwrap();

        let held = Held::key(&executor, Key::Project(project), true).await;
        let deleting = Delete::new(&alice, id);
        assert!(waits_for(held, &executor, deleting).await.unwrap(), "deleted once let go");

        let held = Held::key(&executor, Key::Project(project), true).await;
        let restoring = Restore::new(&alice, id);
        waits_for(held, &executor, restoring).await.expect("restored once it is let go");

        world.manager.delete(&alice, id).await.unwrap();
        let held = Held::key(&executor, Key::Project(project), true).await;
        let purging = Purge::new(&World::system(), id);
        assert!(waits_for(held, &executor, purging).await.unwrap(), "purged once let go");
    }

    /// Moving a session holds the project it goes to, which is not deleted meanwhile.
    #[tokio::test]
    async fn moving_a_session_holds_the_project_it_goes_to() {
        let world = World::new().await;
        let executor = Arc::clone(world.kernel.procedures());
        let alice = World::as_user(world.alice);
        let id = world.shared_session(&[], &[]).await.id();
        let project = project_of_alice(&world).await;

        let held = Held::key(&executor, Key::Project(project), true).await;
        let moving = Move::new(&alice, id, Some(project));
        waits_for(held, &executor, moving).await.expect("moved once it is let go");
    }

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

        // The group of its participants before it, as whoever makes a change announces it, and
        // so that the group admits them as the bus delivers it (issue 593).
        let group_created = events.iter().position(|e| {
            e.about::<Group>() == Some((GroupId::of_session(session_id), &EntityChange::Created))
        });

        assert!(created.is_some(), "expected SessionCreated, got {:?}", events);
        assert!(ch_created.is_some(), "expected the channel created, got {:?}", events);
        assert!(started.is_some(), "expected SessionStarted, got {:?}", events);
        assert!(group_created.is_some(), "expected its group created, got {:?}", events);
        assert!(group_created < created, "its group first: {:?}", events);
        assert!(created < ch_created && ch_created < started, "wrong order: {:?}", events);

        // It is now persisted and running.
        assert!(kernel.entity_stores().sessions.get(&ctx, session_id).await.is_ok());
        assert!(manager.running(session_id).is_some());

        manager.stop(&SecurityContext::system(), session_id).await.unwrap();
    }

    /// Deleting a session announces it deleted as it is marked, then stopped as it stops - whoever
    /// makes a change announces it (issue 593); its channels, hidden with it, are not announced.
    #[tokio::test]
    async fn delete_session_emits_deleted_then_stopped() {
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
        let ch_deleted = events.iter().any(|e| {
            e.about::<Channel>() == Some(((session_id, channel_id), &EntityChange::Deleted))
        });
        let deleted = events
            .iter()
            .position(|e| e.about::<Session>() == Some((session_id, &EntityChange::Deleted)));

        assert!(stopped.is_some(), "expected SessionStopped, got {:?}", events);
        assert!(!ch_deleted, "its channel is hidden with it, not deleted: {:?}", events);
        assert!(deleted.is_some(), "expected SessionDeleted, got {:?}", events);
        assert!(deleted < stopped, "wrong order: {:?}", events);

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

            async fn enumerate(&self, _: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
                locks.exclusive(Key::Session(self.session.id))
            }

            async fn carry_out(self, (): (), _: &Kernel, _: &mut Compensation) -> Result<()> {
                self.go.acquire().await?.forget();
                self.manager.start(self.session, self.channels, None).await?;
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
        let following = Follow::session(session.id());
        kernel.procedures().run(following).await.unwrap();
        assert!(!talk.participant_ids().contains(&actor), "looking again, it stops the actor");
        let announced = rx.drain().await;
        assert!(announced.iter().any(|e| matches!(e.about::<Channel>(), Some((address, change)) if address == (session.id(), talk.id()) && change.data::<ActorId>(channel::LEFT) == Some(ActorId::Agent(helper.id)))), "as a leave: {announced:?}");

        kernel.entity_stores().agents.restore(&system, helper.id).await.unwrap();
        let following = Follow::session(session.id());
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
        let project = manager
            .kernel()
            .project_manager()
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
        let mut events = kernel.event_bus().subscribe_all();
        let manager = SessionManager::new(kernel);
        let owner = UserId::new();
        let ctx =
            SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let session = create(&manager, &ctx, make_session("moving", owner)).await.unwrap();
        let project = manager
            .kernel()
            .project_manager()
            .create(&ctx, ProjectSpec::new("report", Scope::UserScope { user_id: owner }))
            .await
            .unwrap();
        manager.update_project(&ctx, session.id(), Some(project.id)).await.unwrap();
        // Its own gone, as lifting it takes it out of the session's scope.
        let own = manager
            .kernel()
            .project_manager()
            .load_own(session.id())
            .await
            .unwrap()
            .expect("its own");
        manager.kernel().project_manager().purge_own(own.id).await.unwrap();

        events.drain().await;
        passing.store(0, Ordering::SeqCst);
        manager
            .update_project(&ctx, session.id(), None)
            .await
            .expect_err("the session cannot be stored");
        assert_eq!(session.session().project, project.id, "it lies where it lay");
        let made = manager.kernel().project_manager().load_own(session.id()).await.unwrap();
        assert!(made.is_none(), "the own made for it is purged: {made:?}");
        // Announced as made, and as taken back, as it happened (issue 593).
        let changes: Vec<_> = events
            .drain()
            .await
            .iter()
            .filter_map(|e| e.about::<Project>().map(|(_, change)| change.clone()))
            .collect();
        assert!(
            matches!(changes.as_slice(), [EntityChange::Created, EntityChange::Deleted]),
            "created, then deleted: {changes:?}"
        );
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
            state.begin_turn(
                ThreadId::default(),
                Work::new(Default::default(), None, Default::default(), Default::default()),
            );
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
            async move {
                let state = states.get_agent_channel_state(agent, elsewhere).await.unwrap();
                state.turn(ThreadId::default()).is_some()
            }
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
            .send_message(None, Some("hello".to_string()), None, Vec::new())
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
        let in_turn = |processing_state| {
            let mut state = AgentChannelState::default();
            let part = state.thread_mut(ThreadId::default());
            part.processing_state = processing_state;
            part.turn = Some(TurnState::default());
            state
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

    /// Issue 593: a session whose storing fails part of the way - its second channel refused by the
    /// store for a name the session has already, once the session, its own project and its first
    /// channel are stored - leaves nothing behind: what was stored is taken back, the last first,
    /// in the kernel's name, and announced deleted as it was announced made.
    #[tokio::test]
    async fn a_session_whose_storing_fails_part_of_the_way_leaves_nothing_behind() {
        let (kernel, manager) = owned_kernel(Default::default());
        let mut events = kernel.event_bus().subscribe_all();
        let alice = make_user("alice");
        let ctx =
            SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let session = make_session("twice", alice.id);
        let first = main_channel(&session);
        let second = Channel { id: ChannelId::new(), ..first.clone() };
        let channels = vec![first.clone(), second];

        let refused =
            manager.create(&ctx, session.clone(), channels).await.expect_err("named twice");
        assert!(
            matches!(refused.downcast_ref(), Some(StoreError::NameTaken { .. })),
            "refused by the store: {refused}"
        );
        let system = SecurityContext::system();
        let stores = kernel.entity_stores();
        assert!(stores.sessions.get_deleted(&system, session.id).await.is_err(), "no session");
        assert!(stores.channels.get_deleted(&system, first.id).await.is_err(), "no channel");
        assert!(
            manager.kernel().project_manager().load_own(session.id).await.unwrap().is_none(),
            "no project of its own"
        );
        for group in [GroupId::of_session(session.id), GroupId::of_channel(first.id)] {
            assert!(stores.groups.get_deleted(&system, group).await.is_err(), "no group");
        }
        let changes = |announced: &[GlobalEvent]| -> Vec<(&'static str, EntityChange)> {
            let of =
                |event: &GlobalEvent| match (event.about::<Session>(), event.about::<Channel>()) {
                    (Some((_, change)), _) => Some(("session", change.clone())),
                    (_, Some((_, change))) => Some(("channel", change.clone())),
                    _ => None,
                };
            announced.iter().filter_map(of).collect()
        };
        assert_eq!(
            changes(&events.drain().await),
            [
                ("session", EntityChange::Created),
                ("channel", EntityChange::Created),
                ("channel", EntityChange::Deleted),
                ("session", EntityChange::Deleted),
            ],
            "made, then taken back the last first"
        );
    }
}
