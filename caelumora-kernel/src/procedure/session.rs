//! The procedures of a session's lifecycle (issue 593): creating, opening, stopping, deleting,
//! restoring and purging it, moving it into another project, and stopping every session as the
//! process ends.
//!
//! Each locks its session exclusively - opening one that runs locks it shared -, and the project
//! where it changes whether the session is in it or deleted, and carries the change out by the
//! building blocks of the `SessionManager` and the `SessionLoop`, which ask for the proof that it
//! holds the session (`Admitted`).

use super::{Admitted, Compensation, Deferred, Key, Locks, Procedure, ProcedureExecutor};
use crate::session::{SessionError, SessionLoop, SessionManager, SessionSpec};
use async_trait::async_trait;
use caelumora_domain::project::ProjectId;
use caelumora_domain::removal::Removal;
use caelumora_domain::session::{Session, SessionId};
use caelumora_domain::template::SessionTemplateId;
use caelumora_security::SecurityContext;
use futures::future::join_all;
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
    sessions: Arc<SessionManager>,
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
        sessions: &Arc<SessionManager>,
        context: &SecurityContext,
        template_id: SessionTemplateId,
        spec: SessionSpec,
    ) -> Self {
        Self {
            sessions: Arc::clone(sessions),
            context: context.clone(),
            id: SessionId::new(),
            template_id,
            spec,
        }
    }
}

#[async_trait]
impl Procedure for Create {
    type Plan = ();
    type Output = Arc<SessionLoop>;
    type Error = SessionError;

    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
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
        admitted: &Admitted<'_>,
        log: &mut Compensation,
    ) -> Result<Arc<SessionLoop>, SessionError> {
        let Self { sessions, context, id, template_id, spec } = self;
        record_creation(&sessions, id, log);
        let made = sessions.make_from_template(admitted, &context, id, template_id, spec).await;
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
    log.record("creating the session", move |admitted| {
        Box::pin(async move { roll_back_creation(&rolled_back, admitted, id).await })
    });
}

/// Rolls back creating session `id`, not to be kept - not complete -, in the kernel's name -
/// making it was permitted: deleted and purged, with what was made in its scope.
///
/// # Errors
/// What deleting or purging it reports.
async fn roll_back_creation(
    sessions: &SessionManager,
    admitted: &Admitted<'_>,
    id: SessionId,
) -> anyhow::Result<()> {
    let system = SecurityContext::system();
    sessions.delete_held(admitted, &system, id, &Removal::now(system.caller_id)).await?;
    sessions.purge_held(admitted, &system, id).await?;
    Ok(())
}

/// Opens session `id` for `context`: the running one, or loaded from the store and started - which
/// announces it as `session::STARTED`, not as created, as the session exists already. Started
/// once, however many ask at the same time (issue 62): who comes second waits for the first and
/// gets the session it started - if they may read it. A channel whose components cannot be read is
/// left out as the session starts, and built again whenever the running session is opened
/// (`SessionLoop::retry_unavailable`, issue 463).
pub struct Open {
    sessions: Arc<SessionManager>,
    context: SecurityContext,
    id: SessionId,
}

impl Open {
    /// Opens session `id` for `context`.
    pub fn new(sessions: &Arc<SessionManager>, context: &SecurityContext, id: SessionId) -> Self {
        Self { sessions: Arc::clone(sessions), context: context.clone(), id }
    }
}

#[async_trait]
impl Procedure for Open {
    type Plan = ();
    type Output = Arc<SessionLoop>;
    type Error = SessionError;

    /// Shared for one that runs - which keeps it running -, exclusively for one it starts.
    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        let key = Key::Session(self.id);
        if self.sessions.running(self.id).is_some() {
            locks.shared(key)?;
            if self.sessions.running(self.id).is_some() {
                return Ok(());
            }
        }
        locks.exclusive(key)
    }

    /// # Errors
    /// `SessionError::NotFound` if there is no such session or `context` may not read it,
    /// `RunsElsewhere` if another process that shares the stores runs it (issue 554); whatever
    /// loading or starting it reports otherwise.
    async fn carry_out(
        self,
        (): (),
        admitted: &Admitted<'_>,
        _: &mut Compensation,
    ) -> Result<Arc<SessionLoop>, SessionError> {
        let Self { sessions, context, id } = self;
        Ok(sessions.open_held(admitted, &context, id).await?)
    }
}

/// Stops session `id` on behalf of `context` without deleting it: its channels and actors stop,
/// for everybody in it (`StopSession`) - each actor keeps what it is in the middle of, to be
/// resumed when the session runs again, and is given up to `STOP_GRACE` to store it (issue 377) -,
/// and the commands of its channels end (issue 254). Answers whether it ran.
pub struct Stop {
    sessions: Arc<SessionManager>,
    context: SecurityContext,
    id: SessionId,
}

impl Stop {
    /// Stops session `id` on behalf of `context`.
    pub fn new(sessions: &Arc<SessionManager>, context: &SecurityContext, id: SessionId) -> Self {
        Self { sessions: Arc::clone(sessions), context: context.clone(), id }
    }
}

#[async_trait]
impl Procedure for Stop {
    type Plan = ();
    type Output = bool;
    type Error = SessionError;

    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        locks.exclusive(Key::Session(self.id))
    }

    /// # Errors
    /// As `SessionManager::stop_held`.
    async fn carry_out(
        self,
        (): (),
        admitted: &Admitted<'_>,
        _: &mut Compensation,
    ) -> Result<bool, SessionError> {
        let Self { sessions, context, id } = self;
        Ok(sessions.stop_held(admitted, &context, id).await?)
    }
}

/// Stops and deletes session `id` on behalf of `context` (`SessionManager::delete_held`): marked,
/// with what goes with it, until it is restored or purged. Answers `false` if there is no such
/// session or `context` may not read it, as every deletion answers (issue 510).
pub struct Delete {
    sessions: Arc<SessionManager>,
    context: SecurityContext,
    id: SessionId,
}

impl Delete {
    /// Deletes session `id` on behalf of `context`.
    pub fn new(sessions: &Arc<SessionManager>, context: &SecurityContext, id: SessionId) -> Self {
        Self { sessions: Arc::clone(sessions), context: context.clone(), id }
    }
}

#[async_trait]
impl Procedure for Delete {
    type Plan = ();
    type Output = bool;
    type Error = SessionError;

    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_with_its_project(&self.sessions, locks, self.id).await
    }

    /// # Errors
    /// As `SessionManager::delete_held`.
    async fn carry_out(
        self,
        (): (),
        admitted: &Admitted<'_>,
        _: &mut Compensation,
    ) -> Result<bool, SessionError> {
        let Self { sessions, context, id } = self;
        let removal = Removal::now(context.caller_id);
        Ok(sessions.delete_held(admitted, &context, id, &removal).await?)
    }
}

/// Restores the deleted session `id` on behalf of `context` (`SessionManager::restore_held`, issue
/// 164); it is not started.
pub struct Restore {
    sessions: Arc<SessionManager>,
    context: SecurityContext,
    id: SessionId,
}

impl Restore {
    /// Restores session `id` on behalf of `context`.
    pub fn new(sessions: &Arc<SessionManager>, context: &SecurityContext, id: SessionId) -> Self {
        Self { sessions: Arc::clone(sessions), context: context.clone(), id }
    }
}

#[async_trait]
impl Procedure for Restore {
    type Plan = ();
    type Output = Session;
    type Error = SessionError;

    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_with_its_project(&self.sessions, locks, self.id).await
    }

    /// # Errors
    /// As `SessionManager::restore_held`.
    async fn carry_out(
        self,
        (): (),
        admitted: &Admitted<'_>,
        _: &mut Compensation,
    ) -> Result<Session, SessionError> {
        let Self { sessions, context, id } = self;
        Ok(sessions.restore_held(admitted, &context, id).await?)
    }
}

/// Removes the deleted session `id` for good on behalf of `context` (`SessionManager::purge_held`,
/// issue 164). Answers `false` if there was nothing deleted the caller could see, as every purge
/// answers (issue 510).
pub struct Purge {
    sessions: Arc<SessionManager>,
    context: SecurityContext,
    id: SessionId,
}

impl Purge {
    /// Purges session `id` on behalf of `context`.
    pub fn new(sessions: &Arc<SessionManager>, context: &SecurityContext, id: SessionId) -> Self {
        Self { sessions: Arc::clone(sessions), context: context.clone(), id }
    }
}

#[async_trait]
impl Procedure for Purge {
    type Plan = ();
    type Output = bool;
    type Error = SessionError;

    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_with_its_project(&self.sessions, locks, self.id).await
    }

    /// # Errors
    /// As `SessionManager::purge_held`.
    async fn carry_out(
        self,
        (): (),
        admitted: &Admitted<'_>,
        _: &mut Compensation,
    ) -> Result<bool, SessionError> {
        let Self { sessions, context, id } = self;
        Ok(sessions.purge_held(admitted, &context, id).await?)
    }
}

/// Locks session `id` exclusively, and the project it lies in - deleted or not - shared, as read
/// once the session is locked: deleting, restoring and purging a project enumerate the sessions in
/// it, so that every procedure that changes whether a session is in a project, or whether it is
/// deleted, locks that project (`procedure::project`). A project that cannot be read is not
/// locked: carrying the procedure out then fails at the session.
async fn lock_with_its_project(
    sessions: &SessionManager,
    locks: &mut Locks<'_>,
    id: SessionId,
) -> Result<(), Deferred> {
    locks.exclusive(Key::Session(id))?;
    if let Some(project) = sessions.project_of(id).await {
        locks.shared(Key::Project(project))?;
    }
    Ok(())
}

/// Puts session `id` into the shared project `project` on behalf of `context`, or with `None` into
/// a project of its own again (`SessionLoop::update_project`, issue 467) - starting it, if it does
/// not run. Locks the project it goes to shared, so that it is not deleted meanwhile: deleting a
/// project enumerates the sessions that lie in it, and one put into it meanwhile would be missed.
/// The project it leaves needs no lock: deleting that one locks the session, and enumerates anew
/// once it is moved.
pub struct Move {
    sessions: Arc<SessionManager>,
    context: SecurityContext,
    id: SessionId,
    project: Option<ProjectId>,
}

impl Move {
    /// Puts session `id` into `project` on behalf of `context`, or into its own with `None`.
    pub fn new(
        sessions: &Arc<SessionManager>,
        context: &SecurityContext,
        id: SessionId,
        project: Option<ProjectId>,
    ) -> Self {
        Self { sessions: Arc::clone(sessions), context: context.clone(), id, project }
    }
}

#[async_trait]
impl Procedure for Move {
    type Plan = ();
    type Output = ();
    type Error = SessionError;

    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        locks.exclusive(Key::Session(self.id))?;
        if let Some(project) = self.project {
            locks.shared(Key::Project(project))?;
        }
        Ok(())
    }

    /// # Errors
    /// As `Open`, and as `SessionLoop::update_project`.
    async fn carry_out(
        self,
        (): (),
        admitted: &Admitted<'_>,
        _: &mut Compensation,
    ) -> Result<(), SessionError> {
        let Self { sessions, context, id, project } = self;
        let session = sessions.open_held(admitted, &context, id).await?;
        Ok(session.update_project(admitted, &context, project).await?)
    }
}

/// Stops every session that runs as the process ends (issue 297), as `Stop` stops one, all at
/// once - each actor keeping what it is in the middle of, to be resumed when its session runs
/// again -, then ends, of the commands, only what does not outlive the process
/// (`CommandReaper::shutdown`): an Open Terminal instance keeps its commands, unlike at a stop.
/// The last procedure of the executor (`shut_down`).
struct ShutDown {
    sessions: Arc<SessionManager>,
}

impl ShutDown {
    /// Shuts down the sessions of `sessions`.
    fn new(sessions: &Arc<SessionManager>) -> Self {
        Self { sessions: Arc::clone(sessions) }
    }
}

#[async_trait]
impl Procedure for ShutDown {
    type Plan = Vec<SessionId>;
    type Output = ();
    type Error = SessionError;

    /// Every session that runs: none starts meanwhile, as the executor admits nothing any more and
    /// waited for what was under way.
    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<Vec<SessionId>, Deferred> {
        let ids = self.sessions.running_ids();
        for id in &ids {
            locks.exclusive(Key::Session(*id))?;
        }
        Ok(ids)
    }

    async fn carry_out(
        self,
        ids: Vec<SessionId>,
        admitted: &Admitted<'_>,
        _: &mut Compensation,
    ) -> Result<(), SessionError> {
        let sessions = &self.sessions;
        join_all(
            ids.into_iter().map(|id| sessions.stop_running(admitted, id, SessionLoop::stop_actors)),
        )
        .await;
        sessions.shut_down_commands(admitted).await;
        Ok(())
    }
}

/// Shuts the sessions down as the process ends (issue 297): `procedures` admits nothing any more,
/// and every session that runs is stopped (`ShutDown`). Returns within `SHUTDOWN_BOUND`, leaving
/// whatever is still under way to end with the process: a shutdown that hangs is worse than none.
pub async fn shut_down(procedures: &ProcedureExecutor, sessions: &Arc<SessionManager>) {
    match procedures.shut_down(ShutDown::new(sessions), SHUTDOWN_BOUND).await {
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
/// way lists them (`SessionManager::interrupted`): the turns stored mid-way go on from the step
/// they stood at, the remote tasks are followed again. Nobody asked for it, so the kernel opens
/// them in its own name (`Open`); what the agents then do is done in their own names, as at every
/// start. One after another, not bounded otherwise. A session that is gone or deleted is passed
/// over; the sessions with no work under way stay stopped until someone acts in them. Ends early
/// once `procedures` shuts down. Returns how many it started.
pub async fn resume_interrupted(
    procedures: &ProcedureExecutor,
    sessions: &Arc<SessionManager>,
) -> usize {
    let system = SecurityContext::system();
    let mut started = 0;
    for id in sessions.interrupted().await {
        match procedures.run(Open::new(sessions, &system, id)).await {
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
        executor.run(Open::new(&world.manager, &alice, id)).await.expect("opened beside it");
        held.release().await;

        world.manager.stop(&alice, id).await.unwrap();
        let held = Held::key(&executor, Key::Session(id), false).await;
        let opening = Open::new(&world.manager, &alice, id);
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
        let deleting = Delete::new(&world.manager, &alice, id);
        assert!(waits_for(held, &executor, deleting).await.unwrap(), "deleted once let go");

        let held = Held::key(&executor, Key::Project(project), true).await;
        let restoring = Restore::new(&world.manager, &alice, id);
        waits_for(held, &executor, restoring).await.expect("restored once it is let go");

        world.manager.delete(&alice, id).await.unwrap();
        let held = Held::key(&executor, Key::Project(project), true).await;
        let purging = Purge::new(&world.manager, &World::system(), id);
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
        let moving = Move::new(&world.manager, &alice, id, Some(project));
        waits_for(held, &executor, moving).await.expect("moved once it is let go");
    }
}
