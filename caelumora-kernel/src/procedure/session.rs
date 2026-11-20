//! The procedures of a session's lifecycle (issue 593): creating, opening, stopping, deleting,
//! restoring and purging it, moving it into another project, and stopping every session as the
//! process ends.
//!
//! Each locks its session exclusively - opening one that runs locks it shared -, and the project
//! where it changes whether the session is in it or deleted. Each asks what the change asks and
//! carries it out by the `SessionManager` - which starts and stops the sessions that run here and
//! holds the lifecycles of sessions and their own projects (`Lifecycle`), each change announced as
//! it is stored -, by the session's loop and controller, and by the services of the kernel: the
//! groups of the participants, what lies in a scope, the commands, the state of the channels.

use super::{
    all_or_nothing, sessions_of, Compensation, Deferred, Key, Locks, Procedure, ProcedureExecutor,
};
use crate::kernel::Kernel;
use crate::project;
use crate::scoped::ScopeNotEmpty;
use crate::session::{
    store_error, Ending, Found, SessionError, SessionLoop, SessionManager, SessionSpec,
};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_domain::channel::{ChannelField, ChannelId};
use caelumora_domain::group::GroupId;
use caelumora_domain::project::{Project, ProjectId};
use caelumora_domain::query::{Filter, Query};
use caelumora_domain::removal::Removal;
use caelumora_domain::scope::{Scope, ScopeField};
use caelumora_domain::security::{Grant, Subject};
use caelumora_domain::session::{Session, SessionId};
use caelumora_domain::store::{StoreError, StoreResultExt, Stored};
use caelumora_domain::template::SessionTemplateId;
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
    /// whose user owns it and is the template's "user" (`SessionManager::make_from_template`).
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

    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        log: &mut Compensation,
    ) -> Result<Arc<SessionLoop>, SessionError> {
        let Self { context, id, template_id, spec } = self;
        let sessions = sessions_of(kernel)?;
        record_creation(&sessions, id, log);
        let made = sessions.make_from_template(log, &context, id, template_id, spec).await;
        Ok(made?)
    }
}

/// Records in `log` how creating session `id` is rolled back (`roll_back_creation`) - before it is
/// made: a session not made yet is not there to delete or purge, and what storing it made of it is
/// purged by storing it already (`SessionManager::persist`). For every procedure that creates a
/// session.
pub(crate) fn record_creation(
    sessions: &Arc<SessionManager>,
    id: SessionId,
    log: &mut Compensation,
) {
    let rolled_back = Arc::clone(sessions);
    log.record("creating the session", async move { roll_back_creation(&rolled_back, id).await });
}

/// Rolls back creating session `id`, not to be kept - not complete -, in the kernel's name -
/// making it was permitted: deleted and purged, with what was made in its scope.
///
/// # Errors
/// What deleting or purging it reports.
async fn roll_back_creation(sessions: &SessionManager, id: SessionId) -> Result<()> {
    let kernel = sessions.kernel();
    let system = SecurityContext::system();
    let removal = Removal::now(system.caller_id);
    all_or_nothing("deleting a session not made", async |log: &mut Compensation| {
        delete(log, kernel, &system, id, &removal).await
    })
    .await?;
    purge(kernel, &system, id).await?;
    Ok(())
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
/// taken away (`SessionManager::session_lifecycle`, `own_lifecycle`). `false` if there is no such
/// session or `context` may not read it, as every deletion answers (issue 510).
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
        sessions.own_lifecycle().delete(&SecurityContext::system(), own, removal).await?;
        let (sessions, own) = (Arc::clone(&sessions), own.id);
        log.record("marking the session's own project deleted", async move {
            let system = SecurityContext::system();
            Ok(sessions.own_lifecycle().restore(&system, own).await.map(drop)?)
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
    let own = sessions.load_own(id).await?;
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
/// (`SessionManager::session_lifecycle`, `own_lifecycle`). It is not started; opening it starts it,
/// as any stored session.
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
        sessions.own_lifecycle().restore(&SecurityContext::system(), own.id).await?;
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
/// in it (`ProjectManager`). In the order of every purge of what has something behind it
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
        sessions.purge_own(own.id).await?;
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
    let sessions = sessions_of(kernel)?;
    let found = project_to_move_to(kernel, &sessions, session, context, project).await?;
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
            sessions.make_own(own).await?;
            let (sessions, own) = (Arc::clone(&sessions), own.clone());
            log.record("making a session's own project", async move {
                Ok(sessions.discard_own(&own).await?)
            });
        }
        MoveTarget::Shared(shared) => {
            let participants = Subject::Group(GroupId::of_session(session.id()));
            let controller = controllers.project_controller(shared.id).await?;
            let sharing = controller.share(&system, participants, Grant::Read);
            sharing.await.map_err(anyhow::Error::from)?;
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
    sessions: &SessionManager,
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
        None => match sessions.load_own(session.id()).await? {
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
    use super::{Delete, Move, Open, Purge, Restore};
    use crate::procedure::testing::{waits_for, Held};
    use crate::procedure::Key;
    use crate::project::ProjectSpec;
    use crate::testing::World;
    use caelumora_domain::project::ProjectId;
    use caelumora_domain::scope::Scope;
    use std::sync::Arc;

    /// A shared project of alice's.
    async fn project_of_alice(world: &World) -> ProjectId {
        let spec = ProjectSpec::new("report", Scope::UserScope { user_id: world.alice });
        world.projects.create(&World::as_user(world.alice), spec).await.unwrap().id
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
}
