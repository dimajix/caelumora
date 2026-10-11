//! Changing a running session: its name, privacy, approval rules, description and workspace.

use caelumora_model::approval::ApprovalRule;
use anyhow::Result;
use caelumora_model::security::{ActionType, Enforcement, Grant, SecurityContext, Subject};
use caelumora_model::privacy::PrivacyConfig;
use caelumora_model::store::StoreError;
use tracing::info;
use caelumora_model::group::GroupId;
use caelumora_model::workspace::WorkspaceId;
use super::{SessionLoop, require_attachable};

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
    /// session or share the workspace, `StoreError::NotFound` if there is
    /// no workspace they may read, `SessionError::WorkspaceOutOfScope`, or
    /// whatever else storing reports.
    pub async fn set_workspace(&self, context: &SecurityContext, workspace: Option<WorkspaceId>, share: Option<Grant>) -> Result<()> {
        let session = self.session();
        self.security.require(context, ActionType::UpdateSession(&session.security_attributes), || StoreError::PermissionDenied).await?;
        if let Some(id) = workspace {
            require_attachable(self.security.as_ref(), self.entities.as_ref(), context, id, self.id).await?;
        }
        self.session_controller.update_workspace(context, workspace).await?;
        if let (Some(id), Some(grant)) = (workspace, share) {
            self.runtime_manager.workspace_manager().share(context, id, Subject::Group(GroupId::of_session(self.id)), grant).await?;
        }
        info!(session = %self.id, workspace = ?workspace, by = %context.caller_id, "Session's workspace changed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use caelumora_model::value::name::Name;
    use crate::runtime::RuntimeManager;
    use caelumora_model::event::GlobalEvent;
    use caelumora_model::security::{SecurityAttributes, SecurityContext};
    use crate::view::Viewer;
    use caelumora_model::store::StoreError;
    use std::sync::Arc;
    use caelumora_model::secret::SecretManager;
    use caelumora_model::store::memory::MemoryStore;
    use crate::session::SessionManager;
    use crate::session::testing::*;

    #[tokio::test]
    async fn only_the_owner_changes_a_sessions_privacy_and_everyone_in_it_sees_it() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::privacy::PrivacyConfig;
        use caelumora_model::session::SessionUpdateEvent;
        use caelumora_model::user::{User, UserId};

        let (policy, confidentiality, store) = crate::session::testing::owned_store(Default::default());
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, Arc::clone(&confidentiality), &caelumora_model::config::AppConfig::default(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let user = |name: &str| User { id: UserId::new(), username: Name::new(name).unwrap(), email: String::new(), roles: Vec::new(), deleted: None };
        let (alice, bob) = (user("alice"), user("bob"));
        let as_user = |id: UserId| SecurityContext::new(ActorId::User(id), Default::default(), Default::default());
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes::owned_by(ActorId::User(alice.id)).granting(caelumora_model::security::Subject::Actor(ActorId::User(bob.id)), caelumora_model::security::Grant::Read);
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let session = create(&manager, &as_user(alice.id), session).await.unwrap();
        let mut events = runtime.event_bus().subscribe_all();

        let refused = session.update_privacy(&as_user(bob.id), PrivacyConfig::SharedAudience).await.expect_err("a participant may not");
        assert!(matches!(refused.downcast_ref::<StoreError>(), Some(StoreError::PermissionDenied)), "{refused}");
        assert_eq!(session.privacy().policy, PrivacyConfig::Private, "unchanged");

        session.update_privacy(&as_user(alice.id), PrivacyConfig::SharedAudience).await.expect("the owner may");
        assert_eq!(session.privacy().policy, PrivacyConfig::SharedAudience);
        let stored = runtime.entity_store().get_session(&SecurityContext::system(), session.id()).await.unwrap();
        assert_eq!(stored.privacy, PrivacyConfig::SharedAudience, "stored");
        assert!(drain(&mut events).iter().any(|e| matches!(e, GlobalEvent::SessionUpdated(id, SessionUpdateEvent::PrivacyChanged(PrivacyConfig::SharedAudience)) if *id == session.id())),
                "announced");
        let for_bob = manager.views(Viewer::new(as_user(bob.id))).session(session.id()).await.unwrap();
        assert_eq!(for_bob.privacy().policy, PrivacyConfig::SharedAudience, "shown to everyone who takes part");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }
}
