//! Changing a running session: its name, privacy, approval rules, description and workspace.

use caelumora_model::approval::ApprovalRule;
use anyhow::Result;
use caelumora_model::security::{ActionType, Enforcement, Grant, SecurityContext, Subject};
use caelumora_model::privacy::PrivacyConfig;
use caelumora_model::store::{StoreError, Stored};
use tracing::info;
use caelumora_model::group::GroupId;
use caelumora_model::workspace::WorkspaceId;
use super::{held_workspace, SessionLoop, require_attachable};
use caelumora_model::session::Session;
use crate::controller::WorkspaceController;
use std::sync::Arc;

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

    /// Changes whom the session lets read what was said in it (issue 134)
    /// on behalf of `context`. What was remembered before keeps the privacy
    /// it was said under: a wider policy lets go only what is said from now
    /// on, a narrower one takes back what it let go.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// session, or whatever else storing it reports.
    pub async fn update_privacy(&self, context: &SecurityContext, privacy: PrivacyConfig) -> Result<()> {
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
    pub async fn update_approval_rules(&self, context: &SecurityContext, rules: Vec<ApprovalRule>) -> Result<()> {
        let count = rules.len();
        self.session_controller.update_approval_rules(context, rules).await?;
        info!(session = %self.id, rules = count, by = %context.caller_id, "Session approval rules changed");
        Ok(())
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
    pub async fn update_description(&self, context: &SecurityContext, summary: Option<&str>, briefing: Option<&str>) -> Result<()> {
        self.session_controller.update_description(context, summary, briefing).await?;
        info!(session = %self.id, by = %context.caller_id, "Session description changed");
        Ok(())
    }

    /// Attaches `workspace` to the session, or with `None` takes the one
    /// attached away (issue 253), on behalf of `context`. Whoever may change
    /// the session asks first (`UpdateSession`); to attach one, it must be
    /// theirs to share (`GetWorkspace`, `UpdateWorkspace` - its owner,
    /// issue 251, decision 12) and lie where this session may use it - not
    /// in another session's scope. Taking it away shares nothing and asks
    /// no more. Its channels use it as their classifications and switches
    /// let them, from their next use - and as its access list lets each
    /// member (issue 23): attaching shares nothing by itself, unless `share`
    /// says how far the session's members may use it, which adds an entry
    /// for them. Taking it away leaves the list as it is: it is the
    /// workspace's, and its owner's to change.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// session or share the workspace, `WorkspaceError::NotFound` if there
    /// is no workspace they may read or are cleared for,
    /// `SessionError::WorkspaceOutOfScope`, or
    /// whatever else storing reports.
    pub async fn set_workspace(&self, context: &SecurityContext, workspace: Option<WorkspaceId>, share: Option<Grant>) -> Result<()> {
        let session = self.session();
        self.access_policy.require(context, ActionType::update(Session::KIND, &session.security_attributes), StoreError::PermissionDenied).await?;
        if let Some(id) = workspace {
            require_attachable(self.runtime_manager.workspace_manager(), self.access_policy.as_ref(), context, id, self.id).await?;
        }
        let held = self.attach_workspace(context, workspace).await?;
        if let (Some(controller), Some(grant)) = (held, share) {
            controller.share(context, Subject::Group(GroupId::of_session(self.id)), grant).await?;
        }
        info!(session = %self.id, workspace = ?workspace, by = %context.caller_id, "Session's workspace changed");
        Ok(())
    }

    /// Attaches `workspace` to the session on behalf of `context`, or with
    /// `None` takes the one attached away, and holds it live (issue 344):
    /// stored as changing the session, without asking whether it may be
    /// attached - its caller asked that (`set_workspace`), or found or made
    /// it for the session as it made the session (`SessionManager`).
    /// Returns the workspace as it is held.
    ///
    /// # Errors
    /// Whatever storing the session or reading the workspace reports.
    pub(crate) async fn attach_workspace(&self, context: &SecurityContext, workspace: Option<WorkspaceId>) -> Result<Option<Arc<WorkspaceController>>> {
        self.session_controller.update_workspace(context, workspace).await?;
        let held = held_workspace(&self.controllers, self.id, workspace).await?;
        self.workspace.store(held.clone());
        Ok(held)
    }
}

#[cfg(test)]
mod tests {

    
    use caelumora_model::security::SecurityContext;
    use crate::view::Viewer;
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
        let runtime = Arc::clone(runtime_of(&manager));
        let (alice, bob) = (user("alice"), user("bob"));
        let as_user = |id: UserId| SecurityContext::new(ActorId::User(id), Default::default(), Default::default());
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.security_attributes = session.security_attributes.clone().granting(caelumora_model::security::Subject::Actor(ActorId::User(bob.id)), caelumora_model::security::Grant::Read);
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let session = create(&manager, &as_user(alice.id), session).await.unwrap();
        let mut events = runtime.event_bus().subscribe_all();

        let refused = session.update_privacy(&as_user(bob.id), PrivacyConfig::SharedAudience).await.expect_err("a participant may not");
        assert!(matches!(refused.downcast_ref::<StoreError>(), Some(StoreError::PermissionDenied(_))), "{refused}");
        assert_eq!(session.privacy().policy, PrivacyConfig::Private, "unchanged");

        session.update_privacy(&as_user(alice.id), PrivacyConfig::SharedAudience).await.expect("the owner may");
        assert_eq!(session.privacy().policy, PrivacyConfig::SharedAudience);
        let stored = runtime.entity_stores().sessions.get(&SecurityContext::system(), session.id()).await.unwrap();
        assert_eq!(stored.privacy, PrivacyConfig::SharedAudience, "stored");
        assert!(drain(&mut events).iter().any(|e| matches!(e.about::<Session>(), Some((id, change)) if id == session.id() && matches!(change.update::<SessionUpdateEvent>(), Some(SessionUpdateEvent::PrivacyChanged(PrivacyConfig::SharedAudience))))),
                "announced");
        let for_bob = manager.views(Viewer::new(as_user(bob.id))).session(session.id()).await.unwrap();
        assert_eq!(for_bob.privacy().policy, PrivacyConfig::SharedAudience, "shown to everyone who takes part");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }
}
