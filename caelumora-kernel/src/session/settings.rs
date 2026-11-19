//! Changing a running session: its name, language, time zone, privacy, approval rules,
//! description, workspace and project.

use super::{
    held_workspace, load_own, make_own, purge_own, require_attachable, SessionError, SessionLoop,
};
use crate::controller::WorkspaceController;
use crate::procedure::Admitted;
use crate::project;
use anyhow::Result;
use caelumora_domain::approval::ApprovalRule;
use caelumora_domain::event::{Announcing, GlobalEvent};
use caelumora_domain::group::GroupId;
use caelumora_domain::privacy::PrivacyConfig;
use caelumora_domain::project::{Project, ProjectId};
use caelumora_domain::scope::Scope;
use caelumora_domain::security::{Grant, Subject};
use caelumora_domain::session::Session;
use caelumora_domain::store::{StoreError, StoreResultExt, Stored};
use caelumora_domain::workspace::WorkspaceId;
use caelumora_security::{ActionType, Enforcement, SecurityContext};
use std::sync::Arc;
use tracing::info;

/// The project a session is to be moved into (`SessionLoop::project_to_move_to`): what moving it
/// there does besides storing it as lying there differs by it.
#[derive(Debug)]
pub(crate) enum MoveTarget {
    /// A shared project, shared with the session's participants for reading first.
    Shared(Project),
    /// Its own, which its participants read already.
    Own(Project),
    /// A new own project, made first - the one in its scope was lifted (issue 467).
    NewOwn(Project),
}

impl MoveTarget {
    /// The project it names.
    pub(crate) fn project(&self) -> &Project {
        match self {
            MoveTarget::Shared(project)
            | MoveTarget::Own(project)
            | MoveTarget::NewOwn(project) => project,
        }
    }
}

impl SessionLoop {
    /// Renames the session on behalf of `context`.
    ///
    /// # Errors
    /// `StoreError::Invalid` if `new_name` is no name,
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// session, or whatever else storing it reports.
    pub async fn update_name(&self, context: &SecurityContext, new_name: &str) -> Result<()> {
        Ok(self.session_controller.update_name(context, new_name).await?)
    }

    /// Has the channels of the session that hold no language of their own
    /// held in `language`, or in none in particular, on behalf of `context`
    /// (issue 425): stored first, which is where the permission is checked
    /// (`UpdateSession`), like a new name. The agents of those channels are
    /// told from their next turn on; nothing said before is translated.
    ///
    /// # Errors
    /// `StoreError::Invalid` if `language` is no language tag,
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// session, or whatever else storing it reports.
    pub async fn update_language(
        &self,
        context: &SecurityContext,
        language: Option<&str>,
    ) -> Result<()> {
        Ok(self.session_controller.update_language(context, language).await?)
    }

    /// Has the session held in the time zone `time_zone`, or in UTC, on
    /// behalf of `context` (issue 424): stored first, which is where the
    /// permission is checked (`UpdateSession`), like a new name. Its agents'
    /// models are given the times of messages in it from their next turn on.
    ///
    /// # Errors
    /// `StoreError::Invalid` if `time_zone` is no time zone,
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// session, or whatever else storing it reports.
    pub async fn update_time_zone(
        &self,
        context: &SecurityContext,
        time_zone: Option<&str>,
    ) -> Result<()> {
        Ok(self.session_controller.update_time_zone(context, time_zone).await?)
    }

    /// Changes whom the session lets read what was said in it (issue 134)
    /// on behalf of `context`. What was remembered before keeps the privacy
    /// it was said under: a wider policy lets go only what is said from now
    /// on, a narrower one takes back what it let go.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// session, or whatever else storing it reports.
    pub async fn update_privacy(
        &self,
        context: &SecurityContext,
        privacy: PrivacyConfig,
    ) -> Result<()> {
        Ok(self.session_controller.update_privacy(context, privacy).await?)
    }

    /// Sets what the session restricts of the tool calls made in any of its
    /// channels (issue 13) on behalf of `context`: stored first, which is
    /// where the permission is checked (`UpdateSession`). Its agents' next
    /// calls are decided by them.
    ///
    /// # Errors
    /// `StoreError::Validation` if a rule does not only ask or deny,
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// session, or whatever else storing it reports.
    pub async fn update_approval_rules(
        &self,
        context: &SecurityContext,
        rules: Vec<ApprovalRule>,
    ) -> Result<()> {
        Ok(self.session_controller.update_approval_rules(context, rules).await?)
    }

    /// Says what the session is for (issue 245) on behalf of `context`:
    /// stored first, which is where the permission is checked
    /// (`UpdateSession`). The agents of all its channels are told from their
    /// next turn on.
    ///
    /// # Errors
    /// `StoreError::Validation` if `summary` or `briefing` is too long or
    /// holds what it may not, `StoreError::PermissionDenied` if `context`
    /// may not change the session, or whatever else storing it reports.
    pub async fn update_description(
        &self,
        context: &SecurityContext,
        summary: Option<&str>,
        briefing: Option<&str>,
    ) -> Result<()> {
        Ok(self.session_controller.update_description(context, summary, briefing).await?)
    }

    /// Attaches `workspace` to the session, or with `None` takes the one
    /// attached away (issue 253), on behalf of `context`. Whoever may change
    /// the session asks first (`UpdateSession`); to attach one, it must be
    /// theirs to share (`GetWorkspace`, `UpdateWorkspace` - its owner,
    /// issue 251, decision 12) and lie where this session may use it - not
    /// in another session's scope. Taking it away shares nothing and asks
    /// no more. Its channels use it as their classifications and switches
    /// let them, from their next use - and as its access list lets each
    /// participant (issue 23): attaching shares nothing by itself, unless `share`
    /// says how far the session's participants may use it, which adds an entry
    /// for them. Taking it away leaves the list as it is: it is the
    /// workspace's, and its owner's to change.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if `context` may not change the session; what
    /// `require_attachable` reports of the workspace - `WorkspaceError::NotFound` if there is none
    /// they may read or are cleared for, `WorkspaceError::Store(StoreError::PermissionDenied)` if
    /// it is not theirs to attach, `SessionError::WorkspaceOutOfScope` -; or whatever else storing
    /// reports.
    pub async fn update_workspace(
        &self,
        context: &SecurityContext,
        workspace: Option<WorkspaceId>,
        share: Option<Grant>,
    ) -> Result<()> {
        let session = self.session();
        self.policies
            .access_policy()
            .require(
                context,
                ActionType::update(Session::KIND, &session.security_attributes),
                StoreError::PermissionDenied,
            )
            .await?;
        if let Some(id) = workspace {
            require_attachable(&self.workspaces, context, id, self.id).await?;
        }
        let held = self.attach_workspace(context, workspace).await?;
        if let (Some(controller), Some(grant)) = (held, share) {
            controller.share(context, Subject::Group(GroupId::of_session(self.id)), grant).await?;
        }
        info!(session = %self.id, workspace = ?workspace, by = %context.caller_id, "Session's workspace changed");
        Ok(())
    }

    /// The project the session is to be put into on behalf of `context` - the shared project
    /// `project`, or with `None` its own (issue 467): the one in its scope, or a new one if that
    /// was lifted -, for a procedure that moves it (`procedure::session::move_to`), once it is
    /// asked whether `context` may change the session (`UpdateSession`) and reach the project
    /// (`GetProject`); `None` where it lies already. Stores nothing.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if `context` may not change the session,
    /// `ProjectError::NotFound` if there is no project they may read,
    /// `SessionError::ProjectOutOfScope` for another session's own, `SessionError::OwnerNotAUser`
    /// if a new own is needed and the session is no user's; whatever reading reports.
    pub(crate) async fn project_to_move_to(
        &self,
        context: &SecurityContext,
        project: Option<ProjectId>,
    ) -> Result<Option<MoveTarget>> {
        let session = self.session();
        self.policies
            .access_policy()
            .require(
                context,
                ActionType::update(Session::KIND, &session.security_attributes),
                StoreError::PermissionDenied,
            )
            .await?;
        let target =
            match project {
                Some(id) => match self.reached_project(context, id).await? {
                    own if own.is_own() => MoveTarget::Own(own),
                    shared => MoveTarget::Shared(shared),
                },
                None => match load_own(&self.stores, self.id).await? {
                    Some(own) => MoveTarget::Own(own),
                    None => MoveTarget::NewOwn(session.new_own_project().ok_or(
                        SessionError::OwnerNotAUser(session.security_attributes.owner_id),
                    )?),
                },
            };
        Ok((target.project().id != session.project).then_some(target))
    }

    /// The shared project the session lies in, read in the kernel's name; `None` where it lies in
    /// its own, or in none that is still there.
    ///
    /// # Errors
    /// Whatever reading reports.
    pub(crate) async fn load_shared_project(&self) -> Result<Option<Project>> {
        let system = SecurityContext::system();
        let project = self.session().project;
        let found = self.stores.projects.get(&system, project).await.optional()?;
        Ok(found.filter(|found| !found.is_own()))
    }

    /// Makes `own`, a new own project of the session (`MoveTarget::NewOwn`), with the group of its
    /// members, in the kernel's name (issue 467), for a procedure that moves the session into it;
    /// purged again by `purge_own_project_held`.
    ///
    /// # Errors
    /// Whatever the store reports.
    pub(crate) async fn make_own_project_held(
        &self,
        _: &Admitted<'_>,
        own: &mut Project,
    ) -> Result<()> {
        make_own(&self.stores, own).await?;
        Ok(())
    }

    /// Purges project `own`, made as the session's own, for good with the group of its members, in
    /// the kernel's name - for the compensation of moving the session into it.
    ///
    /// # Errors
    /// Whatever the store reports.
    pub(crate) async fn purge_own_project_held(
        &self,
        _: &Admitted<'_>,
        own: ProjectId,
    ) -> Result<()> {
        purge_own(&self.stores, own).await?;
        Ok(())
    }

    /// Shares project `project` with the session's participants for reading, in the kernel's name,
    /// since the session's move was asked, so that every participant publishes into it, before
    /// the session is stored as lying there; the entry is taken away by `unshare_project_held`.
    ///
    /// # Errors
    /// Whatever the project's controller reports.
    pub(crate) async fn share_project_held(
        &self,
        _: &Admitted<'_>,
        project: ProjectId,
    ) -> Result<()> {
        let participants = Subject::Group(GroupId::of_session(self.id));
        let controller = self.controllers.project_controller(project).await?;
        controller.share(&SecurityContext::system(), participants, Grant::Read).await?;
        Ok(())
    }

    /// Takes the entry of the session's participants away from project `project`, in the kernel's
    /// name - for the shared project the session left, and for the compensation of sharing one.
    ///
    /// # Errors
    /// Whatever the project's controller reports.
    pub(crate) async fn unshare_project_held(
        &self,
        _: &Admitted<'_>,
        project: ProjectId,
    ) -> Result<()> {
        let participants = Subject::Group(GroupId::of_session(self.id));
        let controller = self.controllers.project_controller(project).await?;
        controller.unshare(&SecurityContext::system(), participants).await?;
        Ok(())
    }

    /// Stores the session as lying in project `project` on behalf of `context` (`UpdateSession`,
    /// asked by the store), for a procedure that moves it (`procedure::session::move_to`). Nothing
    /// moves: what went to the project before stays there, only what comes now goes to the other.
    ///
    /// # Errors
    /// Whatever storing the session reports.
    pub(crate) async fn store_project_held(
        &self,
        _: &Admitted<'_>,
        context: &SecurityContext,
        project: ProjectId,
    ) -> Result<()> {
        self.session_controller.update_project(context, project).await?;
        Ok(())
    }

    /// Announces `own`, made as the session's new own project, once the session lies in it.
    pub(crate) fn announce_own_made(&self, own: &Project) {
        self.events.announce(GlobalEvent::created::<Project>(&own.id), own);
    }

    /// Project `id` as `context` reaches it (`GetProject`), if the session may lie in it: a shared
    /// one, or its own.
    ///
    /// # Errors
    /// `ProjectError::NotFound` if there is none they may read, `SessionError::ProjectOutOfScope`
    /// for another session's own.
    async fn reached_project(&self, context: &SecurityContext, id: ProjectId) -> Result<Project> {
        let target =
            self.stores.projects.get(context, id).await.map_err(project::store_error(id))?;
        if target.is_own() && target.name.scope != (Scope::SessionScope { session_id: self.id }) {
            return Err(SessionError::ProjectOutOfScope(id).into());
        }
        Ok(target)
    }

    /// Attaches `workspace` to the session on behalf of `context`, or with
    /// `None` takes the one attached away, and holds it live (issue 344):
    /// stored as changing the session, without asking whether it may be
    /// attached - its caller asked that (`update_workspace`), or found or made
    /// it for the session as it made the session (`SessionManager`).
    /// Returns the workspace as it is held.
    ///
    /// # Errors
    /// Whatever storing the session or reading the workspace reports.
    pub(crate) async fn attach_workspace(
        &self,
        context: &SecurityContext,
        workspace: Option<WorkspaceId>,
    ) -> Result<Option<Arc<WorkspaceController>>> {
        self.session_controller.update_workspace(context, workspace).await?;
        let held = held_workspace(&self.controllers, self.id, workspace).await?;
        self.workspace.store(held.clone());
        Ok(held)
    }
}

#[cfg(test)]
mod tests {

    use crate::session::testing::*;
    use crate::view::Viewer;
    use caelumora_domain::security::{Grant, Subject};
    use caelumora_domain::session::testing::make_session;
    use caelumora_domain::session::Session;
    use caelumora_domain::store::StoreError;
    use caelumora_domain::user::testing::make_user;
    use caelumora_security::SecurityContext;
    use caelumora_store::testing::record_users;
    use std::sync::Arc;

    #[tokio::test]
    async fn only_the_owner_changes_a_sessions_privacy_and_everyone_in_it_sees_it() {
        use caelumora_domain::actor::ActorId;
        use caelumora_domain::privacy::PrivacyConfig;
        use caelumora_domain::session::SessionUpdateEvent;
        use caelumora_domain::user::UserId;

        let (_, manager) = owned_kernel(Default::default());
        let kernel = Arc::clone(manager.kernel());
        let (alice, bob) = (make_user("alice"), make_user("bob"));
        let as_user = |id: UserId| {
            SecurityContext::new(ActorId::User(id), Default::default(), Default::default())
        };
        let mut session = make_session("hers", alice.id);
        session.security_attributes = session
            .security_attributes
            .clone()
            .granting(Subject::Actor(ActorId::User(bob.id)), Grant::Read);
        session.participants =
            record_users(manager.kernel().entity_stores(), &[&alice, &bob]).await;
        let session = create(&manager, &as_user(alice.id), session).await.unwrap();
        let mut events = kernel.event_bus().subscribe_all();

        let refused = session
            .update_privacy(&as_user(bob.id), PrivacyConfig::SharedAudience)
            .await
            .expect_err("a participant may not");
        assert!(
            matches!(refused.downcast_ref::<StoreError>(), Some(StoreError::PermissionDenied(_))),
            "{refused}"
        );
        assert_eq!(session.privacy().policy, PrivacyConfig::Private, "unchanged");

        session
            .update_privacy(&as_user(alice.id), PrivacyConfig::SharedAudience)
            .await
            .expect("the owner may");
        assert_eq!(session.privacy().policy, PrivacyConfig::SharedAudience);
        let stored = kernel
            .entity_stores()
            .sessions
            .get(&SecurityContext::system(), session.id())
            .await
            .unwrap();
        assert_eq!(stored.privacy, PrivacyConfig::SharedAudience, "stored");
        assert!(events.drain().await.iter().any(|e| matches!(e.about::<Session>(), Some((id, change)) if id == session.id() && matches!(change.update::<SessionUpdateEvent>(), Some(SessionUpdateEvent::PrivacyChanged(PrivacyConfig::SharedAudience))))),
                "announced");
        let for_bob =
            manager.views(Viewer::new(as_user(bob.id))).session(session.id()).await.unwrap();
        assert_eq!(
            for_bob.privacy().policy,
            PrivacyConfig::SharedAudience,
            "shown to everyone who takes part"
        );
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }
}
