//! Changing a running session: its name, language, time zone, privacy, approval rules,
//! description, workspace and project.

use super::{held_workspace, load_own, make_own, require_attachable, SessionError, SessionLoop};
use crate::controller::WorkspaceController;
use crate::project;
use crate::scoped::purge_for_good;
use anyhow::{Error, Result};
use caelumora_model::approval::ApprovalRule;
use caelumora_model::event::{Announcing, GlobalEvent};
use caelumora_model::group::GroupId;
use caelumora_model::privacy::PrivacyConfig;
use caelumora_model::project::{Project, ProjectId};
use caelumora_model::scope::Scope;
use caelumora_model::security::{ActionType, Enforcement, Grant, SecurityContext, Subject};
use caelumora_model::session::Session;
use caelumora_model::store::{StoreError, StoreResultExt, Stored};
use caelumora_model::workspace::WorkspaceId;
use std::sync::Arc;
use tracing::{info, warn};

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
        self.access_policy
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

    /// Puts the session into the shared project `project`, or with `None` takes it out into its
    /// own (issue 467) - the one in its scope, or a new one if that was lifted -, on behalf of
    /// `context`: whoever may change the session (`UpdateSession`) and reach the project
    /// (`GetProject`). The project it now lies in is shared with its participants for reading, in
    /// the runtime's name - the session's act was asked -, so that every participant publishes
    /// into it; the entry of the shared one it leaves is taken away. Nothing moves: what went to
    /// the project before stays there, only what comes now goes to the other.
    ///
    /// The project it goes to is shared, or its new own made, before the session is stored as
    /// lying there, so that it never lies where its participants read nothing; if storing the
    /// session fails, that is taken back. One change at a time (`moving`).
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if `context` may not change the session,
    /// `ProjectError::NotFound` if there is no project they may read,
    /// `SessionError::ProjectOutOfScope` for another session's own, `SessionError::OwnerNotAUser`
    /// if a new own is needed and the session is no user's, or whatever else storing reports.
    pub async fn update_project(
        &self,
        context: &SecurityContext,
        project: Option<ProjectId>,
    ) -> Result<()> {
        let _moving = self.moving.lock().await;
        let session = self.session();
        self.access_policy
            .require(
                context,
                ActionType::update(Session::KIND, &session.security_attributes),
                StoreError::PermissionDenied,
            )
            .await?;
        let (target, made) = match project {
            Some(id) => (self.reached_project(context, id).await?, false),
            None => match load_own(&self.stores, self.id).await? {
                Some(own) => (own, false),
                None => (
                    session
                        .new_own_project()
                        .ok_or(SessionError::OwnerNotAUser(session.security_attributes.owner_id))?,
                    true,
                ),
            },
        };
        if target.id == session.project {
            return Ok(());
        }
        let system = SecurityContext::system();
        let left = self
            .stores
            .projects
            .get(&system, session.project)
            .await
            .optional()?
            .filter(|left| !left.is_own());
        let participants = Subject::Group(GroupId::of_session(self.id));
        if made {
            make_own(&self.stores, &target).await?;
        } else if !target.is_own() {
            self.controllers
                .project_controller(target.id)
                .await?
                .share(&system, participants, Grant::Read)
                .await?;
        }
        if let Err(error) = self.session_controller.update_project(context, target.id).await {
            self.take_back_entering(&target, made).await;
            return Err(error.into());
        }
        if made {
            self.events.announce(GlobalEvent::created::<Project>(&target.id), &target);
        }
        if let Some(left) = left {
            let unshared = async {
                self.controllers
                    .project_controller(left.id)
                    .await?
                    .unshare(&system, participants)
                    .await
            };
            if let Err(e) = unshared.await {
                warn!(session = %self.id, project = %left.id, "The project a session left is still shared with its participants: {e}");
            }
        }
        info!(session = %self.id, project = %target.id, by = %context.caller_id, "Session's project changed");
        Ok(())
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

    /// Takes back what `update_project` did for `target` before storing the session failed: its
    /// new own purged if it was `made`, the entry of a shared one taken away; in the runtime's
    /// name. What cannot be is logged.
    async fn take_back_entering(&self, target: &Project, made: bool) {
        let system = SecurityContext::system();
        let taken_back = async {
            if made {
                purge_for_good(self.stores.groups.as_ref(), GroupId::of_project(target.id)).await?;
                purge_for_good(self.stores.projects.as_ref(), target.id).await?;
            } else if !target.is_own() {
                self.controllers
                    .project_controller(target.id)
                    .await?
                    .unshare(&system, Subject::Group(GroupId::of_session(self.id)))
                    .await?;
            }
            Ok::<_, Error>(())
        }
        .await;
        if let Err(e) = taken_back {
            warn!(session = %self.id, project = %target.id, "What was done for a project a session did not go to stays: {e}");
        }
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

    use crate::view::Viewer;
    use caelumora_model::security::{Grant, SecurityContext, Subject};
    use caelumora_model::store::StoreError;
    use std::sync::Arc;

    use crate::session::testing::*;
    use caelumora_model::session::Session;

    #[tokio::test]
    async fn only_the_owner_changes_a_sessions_privacy_and_everyone_in_it_sees_it() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::privacy::PrivacyConfig;
        use caelumora_model::session::SessionUpdateEvent;
        use caelumora_model::user::UserId;

        let (_, manager) = owned_runtime(Default::default());
        let runtime = Arc::clone(manager.runtime());
        let (alice, bob) = (user("alice"), user("bob"));
        let as_user = |id: UserId| {
            SecurityContext::new(ActorId::User(id), Default::default(), Default::default())
        };
        let mut session = make_session("hers", alice.id);
        session.security_attributes = session
            .security_attributes
            .clone()
            .granting(Subject::Actor(ActorId::User(bob.id)), Grant::Read);
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let session = create(&manager, &as_user(alice.id), session).await.unwrap();
        let mut events = runtime.event_bus().subscribe_all();

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
        let stored = runtime
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
