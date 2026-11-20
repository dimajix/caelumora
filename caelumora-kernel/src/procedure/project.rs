//! The procedures of a shared project's lifecycle (issue 593): deleting, restoring and purging it,
//! with the sessions that lie in it.
//!
//! Each locks the project exclusively, and then each session that lies in it - those that run and
//! are stored for a deletion, those deleted for a restoring or a purge -, read once the project is
//! locked: putting a session into a project locks the project shared (`session::Move`), so that
//! none comes in meanwhile.

use super::{Compensation, Deferred, Key, Locks, Procedure};
use crate::kernel::Kernel;
use crate::project::{ProjectError, ProjectManager};
use async_trait::async_trait;
use caelumora_domain::project::{Project, ProjectId};
use caelumora_domain::session::SessionId;
use caelumora_security::SecurityContext;
use std::sync::Arc;

/// Deletes the shared project `id` on behalf of `context`, with the sessions that lie in it
/// (`ProjectManager::delete_held`). Answers `false` if there was none the caller could see.
pub struct Delete {
    projects: Arc<ProjectManager>,
    context: SecurityContext,
    id: ProjectId,
}

impl Delete {
    /// Deletes project `id` on behalf of `context`.
    pub fn new(projects: &Arc<ProjectManager>, context: &SecurityContext, id: ProjectId) -> Self {
        Self { projects: Arc::clone(projects), context: context.clone(), id }
    }
}

#[async_trait]
impl Procedure for Delete {
    type Plan = Result<(), ProjectError>;
    type Output = bool;
    type Error = ProjectError;

    async fn enumerate(&self, _: &Kernel, locks: &mut Locks<'_>) -> Result<Self::Plan, Deferred> {
        locks.exclusive(Key::Project(self.id))?;
        let sessions = self.projects.sessions_in(self.id).await;
        let ids = sessions.map(|sessions| sessions.iter().map(|session| session.id).collect());
        lock_sessions(locks, ids)
    }

    /// # Errors
    /// As `ProjectManager::delete_held`; what reading its sessions reported.
    async fn carry_out(
        self,
        read: Result<(), ProjectError>,
        _: &Kernel,
        _: &mut Compensation,
    ) -> Result<bool, ProjectError> {
        read?;
        self.projects.delete_held(&self.context, self.id).await
    }
}

/// Restores the deleted shared project `id` on behalf of `context`, with the sessions deleted with
/// it (`ProjectManager::restore_held`).
pub struct Restore {
    projects: Arc<ProjectManager>,
    context: SecurityContext,
    id: ProjectId,
}

impl Restore {
    /// Restores project `id` on behalf of `context`.
    pub fn new(projects: &Arc<ProjectManager>, context: &SecurityContext, id: ProjectId) -> Self {
        Self { projects: Arc::clone(projects), context: context.clone(), id }
    }
}

#[async_trait]
impl Procedure for Restore {
    type Plan = Result<(), ProjectError>;
    type Output = Project;
    type Error = ProjectError;

    async fn enumerate(&self, _: &Kernel, locks: &mut Locks<'_>) -> Result<Self::Plan, Deferred> {
        locks.exclusive(Key::Project(self.id))?;
        lock_deleted_sessions(&self.projects, locks, self.id).await
    }

    /// # Errors
    /// As `ProjectManager::restore_held`; what reading its sessions reported.
    async fn carry_out(
        self,
        read: Result<(), ProjectError>,
        _: &Kernel,
        _: &mut Compensation,
    ) -> Result<Project, ProjectError> {
        read?;
        self.projects.restore_held(&self.context, self.id).await
    }
}

/// Removes the deleted shared project `id` for good on behalf of `context`, with the sessions
/// deleted that lie in it (`ProjectManager::purge_held`). Answers `false` if there was nothing
/// deleted the caller could see.
pub struct Purge {
    projects: Arc<ProjectManager>,
    context: SecurityContext,
    id: ProjectId,
}

impl Purge {
    /// Purges project `id` on behalf of `context`.
    pub fn new(projects: &Arc<ProjectManager>, context: &SecurityContext, id: ProjectId) -> Self {
        Self { projects: Arc::clone(projects), context: context.clone(), id }
    }
}

#[async_trait]
impl Procedure for Purge {
    type Plan = Result<(), ProjectError>;
    type Output = bool;
    type Error = ProjectError;

    async fn enumerate(&self, _: &Kernel, locks: &mut Locks<'_>) -> Result<Self::Plan, Deferred> {
        locks.exclusive(Key::Project(self.id))?;
        lock_deleted_sessions(&self.projects, locks, self.id).await
    }

    /// # Errors
    /// As `ProjectManager::purge_held`; what reading its sessions reported.
    async fn carry_out(
        self,
        read: Result<(), ProjectError>,
        _: &Kernel,
        _: &mut Compensation,
    ) -> Result<bool, ProjectError> {
        read?;
        self.projects.purge_held(&self.context, self.id).await
    }
}

/// Locks the deleted sessions that lie in project `id` exclusively; what reading them failed at is
/// the plan.
async fn lock_deleted_sessions(
    projects: &ProjectManager,
    locks: &mut Locks<'_>,
    id: ProjectId,
) -> Result<Result<(), ProjectError>, Deferred> {
    let deleted = projects.deleted_sessions_in(id).await;
    let ids = deleted.map(|deleted| deleted.iter().map(|session| session.entity.id).collect());
    lock_sessions(locks, ids)
}

/// Locks the sessions `ids` exclusively, if they could be read; what reading them failed at is the
/// plan.
fn lock_sessions(
    locks: &mut Locks<'_>,
    ids: Result<Vec<SessionId>, ProjectError>,
) -> Result<Result<(), ProjectError>, Deferred> {
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
    use super::{Delete, Purge, Restore};
    use crate::procedure::testing::{waits_for, Held};
    use crate::procedure::Key;
    use crate::project::ProjectSpec;
    use crate::testing::World;
    use caelumora_domain::scope::Scope;
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
        let project = world.projects.create(&alice, spec).await.unwrap().id;
        world.manager.update_project(&alice, id, Some(project)).await.unwrap();

        let held = Held::key(&executor, Key::Session(id), false).await;
        let deleting = Delete::new(&world.projects, &alice, project);
        assert!(waits_for(held, &executor, deleting).await.unwrap(), "deleted once let go");

        let held = Held::key(&executor, Key::Session(id), true).await;
        let restoring = Restore::new(&world.projects, &alice, project);
        waits_for(held, &executor, restoring).await.expect("restored once it is let go");

        world.projects.delete(&alice, project).await.unwrap();
        let held = Held::key(&executor, Key::Session(id), true).await;
        let purging = Purge::new(&world.projects, &World::system(), project);
        assert!(waits_for(held, &executor, purging).await.unwrap(), "purged once let go");
    }
}
