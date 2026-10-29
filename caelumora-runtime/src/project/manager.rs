//! `ProjectManager`: making, deleting, restoring and purging shared projects (issue 467), with
//! the sessions that lie in them and what lies in their scope.

use super::{store_error, ProjectError, ProjectResult};
use crate::error::is_forbidden;
use crate::event::EventBus;
use crate::group::{keep_participants, GroupManager};
use crate::scoped::{purge_for_good, ScopeNotEmpty, ScopedEntities};
use crate::session::{SessionError, SessionManager};
use caelumora_model::actor::ActorId;
use caelumora_model::event::{Announcing, GlobalEvent};
use caelumora_model::group::{Group, GroupId};
use caelumora_model::project::{Project, ProjectId};
use caelumora_model::removal::Removal;
use caelumora_model::scope::Scope;
use caelumora_model::security::{AccessPolicy, ActionType, Enforcement, SecurityContext};
use caelumora_model::session::{Session, SessionField};
use caelumora_model::store::query::{Filter, Query};
use caelumora_model::store::{Deleted, EntityStores, StoreError, StoreResultExt, Stored};
use caelumora_model::value::name::ScopedName;
use caelumora_model::value::text::Description;
use dashmap::DashMap;
use std::fmt;
use std::slice;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// The one that makes, deletes, restores and purges shared projects, each with the group of its
/// members. Made by the program beside the `SessionManager`, which it holds: deleting a project
/// deletes the sessions that lie in it, and only the `SessionManager` deletes a session - it stops
/// what runs in it.
pub struct ProjectManager {
    /// What deletes, restores and purges the sessions that lie in a project.
    sessions: Arc<SessionManager>,
    stores: EntityStores,
    /// What deletes and restores the group of a project's members with it.
    groups: Arc<GroupManager>,
    events: Arc<EventBus>,
    access_policy: Arc<dyn AccessPolicy>,
    /// What goes with a project deleted, restored or purged.
    scoped: Arc<ScopedEntities>,
    /// One lock per project id, held while that project is deleted, restored or purged, so that
    /// none of them meets another half done: each goes through the project, its sessions, its
    /// scope and its group, more than one store write (issue 492) - as the `SessionManager` holds
    /// one per session.
    lifecycles: DashMap<ProjectId, Arc<Mutex<()>>>,
}

impl fmt::Debug for ProjectManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectManager").finish_non_exhaustive()
    }
}

impl ProjectManager {
    /// The manager of the projects of the runtime `sessions` works with.
    pub fn new(sessions: Arc<SessionManager>) -> Self {
        let runtime = Arc::clone(sessions.runtime());
        Self {
            stores: runtime.entity_stores().clone(),
            groups: Arc::clone(runtime.group_manager()),
            events: Arc::clone(runtime.event_bus()),
            access_policy: Arc::clone(runtime.access_policy()),
            scoped: Arc::clone(runtime.scoped_entities()),
            sessions,
            lifecycles: DashMap::new(),
        }
    }

    /// The lock held while project `id` is deleted, restored or purged.
    fn lifecycle_lock(&self, id: ProjectId) -> Arc<Mutex<()>> {
        Arc::clone(&self.lifecycles.entry(id).or_default())
    }

    /// Makes a shared project of the user `ctx` acts for, of `name` in `scope` - the global one or
    /// their own -, described by `description`, with nobody among its members yet
    /// (`CreateProject`, asked by the store), and the group of its members in the runtime's name.
    ///
    /// # Errors
    /// `OwnerNotAUser`, `InvalidScope` for another scope, `Invalid` for a name or a description
    /// that does not fit, or a `StoreError` - `NameTaken`, `PermissionDenied`, and `Validation` for
    /// the name `own` or another user's scope.
    pub async fn create(
        &self,
        ctx: &SecurityContext,
        name: &str,
        scope: Scope,
        description: Option<&str>,
    ) -> ProjectResult<Project> {
        let ActorId::User(owner) = ctx.principal_id else {
            return Err(ProjectError::OwnerNotAUser);
        };
        if !matches!(scope, Scope::GlobalScope | Scope::UserScope { .. }) {
            return Err(ProjectError::InvalidScope);
        }
        let mut project = Project::new(ScopedName::new(name, scope)?, owner);
        project.description = Description::optional(description)?;
        self.stores.projects.create(ctx, &project).await?;
        if let Err(e) =
            keep_participants(self.stores.groups.as_ref(), Group::of_project(&project)).await
        {
            if let Err(cleanup) = purge_for_good(self.stores.projects.as_ref(), project.id).await {
                warn!(project = %project.id, "A project whose group could not be made is left: {cleanup}");
            }
            return Err(e.into());
        }
        self.events.announce(GlobalEvent::created::<Project>(&project.id), &project);
        info!(project = %project.id, by = %ctx.caller_id, "Project created");
        Ok(project)
    }

    /// Deletes project `id` on behalf of `ctx` (`DeleteProject`): marked first, then the sessions
    /// that lie in it - each through the `SessionManager`, in `ctx`'s name, with what lies in its
    /// scope -, then what lies in its scope (`ScopedEntities`), as far as `ctx` may delete it, then
    /// the group of its members in the runtime's name - all of it with the project's mark, by
    /// `ctx`'s caller (issue 469). If a session or something of its scope is left that `ctx` may
    /// not delete, the project is kept, its mark taken away again, and what they could delete stays
    /// deleted. `false` if there was none the caller could see.
    ///
    /// # Errors
    /// `InvalidScope` for a session's own, which goes with the session; `ScopeNotEmpty` with how
    /// many sessions and what else are left; `Session` if deleting a session failed otherwise; or
    /// whatever the store reports, `PermissionDenied` included.
    pub async fn delete(&self, ctx: &SecurityContext, id: ProjectId) -> ProjectResult<bool> {
        let lifecycle = self.lifecycle_lock(id);
        let _lifecycle = lifecycle.lock().await;
        let Some(project) = self.stores.projects.get(ctx, id).await.optional()? else {
            return Ok(false);
        };
        if project.is_own() {
            return Err(ProjectError::InvalidScope);
        }
        // One mark for all that goes with it (issue 469); marked before its scope is gone through,
        // so that nothing is put into it afterwards (`ScopeHolders`, issue 458).
        let removal = Removal::now(ctx.caller_id);
        if !self.stores.projects.delete(ctx, id, &removal).await? {
            return Ok(false);
        }
        let holder = project.holds();
        let deleted = async {
            let mut left = Vec::new();
            let sessions = self.sessions_in(id).await?;
            let kept = self.delete_sessions(ctx, &sessions, &removal).await?;
            if kept > 0 {
                left.push((Session::KIND, kept));
            }
            left.extend(
                self.scoped.delete_lying_in(ctx, slice::from_ref(&holder), &removal).await?,
            );
            ScopeNotEmpty::require_empty(&holder, left)?;
            self.groups.delete_participants(slice::from_ref(&holder), &removal).await?;
            Ok::<_, ProjectError>(())
        }
        .await;
        if let Err(error) = deleted {
            if let Err(undo) = self.stores.projects.restore(&SecurityContext::system(), id).await {
                warn!(project = %id, "A project could not be deleted and not be unmarked again: {undo}");
            }
            return Err(error);
        }
        self.events.announce(GlobalEvent::deleted::<Project>(&id), &project);
        info!(project = %id, by = %ctx.caller_id, "Project deleted");
        Ok(true)
    }

    /// Restores the deleted project `id` on behalf of `ctx` (`Restore`): its mark taken away and
    /// the group of its members restored in the runtime's name - both, or neither: it keeps its
    /// mark then, so that it can be tried again (issue 469) -, then the sessions deleted with it
    /// and what lay in its scope - what carries its mark -, as far as `ctx` may restore them; what
    /// they may not stays deleted, and the project comes back all the same.
    ///
    /// # Errors
    /// `NotFound` if it is not deleted or not the caller's to see, `InvalidScope` for a session's
    /// own, or whatever else the store reports - `NameTaken` among it.
    pub async fn restore(&self, ctx: &SecurityContext, id: ProjectId) -> ProjectResult<Project> {
        let lifecycle = self.lifecycle_lock(id);
        let _lifecycle = lifecycle.lock().await;
        // What went with it carries its mark (issue 469).
        let Deleted { entity: deleted, removal } =
            self.stores.projects.get_deleted(ctx, id).await.map_err(store_error(id))?;
        if deleted.is_own() {
            return Err(ProjectError::InvalidScope);
        }
        let project = self.stores.projects.restore(ctx, id).await.map_err(store_error(id))?;
        let holder = project.holds();
        // If its members cannot come back, it is marked again as it was, so that a restoring tried
        // again finds what went with it.
        if let Err(error) =
            self.groups.restore_participants(slice::from_ref(&holder), &removal).await
        {
            if let Err(undo) =
                self.stores.projects.delete(&SecurityContext::system(), id, &removal).await
            {
                warn!(project = %id, "A project could not be restored and not be marked deleted again: {undo}");
            }
            return Err(error.into());
        }
        for session in self
            .stores
            .sessions
            .list_deleted_with(&SecurityContext::system(), removal.id, &in_project(id))
            .await?
            .items
        {
            if let Err(e) = self.sessions.restore(ctx, session.id).await {
                info!(project = %id, session = %session.id, "A session of a restored project stays deleted: {e}");
            }
        }
        if let Err(e) = self.scoped.restore_lying_in(ctx, slice::from_ref(&holder), &removal).await
        {
            warn!(project = %id, "The project was restored, but not what lies in its scope: {e}");
        }
        self.events.announce(GlobalEvent::restored::<Project>(&id), &project);
        info!(project = %id, by = %ctx.caller_id, "Project restored");
        Ok(project)
    }

    /// Removes the deleted project `id` for good on behalf of `ctx` (`Purge`, asked before
    /// anything is removed): in the runtime's name, the sessions deleted that lie in it, what lies
    /// in its scope and the group of its members; then the project. Announces nothing, as no purge
    /// does. `false` if there was nothing deleted the caller could see.
    ///
    /// # Errors
    /// `InvalidScope` for a session's own, `PermissionDenied`, `Session` if purging a session
    /// failed, or whatever the store reports; what was purged before stays purged.
    pub async fn purge(&self, ctx: &SecurityContext, id: ProjectId) -> ProjectResult<bool> {
        let lifecycle = self.lifecycle_lock(id);
        let _lifecycle = lifecycle.lock().await;
        let Some(Deleted { entity: deleted, .. }) =
            self.stores.projects.get_deleted(ctx, id).await.optional()?
        else {
            return Ok(false);
        };
        if deleted.is_own() {
            return Err(ProjectError::InvalidScope);
        }
        self.access_policy
            .require(
                ctx,
                ActionType::purge(Project::KIND, &deleted.security_attributes),
                StoreError::PermissionDenied,
            )
            .await?;
        let system = SecurityContext::system();
        for Deleted { entity: session, .. } in self.deleted_sessions_in(id).await? {
            self.sessions.purge(&system, session.id).await.map_err(ProjectError::Session)?;
        }
        self.scoped.purge_lying_in(slice::from_ref(&deleted.holds())).await?;
        purge_for_good(self.stores.groups.as_ref(), GroupId::of_project(id)).await?;
        let purged = self.stores.projects.purge(ctx, id).await?;
        // Gone for good: nothing of it is deleted, restored or purged again.
        self.lifecycles.remove(&id);
        info!(project = %id, by = %ctx.caller_id, "Project purged");
        Ok(purged)
    }

    /// The sessions that lie in project `id`, read in the runtime's name: all of them are found,
    /// whatever the caller may read.
    async fn sessions_in(&self, id: ProjectId) -> ProjectResult<Vec<Session>> {
        Ok(self.stores.sessions.list(&SecurityContext::system(), &in_project(id)).await?.items)
    }

    /// The deleted sessions that lie in project `id`, read in the runtime's name.
    async fn deleted_sessions_in(&self, id: ProjectId) -> ProjectResult<Vec<Deleted<Session>>> {
        Ok(self
            .stores
            .sessions
            .list_deleted(&SecurityContext::system(), &in_project(id))
            .await?
            .items)
    }

    /// Deletes `sessions` on behalf of `ctx` through the `SessionManager`, as a project they lie
    /// in is deleted, marked with `removal`, its mark; answers how many are left - not the caller's
    /// to read or delete, or with something in their scope that is not.
    ///
    /// # Errors
    /// `Session` if deleting one failed otherwise; what was deleted before stays deleted.
    async fn delete_sessions(
        &self,
        ctx: &SecurityContext,
        sessions: &[Session],
        removal: &Removal,
    ) -> ProjectResult<usize> {
        let mut kept = 0;
        for session in sessions {
            match self.sessions.delete_with(ctx, session.id, removal).await {
                Ok(_) => {}
                Err(e)
                    if is_forbidden(&e)
                        || matches!(
                            e.downcast_ref::<SessionError>(),
                            Some(SessionError::NotFound(_) | SessionError::ScopeNotEmpty(_))
                        ) =>
                {
                    kept += 1
                }
                Err(e) => return Err(ProjectError::Session(e)),
            }
        }
        Ok(kept)
    }
}

/// The sessions that lie in project `id`.
fn in_project(id: ProjectId) -> Query<SessionField> {
    Query::filtered(Filter::in_project(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::testing::{create, failing_runtime};
    use caelumora_model::component::ComponentBundle;
    use caelumora_model::security::SecurityAttributes;
    use caelumora_model::session::testing::make_session;
    use caelumora_model::user::UserId;
    use std::sync::atomic::Ordering;

    /// Issue 469: a project whose members' group cannot come back stays deleted, marked as it was,
    /// and tried again, comes back with its sessions and what lay in its scope.
    #[tokio::test]
    async fn a_project_whose_restoring_failed_comes_back_whole_when_tried_again() {
        let (runtime, passing) = failing_runtime(Group::KIND);
        let stores = runtime.entity_stores().clone();
        let manager = Arc::new(SessionManager::new(runtime));
        let projects = ProjectManager::new(Arc::clone(&manager));
        let owner = UserId::new();
        let ctx =
            SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let system = SecurityContext::system();
        let session = create(&manager, &ctx, make_session("in it", owner)).await.unwrap();
        let project = projects
            .create(&ctx, "report", Scope::UserScope { user_id: owner }, None)
            .await
            .unwrap();
        session.update_project(&ctx, Some(project.id)).await.unwrap();
        let bundle = ComponentBundle::new(
            ScopedName::new("tools", project.holds()).unwrap(),
            Default::default(),
            SecurityAttributes::systems(),
        );
        stores.component_bundles.create(&system, &bundle).await.unwrap();
        projects.delete(&ctx, project.id).await.unwrap();
        let mark = stores.projects.get_deleted(&system, project.id).await.unwrap().removal;

        passing.store(0, Ordering::SeqCst);
        projects.restore(&ctx, project.id).await.expect_err("its members cannot come back");
        assert_eq!(
            stores.projects.get_deleted(&system, project.id).await.unwrap().removal,
            mark,
            "it stays deleted, marked as it was"
        );

        passing.store(usize::MAX, Ordering::SeqCst);
        projects.restore(&ctx, project.id).await.expect("restored when tried again");
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
}
