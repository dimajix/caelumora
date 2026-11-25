//! The procedures of a project's lifecycle (issue 593): deleting, restoring and purging a shared
//! one, with the sessions that lie in it and what lies in its scope, by the lifecycle the
//! `ProjectManager` holds, and lifting a session's own into a shared one.
//!
//! Each locks the project exclusively, and then each session that lies in it - those that run and
//! are stored for a deletion, those deleted for a restoring or a purge -, read once the project is
//! locked: putting a session into a project locks the project shared (`session::Move`), so that
//! none comes in meanwhile. Lifting a session's own locks the session whose own it is instead,
//! wherever that session lies, since moving it locks no project it leaves.

use super::session::{
    delete as delete_session, purge as purge_session, restore as restore_session,
};
use super::{all_or_nothing, Compensation, Deferred, Key, Locks, Procedure};
use crate::error::is_forbidden;
use crate::kernel::Kernel;
use crate::project::{store_error, ProjectError, ProjectResult};
use crate::scoped::ScopeNotEmpty;
use crate::session::SessionError;
use async_trait::async_trait;
use caelumora_domain::group::GroupId;
use caelumora_domain::project::{Project, ProjectId};
use caelumora_domain::query::{Filter, Query};
use caelumora_domain::removal::Removal;
use caelumora_domain::scope::Scope;
use caelumora_domain::session::{Session, SessionField, SessionId};
use caelumora_domain::store::{StoreError, StoreResultExt, Stored};
use caelumora_security::{ActionType, Enforcement, SecurityContext};
use caelumora_store::Deleted;
use std::slice;
use std::sync::Arc;
use tracing::{info, warn};

/// Deletes the shared project `id` on behalf of `context` (`DeleteProject`): marked first, then
/// the sessions that lie in it - each deleted whole or not at all, in `context`'s name, with what
/// lies in its scope -, then what lies in its scope (`ScopedEntities`), as far as `context` may
/// delete it, then the group of its members in the kernel's name - all of it with the project's
/// mark, by `context`'s caller (issue 469). If a session or something of its scope is left that
/// `context` may not delete, the project is kept, its mark taken away again, and what they could
/// delete stays deleted. Answers `false` if there was none the caller could see.
pub struct Delete {
    context: SecurityContext,
    id: ProjectId,
}

impl Delete {
    /// Deletes project `id` on behalf of `context`.
    pub fn new(context: &SecurityContext, id: ProjectId) -> Self {
        Self { context: context.clone(), id }
    }
}

#[async_trait]
impl Procedure for Delete {
    type Plan = ProjectResult<()>;
    type Output = bool;
    type Error = ProjectError;

    async fn enumerate(
        &self,
        kernel: &Kernel,
        locks: &mut Locks<'_>,
    ) -> Result<Self::Plan, Deferred> {
        locks.exclusive(Key::Project(self.id))?;
        let sessions = sessions_in(kernel, self.id).await;
        let ids = sessions.map(|sessions| sessions.iter().map(|session| session.id).collect());
        lock_sessions(locks, ids)
    }

    /// # Errors
    /// `InvalidScope` for a session's own, which goes with the session; `ScopeNotEmpty` with how
    /// many sessions and what else are left; `Session` if deleting a session failed otherwise -
    /// `SessionError::RunsElsewhere` for one another process runs (issue 554), which keeps the
    /// project as it is; what reading its sessions reported; or whatever the store reports,
    /// `PermissionDenied` included.
    async fn carry_out(
        self,
        read: ProjectResult<()>,
        kernel: &Kernel,
        log: &mut Compensation,
    ) -> ProjectResult<bool> {
        read?;
        let Self { context, id } = self;
        let projects = kernel.project_manager();
        let Some(project) = projects.lifecycle().store().get(&context, id).await.optional()? else {
            return Ok(false);
        };
        if project.is_own() {
            return Err(ProjectError::InvalidScope);
        }
        // One mark for all that goes with it (issue 469); marked before its scope is gone through,
        // so that nothing is put into it afterwards (`ScopeHolders`, issue 458).
        let removal = Removal::now(context.caller_id);
        if !projects.lifecycle().delete(&context, &project, &removal).await? {
            return Ok(false);
        }
        {
            let projects = Arc::clone(projects);
            log.record("marking the project deleted", async move {
                let system = SecurityContext::system();
                Ok(projects.lifecycle().restore(&system, id).await.map(drop)?)
            });
        }
        let holder = project.holds();
        let mut left = Vec::new();
        let sessions = sessions_in(kernel, id).await?;
        let kept = delete_sessions(kernel, &context, &sessions, &removal).await?;
        if kept > 0 {
            left.push((Session::KIND, kept));
        }
        let scoped = kernel.scoped_entities();
        left.extend(scoped.delete_lying_in(&context, slice::from_ref(&holder), &removal).await?);
        ScopeNotEmpty::require_empty(&holder, left)?;
        kernel.group_manager().delete_participants(slice::from_ref(&holder), &removal).await?;
        info!(project = %id, by = %context.caller_id, "Project deleted");
        Ok(true)
    }
}

/// Restores the deleted shared project `id` on behalf of `context` (`Restore`, asked ahead and by
/// the store again): the group of its members restored in the kernel's name - first, so that they
/// hear it restored -, then its mark taken away, announced - both, or neither: the group goes again
/// and the project keeps its mark, so that it can be tried again (issue 469) -, then the sessions
/// deleted with it and what lay in its scope - what carries its mark -, as far as `context` may
/// restore them; what they may not stays deleted, and the project comes back all the same.
pub struct Restore {
    context: SecurityContext,
    id: ProjectId,
}

impl Restore {
    /// Restores project `id` on behalf of `context`.
    pub fn new(context: &SecurityContext, id: ProjectId) -> Self {
        Self { context: context.clone(), id }
    }
}

#[async_trait]
impl Procedure for Restore {
    type Plan = ProjectResult<()>;
    type Output = Project;
    type Error = ProjectError;

    async fn enumerate(
        &self,
        kernel: &Kernel,
        locks: &mut Locks<'_>,
    ) -> Result<Self::Plan, Deferred> {
        locks.exclusive(Key::Project(self.id))?;
        lock_deleted_sessions(kernel, locks, self.id).await
    }

    /// # Errors
    /// `NotFound` if it is not deleted or not the caller's to see, or its purge is begun (issue
    /// 496), `InvalidScope` for a session's own, `PermissionDenied` if they may not restore it;
    /// what reading its sessions reported; or whatever else the store reports - `NameTaken` among
    /// it.
    async fn carry_out(
        self,
        read: ProjectResult<()>,
        kernel: &Kernel,
        log: &mut Compensation,
    ) -> ProjectResult<Project> {
        read?;
        let Self { context, id } = self;
        let projects = kernel.project_manager().lifecycle();
        // What went with it carries its mark (issue 469).
        let Deleted { entity: deleted, removal } =
            projects.store().get_deleted(&context, id).await.map_err(store_error(id))?;
        // One whose purge is begun is the store's to refuse, as it would: refused before anything
        // else is asked or restored.
        if deleted.purging() {
            return Err(ProjectError::NotFound(id));
        }
        if deleted.is_own() {
            return Err(ProjectError::InvalidScope);
        }
        // Asked ahead, as the store asks again as it restores it: the group of its members comes
        // back first, so that they hear it restored, and not for whoever may not restore it.
        let restoring = ActionType::restore(Project::KIND, &deleted.security_attributes);
        kernel.access_policy().require(&context, restoring, StoreError::PermissionDenied).await?;
        // Read before anything is restored, so that restoring the project is the last step that
        // can fail: what comes after it stays deleted if it cannot come back, and is logged.
        let system = SecurityContext::system();
        let sessions = &kernel.entity_stores().sessions;
        let with_it = sessions.list_deleted_with(&system, removal.id, &in_project(id)).await?;
        let holder = deleted.holds();
        let groups = kernel.group_manager();
        groups.restore_participants(slice::from_ref(&holder), &removal).await?;
        // If it cannot come back, its members' group goes again, so that a restoring tried again
        // finds what went with it.
        {
            let (groups, holder, removal) = (Arc::clone(groups), holder.clone(), removal.clone());
            log.record("restoring the group of the project's members", async move {
                Ok(groups.delete_participants(slice::from_ref(&holder), &removal).await?)
            });
        }
        let project = projects.restore(&context, id).await.map_err(store_error(id))?;
        for session in with_it.items {
            let restored = all_or_nothing("restoring a session", async |log| {
                restore_session(log, kernel, &context, session.id).await
            });
            if let Err(e) = restored.await {
                info!(project = %id, session = %session.id, "A session of a restored project stays deleted: {e}");
            }
        }
        let scoped = kernel.scoped_entities();
        if let Err(e) = scoped.restore_lying_in(&context, slice::from_ref(&holder), &removal).await
        {
            warn!(project = %id, "The project was restored, but not what lies in its scope: {e}");
        }
        info!(project = %id, by = %context.caller_id, "Project restored");
        Ok(project)
    }
}

/// Removes the deleted shared project `id` for good on behalf of `context`, with the sessions
/// deleted that lie in it, in the order of every purge of what has something behind it
/// (`purge_behind`): its purge begun (`Purge`, asked before anything is removed), then - in the
/// kernel's name - what lies behind it (`remove_behind`), then the project. A step that fails
/// leaves it deleted, its purge begun, so that the next purge removes what is left (issue 496).
/// Announces nothing, as no purge does. Answers `false` if there was nothing deleted the caller
/// could see.
pub struct Purge {
    context: SecurityContext,
    id: ProjectId,
}

impl Purge {
    /// Purges project `id` on behalf of `context`.
    pub fn new(context: &SecurityContext, id: ProjectId) -> Self {
        Self { context: context.clone(), id }
    }
}

#[async_trait]
impl Procedure for Purge {
    type Plan = ProjectResult<()>;
    type Output = bool;
    type Error = ProjectError;

    async fn enumerate(
        &self,
        kernel: &Kernel,
        locks: &mut Locks<'_>,
    ) -> Result<Self::Plan, Deferred> {
        locks.exclusive(Key::Project(self.id))?;
        lock_deleted_sessions(kernel, locks, self.id).await
    }

    /// # Errors
    /// `InvalidScope` for a session's own, `PermissionDenied`, `Session` if purging a session
    /// failed; what reading its sessions reported; or whatever the store reports.
    async fn carry_out(
        self,
        read: ProjectResult<()>,
        kernel: &Kernel,
        _: &mut Compensation,
    ) -> ProjectResult<bool> {
        read?;
        let Self { context, id } = self;
        let projects = kernel.project_manager().lifecycle();
        let Some(Deleted { entity: deleted, .. }) =
            projects.store().get_deleted(&context, id).await.optional()?
        else {
            return Ok(false);
        };
        if deleted.is_own() {
            return Err(ProjectError::InvalidScope);
        }
        let removing = async |project: &Project| remove_behind(kernel, project).await;
        let purged = projects.purge(&context, id, removing).await?;
        if purged {
            info!(project = %id, by = %context.caller_id, "Project purged");
        }
        Ok(purged)
    }
}

/// Lifts the own project `id` of a session into `scope` - its owner's or the global one - under
/// `name`, on behalf of `context` (issue 467): a shared project then, with all that lies in it, the
/// session included, which no longer takes it along (`ProjectController::lift`). Locks the project
/// exclusively, against the procedures of its session, which lock the project the session lies in
/// shared, and the session whose own it is exclusively, as moving it does, which locks only the
/// project it goes to - and holds that session against the other processes that share the stores,
/// as deleting it does (issue 554): neither meets it half lifted, nor takes it along once it is
/// shared.
pub struct Lift {
    context: SecurityContext,
    id: ProjectId,
    name: String,
    scope: Scope,
}

impl Lift {
    /// Lifts project `id` into `scope` under `name` on behalf of `context`.
    pub fn new(context: &SecurityContext, id: ProjectId, name: &str, scope: Scope) -> Self {
        Self { context: context.clone(), id, name: name.to_string(), scope }
    }
}

#[async_trait]
impl Procedure for Lift {
    /// The session whose own it is, if it is one; what reading it failed at.
    type Plan = ProjectResult<Option<SessionId>>;
    type Output = ();
    type Error = ProjectError;

    async fn enumerate(
        &self,
        kernel: &Kernel,
        locks: &mut Locks<'_>,
    ) -> Result<Self::Plan, Deferred> {
        locks.exclusive(Key::Project(self.id))?;
        // Read in the kernel's name, once it is locked: whether the caller reaches it is the
        // controller's to ask, as it lifts it.
        let store = kernel.project_manager().lifecycle().store();
        let project = match store.get(&SecurityContext::system(), self.id).await {
            Ok(project) => project,
            Err(error) => return Ok(Err(store_error(self.id)(error))),
        };
        let Scope::SessionScope { session_id } = project.name.scope else {
            return Ok(Ok(None));
        };
        locks.exclusive(Key::Session(session_id))?;
        Ok(Ok(Some(session_id)))
    }

    /// # Errors
    /// `NotFound` if there is no such project or the caller does not reach it; `Session` -
    /// `SessionError::RunsElsewhere` - if another process holds its session; otherwise as
    /// `ProjectController::lift` - `InvalidScope`, `PermissionDenied`, `Validation`, `NameTaken`.
    async fn carry_out(
        self,
        read: ProjectResult<Option<SessionId>>,
        kernel: &Kernel,
        log: &mut Compensation,
    ) -> ProjectResult<()> {
        let Self { context, id, name, scope } = self;
        if let Some(session_id) = read? {
            let sessions = kernel.session_manager();
            log.keep(sessions.hold(session_id).map_err(ProjectError::Session)?);
        }
        let found = kernel.controller_manager().project_controller(id).await;
        found.map_err(store_error(id))?.lift(&context, &name, scope).await
    }
}

/// Deletes `sessions` on behalf of `context`, each whole or not at all
/// (`procedure::session::delete` as one part, `all_or_nothing`), as a project they lie in is
/// deleted, marked with `removal`, its mark; answers how many are left - not the caller's to read
/// or delete, or with something in their scope that is not.
///
/// # Errors
/// `Session` if deleting one failed otherwise; what was deleted before stays deleted.
async fn delete_sessions(
    kernel: &Kernel,
    context: &SecurityContext,
    sessions: &[Session],
    removal: &Removal,
) -> ProjectResult<usize> {
    let mut kept = 0;
    for session in sessions {
        let deleted = all_or_nothing("deleting a session", async |log| {
            delete_session(log, kernel, context, session.id, removal).await
        });
        match deleted.await {
            Ok(true) => {}
            Ok(false) | Err(SessionError::ScopeNotEmpty(_)) => kept += 1,
            Err(e) => {
                let e = e.untyped();
                if !is_forbidden(&e) {
                    return Err(ProjectError::Session(e));
                }
                kept += 1;
            }
        }
    }
    Ok(kept)
}

/// Removes what lies behind `project` once its purge is begun, in the kernel's name: the sessions
/// deleted that lie in it, what lies in its scope and the group of its members. What a purge that
/// failed before removed already, or another purged meanwhile, is passed over.
///
/// # Errors
/// `Session` if purging a session failed, or whatever the store reports.
async fn remove_behind(kernel: &Kernel, project: &Project) -> ProjectResult<()> {
    let system = SecurityContext::system();
    for Deleted { entity: session, .. } in deleted_sessions_in(kernel, project.id).await? {
        let purged = purge_session(kernel, &system, session.id).await;
        purged.map_err(|e| ProjectError::Session(e.untyped()))?;
    }
    kernel.scoped_entities().purge_lying_in(slice::from_ref(&project.holds())).await?;
    kernel.group_manager().purge_participants(GroupId::of_project(project.id)).await?;
    Ok(())
}

/// The sessions that lie in project `id`, read in the kernel's name: all of them are found,
/// whatever the caller may read.
async fn sessions_in(kernel: &Kernel, id: ProjectId) -> ProjectResult<Vec<Session>> {
    let system = SecurityContext::system();
    Ok(kernel.entity_stores().sessions.list(&system, &in_project(id)).await?.items)
}

/// The deleted sessions that lie in project `id`, read in the kernel's name.
async fn deleted_sessions_in(
    kernel: &Kernel,
    id: ProjectId,
) -> ProjectResult<Vec<Deleted<Session>>> {
    let system = SecurityContext::system();
    Ok(kernel.entity_stores().sessions.list_deleted(&system, &in_project(id)).await?.items)
}

/// The sessions that lie in project `id`.
fn in_project(id: ProjectId) -> Query<SessionField> {
    Query::filtered(Filter::in_project(id))
}

/// Locks the deleted sessions that lie in project `id` exclusively; what reading them failed at is
/// the plan.
async fn lock_deleted_sessions(
    kernel: &Kernel,
    locks: &mut Locks<'_>,
    id: ProjectId,
) -> Result<ProjectResult<()>, Deferred> {
    let deleted = deleted_sessions_in(kernel, id).await;
    let ids = deleted.map(|deleted| deleted.iter().map(|session| session.entity.id).collect());
    lock_sessions(locks, ids)
}

/// Locks the sessions `ids` exclusively, if they could be read; what reading them failed at is the
/// plan.
fn lock_sessions(
    locks: &mut Locks<'_>,
    ids: ProjectResult<Vec<SessionId>>,
) -> Result<ProjectResult<()>, Deferred> {
    let ids = match ids {
        Ok(ids) => ids,
        Err(error) => return Ok(Err(error)),
    };
    for id in ids {
        locks.exclusive(Key::Session(id))?;
    }
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::{Delete, Lift, Purge, Restore};
    use crate::procedure::testing::{waits_for, Held};
    use crate::procedure::Key;
    use crate::project::ProjectError;
    use crate::project::ProjectSpec;
    use crate::session::testing::{create, failing_kernel, shared_kernel};
    use crate::session::SessionError;
    use crate::testing::World;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::channel::Channel;
    use caelumora_domain::component::ComponentBundle;
    use caelumora_domain::group::{Group, GroupId};
    use caelumora_domain::project::Project;
    use caelumora_domain::scope::Scope;
    use caelumora_domain::security::SecurityAttributes;
    use caelumora_domain::session::testing::make_session;
    use caelumora_domain::store::Stored;
    use caelumora_domain::user::UserId;
    use caelumora_domain::value::name::ScopedName;
    use caelumora_security::SecurityContext;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    /// Deleting a project holds the sessions in it, restoring and purging it those deleted with
    /// it: none of them is changed by another meanwhile.
    #[tokio::test]
    async fn a_project_holds_the_sessions_in_it() {
        let world = World::new().await;
        let executor = Arc::clone(world.kernel.procedures());
        let alice = World::as_user(world.alice);
        let id = world.shared_session(&[], &[]).await.id();
        let spec = ProjectSpec::new("report", Scope::UserScope { user_id: world.alice });
        let project = world.kernel.project_manager().create(&alice, spec).await.unwrap().id;
        world.manager.update_project(&alice, id, Some(project)).await.unwrap();

        let held = Held::key(&executor, Key::Session(id), false).await;
        let deleting = Delete::new(&alice, project);
        assert!(waits_for(held, &executor, deleting).await.unwrap(), "deleted once let go");

        let held = Held::key(&executor, Key::Session(id), true).await;
        let restoring = Restore::new(&alice, project);
        waits_for(held, &executor, restoring).await.expect("restored once it is let go");

        executor.run(Delete::new(&alice, project)).await.unwrap();
        let held = Held::key(&executor, Key::Session(id), true).await;
        let purging = Purge::new(&World::system(), project);
        assert!(waits_for(held, &executor, purging).await.unwrap(), "purged once let go");
    }

    /// Lifting a session's own project holds the project and the session: neither the procedures
    /// of the session, which hold the project it lies in, nor moving it meet it half lifted.
    #[tokio::test]
    async fn lifting_a_project_holds_it_and_its_session() {
        let world = World::new().await;
        let executor = Arc::clone(world.kernel.procedures());
        let alice = World::as_user(world.alice);
        let session = world.shared_session(&[], &[]).await;
        let own = session.session().project;
        let mine = Scope::UserScope { user_id: world.alice };

        let held = Held::key(&executor, Key::Project(own), false).await;
        let lifting = Lift::new(&alice, own, "results", mine.clone());
        waits_for(held, &executor, lifting).await.expect("lifted once the project is let go");

        world.manager.update_project(&alice, session.id(), None).await.unwrap();
        let anew = session.session().project;
        let held = Held::key(&executor, Key::Session(session.id()), false).await;
        let lifting = Lift::new(&alice, anew, "more results", mine);
        waits_for(held, &executor, lifting).await.expect("lifted once the session is let go");
    }

    /// Issue 554: a session's own project whose session another process that shares the stores
    /// holds is not lifted - that process may be deleting the session with it -, and is once the
    /// session is let go.
    #[tokio::test]
    async fn a_project_whose_session_another_process_holds_is_not_lifted() {
        let (kernel, locks) = shared_kernel();
        let executor = Arc::clone(kernel.procedures());
        let manager = kernel.session_manager();
        let owner = UserId::new();
        let ctx =
            SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let session = create(manager, &ctx, make_session("held", owner)).await.unwrap();
        let (id, own) = (session.id(), session.session().project);
        assert!(manager.stop(&ctx, id).await.unwrap());
        let mine = Scope::UserScope { user_id: owner };

        locks.elsewhere.lock().unwrap().insert(id);
        let refused = executor.run(Lift::new(&ctx, own, "results", mine.clone())).await;
        assert!(
            matches!(&refused, Err(ProjectError::Session(e)) if matches!(e.downcast_ref(), Some(SessionError::RunsElsewhere(it)) if *it == id)),
            "left to the other: {refused:?}"
        );
        locks.elsewhere.lock().unwrap().remove(&id);
        executor.run(Lift::new(&ctx, own, "results", mine)).await.expect("lifted once let go");
    }

    /// Issue 469: a project whose members' group cannot come back stays deleted, marked as it was,
    /// and tried again, comes back with its sessions and what lay in its scope.
    #[tokio::test]
    async fn a_project_whose_restoring_failed_comes_back_whole_when_tried_again() {
        let (kernel, passing) = failing_kernel(Group::KIND);
        let stores = kernel.entity_stores().clone();
        let projects = Arc::clone(kernel.project_manager());
        let executor = Arc::clone(kernel.procedures());
        let manager = kernel.session_manager();
        let owner = UserId::new();
        let ctx =
            SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let system = SecurityContext::system();
        let session = create(manager, &ctx, make_session("in it", owner)).await.unwrap();
        let project = projects
            .create(&ctx, ProjectSpec::new("report", Scope::UserScope { user_id: owner }))
            .await
            .unwrap();
        manager.update_project(&ctx, session.id(), Some(project.id)).await.unwrap();
        let mut bundle = ComponentBundle::new(
            ScopedName::new("tools", project.holds()).unwrap(),
            Default::default(),
            SecurityAttributes::systems(),
        );
        stores.component_bundles.create(&system, &mut bundle).await.unwrap();
        executor.run(Delete::new(&ctx, project.id)).await.unwrap();
        let mark = stores.projects.get_deleted(&system, project.id).await.unwrap().removal;

        passing.store(0, Ordering::SeqCst);
        executor
            .run(Restore::new(&ctx, project.id))
            .await
            .expect_err("its members cannot come back");
        assert_eq!(
            stores.projects.get_deleted(&system, project.id).await.unwrap().removal,
            mark,
            "it stays deleted, marked as it was"
        );

        passing.store(usize::MAX, Ordering::SeqCst);
        executor.run(Restore::new(&ctx, project.id)).await.expect("restored when tried again");
        assert!(
            stores.groups.get(&system, GroupId::of_project(project.id)).await.is_ok(),
            "with its members"
        );
        assert!(stores.sessions.get(&system, session.id()).await.is_ok(), "its session");
        assert!(
            stores.component_bundles.get(&system, bundle.id).await.is_ok(),
            "and what lay in its scope"
        );
    }

    /// A project that cannot come back once the group of its members did takes the group away
    /// again, marked as the project is, so that a restoring tried again finds it (issues 469, 593).
    #[tokio::test]
    async fn a_project_that_cannot_come_back_takes_its_members_group_away_again() {
        let (kernel, passing) = failing_kernel(Project::KIND);
        let stores = kernel.entity_stores();
        let projects = kernel.project_manager();
        let executor = kernel.procedures();
        let owner = UserId::new();
        let ctx =
            SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let system = SecurityContext::system();
        let project = projects
            .create(&ctx, ProjectSpec::new("report", Scope::UserScope { user_id: owner }))
            .await
            .unwrap();
        executor.run(Delete::new(&ctx, project.id)).await.unwrap();
        let mark = stores.projects.get_deleted(&system, project.id).await.unwrap().removal;

        passing.store(0, Ordering::SeqCst);
        executor.run(Restore::new(&ctx, project.id)).await.expect_err("it cannot come back");
        let group = stores.groups.get_deleted(&system, GroupId::of_project(project.id)).await;
        assert_eq!(group.expect("its group deleted again").removal, mark, "with its mark");

        passing.store(usize::MAX, Ordering::SeqCst);
        executor.run(Restore::new(&ctx, project.id)).await.expect("restored when tried again");
        assert!(stores.groups.get(&system, GroupId::of_project(project.id)).await.is_ok());
    }

    /// Issue 496: a purge that fails part of the way - here as the channel of a session in the
    /// project is marked - leaves the project deleted, its purge begun, so that it is not restored
    /// without what went already, and the next purge removes what is left.
    #[tokio::test]
    async fn a_project_whose_purge_failed_is_finished_by_the_next() {
        let (kernel, passing) = failing_kernel(Channel::KIND);
        let stores = kernel.entity_stores().clone();
        let projects = Arc::clone(kernel.project_manager());
        let executor = Arc::clone(kernel.procedures());
        let manager = kernel.session_manager();
        let owner = UserId::new();
        let ctx =
            SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let system = SecurityContext::system();
        let session = create(manager, &ctx, make_session("in it", owner)).await.unwrap();
        let project = projects
            .create(&ctx, ProjectSpec::new("report", Scope::UserScope { user_id: owner }))
            .await
            .unwrap();
        manager.update_project(&ctx, session.id(), Some(project.id)).await.unwrap();
        executor.run(Delete::new(&ctx, project.id)).await.unwrap();

        passing.store(0, Ordering::SeqCst);
        executor
            .run(Purge::new(&system, project.id))
            .await
            .expect_err("its session's channel cannot go");
        let left = stores.projects.get_deleted(&system, project.id).await.expect("it is left");
        assert!(left.entity.purging(), "its purge begun");
        assert!(executor.run(Restore::new(&ctx, project.id)).await.is_err(), "not to be restored");

        passing.store(usize::MAX, Ordering::SeqCst);
        assert!(executor
            .run(Purge::new(&system, project.id))
            .await
            .expect("purged when tried again"));
        assert!(stores.projects.get_deleted(&system, project.id).await.is_err(), "it is gone");
        assert!(
            stores.sessions.get_deleted(&system, session.id()).await.is_err(),
            "with its session"
        );
    }
}
