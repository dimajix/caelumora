//! `ProjectManager`: making projects (issue 467) - shared ones, and the own projects of sessions -
//! and the lifecycle by which they are made, deleted, restored and purged, the latter three by the
//! procedures of a project and of a session (`procedure::project`, `procedure::session`, issue
//! 593).

use super::{ProjectError, ProjectResult, ProjectSpec};
use crate::event::EventBus;
use crate::group::GroupManager;
use crate::lifecycle::Lifecycle;
use crate::scoped::found_or_made;
use caelumora_domain::actor::ActorId;
use caelumora_domain::group::{Group, GroupId};
use caelumora_domain::project::{Project, ProjectId};
use caelumora_domain::scope::Scope;
use caelumora_domain::session::SessionId;
use caelumora_domain::store::{StoreError, StoreResult, StoreResultExt};
use caelumora_domain::value::name::ScopedName;
use caelumora_domain::value::text::Description;
use caelumora_security::SecurityContext;
use caelumora_store::EntityStores;
use std::fmt;
use std::sync::Arc;
use tracing::{info, warn};

/// The one that makes projects, each with the group of its members - shared ones, and the own
/// projects of sessions -, and holds their lifecycle, by which the procedures of a project and of
/// a session delete, restore and purge them. The kernel's (`Kernel::project_manager`).
pub struct ProjectManager {
    /// The lifecycle of projects: each made, deleted, restored and purged by it, announced as each
    /// is but a purge (issue 593).
    projects: Lifecycle<Project>,
    /// What makes and purges the group of a project's members with it (issue 348).
    groups: Arc<GroupManager>,
}

impl fmt::Debug for ProjectManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectManager").finish_non_exhaustive()
    }
}

impl ProjectManager {
    /// The manager of the projects in `stores`, announced on `events`, each with the group of its
    /// members made by `groups`.
    pub(crate) fn new(
        stores: &EntityStores,
        events: Arc<EventBus>,
        groups: Arc<GroupManager>,
    ) -> Self {
        Self { projects: Lifecycle::new(&stores.projects, events), groups }
    }

    /// Makes the shared project `spec` asks for, of the user `ctx` acts for - in the global scope
    /// or their own -, with nobody among its members yet (`CreateProject`, asked by the store),
    /// announced as it is stored (`Lifecycle`), then the group of its members in the kernel's name:
    /// asked first what the caller may, as nobody hears of it by the group yet. If the group cannot
    /// be made, the project is marked deleted again, announced so, and purged.
    ///
    /// # Errors
    /// `OwnerNotAUser`, `InvalidScope` for another scope, `Invalid` for a name or a description
    /// that does not fit, or a `StoreError` - `NameTaken`, `PermissionDenied`, and `Validation` for
    /// the name `own` or another user's scope.
    pub async fn create(&self, ctx: &SecurityContext, spec: ProjectSpec) -> ProjectResult<Project> {
        let ProjectSpec { name, scope, description } = spec;
        let ActorId::User(owner) = ctx.principal_id else {
            return Err(ProjectError::OwnerNotAUser);
        };
        require_shared(&scope)?;
        let mut project = Project::new(ScopedName::new(&name, scope)?, owner);
        project.description = Description::optional(description.as_deref())?;
        self.projects.create(ctx, &mut project).await?;
        if let Err(e) = self.groups.keep_participants(Group::of_project(&project)).await {
            if let Err(cleanup) = self.projects.discard(&project).await {
                warn!(project = %project.id, "A project whose group could not be made is left: {cleanup}");
            }
            return Err(e.into());
        }
        info!(project = %project.id, by = %ctx.caller_id, "Project created");
        Ok(project)
    }

    /// The project of the name `spec` asks for in exactly its scope, among those `ctx` may read;
    /// else a new one, made as `create` makes it - with whether it was made. One made by another
    /// meanwhile is looked up once more (`found_or_made`, issue 250).
    ///
    /// # Errors
    /// `InvalidScope` for a scope no shared project lies in, `Invalid` for a name that does not
    /// fit; what `create` reports - `NameTaken` if the one of its name lies where `ctx` may not
    /// read it -; whatever else the store reports.
    pub async fn find_or_create(
        &self,
        ctx: &SecurityContext,
        spec: &ProjectSpec,
    ) -> ProjectResult<(Project, bool)> {
        require_shared(&spec.scope)?;
        let name = ScopedName::new(&spec.name, spec.scope.clone())?;
        found_or_made(
            || async { Ok(self.projects.store().find_by_name(ctx, &name).await.optional()?) },
            || self.create(ctx, spec.clone()),
            |e| matches!(e, ProjectError::Store(StoreError::NameTaken { .. })),
        )
        .await
    }

    /// The own project of session `id`, if it is there: the one in its scope, found by its name
    /// (`Project::own_name`) in the kernel's name - none once it was lifted (issue 467).
    ///
    /// # Errors
    /// Whatever the store reports.
    pub(crate) async fn load_own(&self, id: SessionId) -> StoreResult<Option<Project>> {
        let system = SecurityContext::system();
        self.projects.store().find_by_name(&system, &Project::own_name(id)).await.optional()
    }

    /// Makes `own`, a session's own project, with the group of its members, in the kernel's name
    /// (issue 467), each announced as it is made - the group first, so that its members hear of
    /// the project, and both, or, if the project cannot be made, neither -: as a session is made,
    /// and for a procedure that moves a session into a new own (`procedure::session::move_to`);
    /// taken back by `discard_own`.
    ///
    /// # Errors
    /// Whatever the store reports.
    pub(crate) async fn make_own(&self, own: &mut Project) -> StoreResult<()> {
        self.groups.keep_participants(Group::of_project(own)).await?;
        if let Err(error) = self.projects.create(&SecurityContext::system(), own).await {
            let group = GroupId::of_project(own.id);
            if let Err(undo) = self.groups.discard_participants(group).await {
                warn!(project = %own.id, "The group of a session's own project that could not be made is left: {undo}");
            }
            return Err(error);
        }
        Ok(())
    }

    /// Takes `own`, made as a session's own project and not to be kept, back, in the kernel's name:
    /// marked deleted and announced so, as its making was announced, and purged
    /// (`Lifecycle::discard`), the group of its members with it - as making a session fails, and
    /// for the compensation of moving a session into it.
    ///
    /// # Errors
    /// Whatever the store reports.
    pub(crate) async fn discard_own(&self, own: &Project) -> StoreResult<()> {
        self.projects.discard(own).await?;
        self.groups.discard_participants(GroupId::of_project(own.id)).await
    }

    /// Removes `own`, the own project of a session, for good with the group of its members, in the
    /// kernel's name - marked deleted first if it is not -: as the session is purged. What lay in
    /// its scope is the caller's to have purged. Announces nothing.
    ///
    /// # Errors
    /// Whatever the store reports.
    pub(crate) async fn purge_own(&self, own: ProjectId) -> StoreResult<()> {
        self.groups.purge_participants(GroupId::of_project(own)).await?;
        self.projects.purge_for_good(own).await?;
        Ok(())
    }

    /// The lifecycle of projects, shared ones and the own projects of sessions, by which the
    /// procedures of a project and of a session delete, restore and purge them
    /// (`procedure::project`, `procedure::session`).
    pub(crate) fn lifecycle(&self) -> &Lifecycle<Project> {
        &self.projects
    }
}

/// Whether a shared project lies in `scope`: the global one or a user's own - a session's holds
/// its own project, which goes with the session.
///
/// # Errors
/// `InvalidScope` for another scope.
fn require_shared(scope: &Scope) -> ProjectResult<()> {
    match scope {
        Scope::GlobalScope | Scope::UserScope { .. } => Ok(()),
        _ => Err(ProjectError::InvalidScope),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestKernel;
    use caelumora_domain::user::UserId;

    /// A project is found by its name where it is asked for, or made there: whoever asks twice, or
    /// two at once, get the one project, made once.
    #[tokio::test]
    async fn a_project_is_found_by_its_name_or_made_once() {
        let kernel = TestKernel::default().build();
        let projects = kernel.project_manager();
        let owner = UserId::new();
        let ctx =
            SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let spec = ProjectSpec::new("report", Scope::UserScope { user_id: owner });

        let (one, other) = tokio::join!(
            projects.find_or_create(&ctx, &spec),
            projects.find_or_create(&ctx, &spec)
        );
        let ((one, one_made), (other, other_made)) = (one.unwrap(), other.unwrap());
        assert_eq!(one.id, other.id, "one project");
        assert!(one_made != other_made, "made once: {one_made}, {other_made}");
        let (again, made) = projects.find_or_create(&ctx, &spec).await.unwrap();
        assert_eq!((again.id, made), (one.id, false), "found the next time");
    }
}
