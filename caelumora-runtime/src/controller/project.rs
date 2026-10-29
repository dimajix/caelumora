//! `ProjectController`: the one writer of a live project in this instance (issue 467).

use super::document::{LiveDocument, Writing};
use crate::group::{keep_participants, prune_gone_groups, require_nameable};
use crate::project::{store_error, ProjectError, ProjectResult};
use arc_swap::Guard;
use caelumora_model::actor::ActorId;
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::group::Group;
use caelumora_model::project::{Project, ProjectId, ProjectUpdateEvent};
use caelumora_model::scope::Scope;
use caelumora_model::security::{
    AccessPolicy, AclEntry, ActionType, Enforcement, Grant, SecurityContext, Subject,
};
use caelumora_model::store::{
    EntityStore, EntityStores, StoreError, StoreResult, StoreResultExt, Stored, Updatable,
};
use caelumora_model::user::{User, UserId};
use std::sync::Arc;
use tracing::error;

/// The live project in this instance (issue 467), shared by everything that uses it - the
/// sessions that lie in it, a description a running system reads: the one writer of what it is -
/// its name, its description, its members, its access list and where it lies. Making, deleting,
/// restoring and purging it are the `ProjectManager`'s, or, for a session's own, the
/// `SessionManager`'s.
///
/// Every change reaches the project first as the views reach it - by the store, in the caller's
/// name -, so that whoever does not reach it learns nothing else of it, and is applied to it as
/// stored: a change made elsewhere may not have reached the live copy yet, an access list never
/// does by its announcement. Changing it is its owner's (`UpdateProject`, asked by the store as
/// the change is stored).
#[derive(Debug)]
pub struct ProjectController {
    project: LiveDocument<Project>,
    users: Arc<dyn EntityStore<User>>,
    groups: Arc<dyn EntityStore<Group>>,
    /// Asked whether the caller may change it before anything of others is asked (issue 312).
    access_policy: Arc<dyn AccessPolicy>,
}

impl ProjectController {
    /// The live `project`. Made by the `ControllerManager`, which keeps one per project.
    pub(crate) fn new(
        project: Project,
        event_publisher: Arc<dyn EventPublisher>,
        stores: &EntityStores,
        access_policy: Arc<dyn AccessPolicy>,
    ) -> Self {
        Self {
            project: LiveDocument::new(
                project.id,
                project,
                Arc::clone(&stores.projects),
                event_publisher,
            ),
            users: Arc::clone(&stores.users),
            groups: Arc::clone(&stores.groups),
            access_policy,
        }
    }

    pub fn load(&self) -> Guard<Arc<Project>> {
        self.project.load()
    }

    pub fn id(&self) -> ProjectId {
        *self.project.address()
    }

    /// Renames the project where it lies, on behalf of `ctx`.
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn update_name(&self, ctx: &SecurityContext, name: &str) -> ProjectResult<()> {
        self.update(ctx, ProjectUpdateEvent::NameChanged(name.to_string())).await
    }

    /// Says what the project is for; `None` or empty takes it away.
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn update_description(
        &self,
        ctx: &SecurityContext,
        description: Option<&str>,
    ) -> ProjectResult<()> {
        self.update(ctx, ProjectUpdateEvent::DescriptionChanged(description.map(String::from)))
            .await
    }

    /// Makes `members` the users who work on the project, on behalf of `ctx`: reached, then asked
    /// whether they may change it (`UpdateProject`) before whether each member exists is asked in
    /// the runtime's name (issue 312). The group of its members follows once it is stored, as a
    /// channel's participants' does; who is no longer among them reads it no longer, and learns
    /// so.
    ///
    /// # Errors
    /// `UnknownMember` for a user that does not exist; `StoreError::Validation` for a session's own
    /// project, which has none; otherwise as [`Self::update`].
    pub async fn update_members(
        &self,
        ctx: &SecurityContext,
        members: Vec<UserId>,
    ) -> ProjectResult<()> {
        let mut writing = self.project.write().await;
        let mut project = writing.reached(ctx).await.map_err(store_error(self.id()))?;
        self.access_policy
            .require(
                ctx,
                ActionType::update(Project::KIND, &project.security_attributes),
                StoreError::PermissionDenied,
            )
            .await?;
        let system = SecurityContext::system();
        let mut wanted = Vec::with_capacity(members.len());
        for member in members {
            if self.users.get(&system, member).await.optional()?.is_none() {
                return Err(ProjectError::UnknownMember(member));
            }
            if !wanted.contains(&member) {
                wanted.push(member);
            }
        }
        let left: Vec<Subject> = project
            .members
            .iter()
            .filter(|member| !wanted.contains(member))
            .map(|member| Subject::Actor(ActorId::User(*member)))
            .collect();
        let event = ProjectUpdateEvent::MembersChanged(wanted);
        project.apply(&event)?;
        let project = self.store(ctx, &writing, project).await?;
        if let Err(e) = keep_participants(self.groups.as_ref(), Group::of_project(&project)).await {
            error!(project = %project.id, "The group of the project's members does not follow them: {e}");
        }
        writing.announce_also_to(
            GlobalEvent::updated::<Project>(&project.id, &event),
            &project,
            left,
        );
        Ok(())
    }

    /// Gives the project the access list `acl` on behalf of `ctx`: reached as the views reach it,
    /// each group the list newly names read - who names one must read it (issue 286) -, then
    /// stored, which is where its owner's act is asked (`UpdateProject`). Announced without the
    /// list, which only its owner sees.
    ///
    /// # Errors
    /// `NotFound` if the caller does not reach it, a `StoreError` - `PermissionDenied` for anyone
    /// but its owner, `Validation` for an entry that names an agent, the system or nobody, or a
    /// group the caller cannot read.
    pub async fn update_access(
        &self,
        ctx: &SecurityContext,
        acl: Vec<AclEntry>,
    ) -> ProjectResult<()> {
        let mut writing = self.project.write().await;
        let project = writing.reached(ctx).await.map_err(store_error(self.id()))?;
        self.store_access(ctx, &writing, project, acl).await
    }

    /// Lets `subject` reach the project as far as `grant` reaches, on behalf of `ctx`: an entry
    /// added to its access list, unless one already grants `subject` as much - one that grants
    /// less is widened. In the runtime's name where a session shares the project it lies in with
    /// its participants: the session's act was asked.
    ///
    /// # Errors
    /// As [`Self::update_access`].
    pub async fn share(
        &self,
        ctx: &SecurityContext,
        subject: Subject,
        grant: Grant,
    ) -> ProjectResult<()> {
        let mut writing = self.project.write().await;
        let project = writing.reached(ctx).await.map_err(store_error(self.id()))?;
        let mut acl = project.security_attributes.acl.clone();
        if acl.iter().any(|entry| entry.subject == subject && entry.grant.covers(grant)) {
            return Ok(());
        }
        acl.retain(|entry| entry.subject != subject);
        acl.push(AclEntry { subject, grant });
        self.store_access(ctx, &writing, project, acl).await
    }

    /// Takes away the entry of `subject` from the project's access list, on behalf of `ctx`, if
    /// there is one: a session that leaves the project, in the runtime's name.
    ///
    /// # Errors
    /// As [`Self::update_access`].
    pub async fn unshare(&self, ctx: &SecurityContext, subject: Subject) -> ProjectResult<()> {
        let mut writing = self.project.write().await;
        let project = writing.reached(ctx).await.map_err(store_error(self.id()))?;
        if !project.security_attributes.acl.iter().any(|entry| entry.subject == subject) {
            return Ok(());
        }
        let acl = project
            .security_attributes
            .acl
            .iter()
            .filter(|entry| entry.subject != subject)
            .cloned()
            .collect();
        self.store_access(ctx, &writing, project, acl).await
    }

    /// Lifts a session's own project into `scope` - its owner's or the global one - under `name`,
    /// on behalf of `ctx` (issue 467): it becomes a shared project with all that lies in it, the
    /// session included, which no longer takes it along.
    ///
    /// Reached first, so that who does not reach it learns nothing of it, and refused as stored
    /// under the lock of its writing, so that it is lifted once.
    ///
    /// # Errors
    /// `NotFound` if the caller does not reach it; `InvalidScope` for a project that is shared
    /// already or a scope a shared project does not lie in; otherwise as [`Self::update`] -
    /// `StoreError::Validation` for the name `own` or another user's scope, `NameTaken` where the
    /// name is taken.
    pub async fn lift(&self, ctx: &SecurityContext, name: &str, scope: Scope) -> ProjectResult<()> {
        let mut writing = self.project.write().await;
        let mut project = writing.reached(ctx).await.map_err(store_error(self.id()))?;
        if !project.is_own() || !matches!(scope, Scope::GlobalScope | Scope::UserScope { .. }) {
            return Err(ProjectError::InvalidScope);
        }
        let event = ProjectUpdateEvent::Lifted { name: name.to_string(), scope };
        project.apply(&event)?;
        let project = self.store(ctx, &writing, project).await?;
        writing.announce_update(&event, &project);
        Ok(())
    }

    /// Takes over a change made elsewhere - already stored there - once it is announced: read
    /// again as stored, in the runtime's name, since the announcement of a new access list does
    /// not carry it.
    ///
    /// # Errors
    /// Whatever the store reports; the live copy is then unchanged.
    pub async fn process_global_event(&self, event: &GlobalEvent) -> StoreResult<()> {
        if event.about::<Project>().is_some_and(|(id, change)| {
            id == self.id() && change.update::<ProjectUpdateEvent>().is_some()
        }) {
            self.project.reload().await?;
        }
        Ok(())
    }

    /// Applies `event` to the project on behalf of `ctx`: reached as the views reach it, stored,
    /// which is where the permission is checked (`UpdateProject`), then taken over live and
    /// announced.
    ///
    /// # Errors
    /// `NotFound` if the caller does not reach it, `Invalid` if what `event` says does not fit, or
    /// whatever the store reports.
    async fn update(&self, ctx: &SecurityContext, event: ProjectUpdateEvent) -> ProjectResult<()> {
        let mut writing = self.project.write().await;
        let mut project = writing.reached(ctx).await.map_err(store_error(self.id()))?;
        project.apply(&event)?;
        let project = self.store(ctx, &writing, project).await?;
        writing.announce_update(&event, &project);
        Ok(())
    }

    /// Gives the reached `project` the access list `acl`, as `update_access` says, under
    /// `writing`. Announced without the list (`ProjectUpdateEvent::AccessChanged`).
    async fn store_access(
        &self,
        ctx: &SecurityContext,
        writing: &Writing<'_, Project>,
        mut project: Project,
        acl: Vec<AclEntry>,
    ) -> ProjectResult<()> {
        require_nameable(self.groups.as_ref(), ctx, &project.security_attributes.acl, &acl).await?;
        project.security_attributes.acl = acl;
        let project = self.store(ctx, writing, project).await?;
        writing.announce_update(&ProjectUpdateEvent::AccessChanged, &project);
        Ok(())
    }

    /// Stores `project` as changed by `ctx` under `writing` - its access list without the entries
    /// naming a group that is gone (issue 347) - and takes it over live.
    async fn store(
        &self,
        ctx: &SecurityContext,
        writing: &Writing<'_, Project>,
        mut project: Project,
    ) -> ProjectResult<Arc<Project>> {
        prune_gone_groups(self.groups.as_ref(), [&mut project.security_attributes.acl]).await;
        writing.store(ctx, project).await.map_err(store_error(self.id()))
    }
}
