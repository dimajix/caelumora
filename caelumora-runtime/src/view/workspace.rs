//! A workspace as a viewer reached it (issue 253): directly, as its access
//! list lets them wherever they act (issue 23), or - an agent - in a
//! channel in which the session's workspace may be used (issue 251,
//! decision 10). It is read alone: what is done with its files - reading
//! them through its terminal, changing them, running commands, publishing
//! one - its controller does, asking afresh (`WorkspaceController`, issues
//! 296, 344).

use super::Views;
use caelumora_model::confidentiality::Classification;
use caelumora_model::security::AccessQuestions;
use caelumora_model::snapshot::WorkspaceSnapshot;
use caelumora_model::value::text::Description;
use caelumora_model::workspace::{Workspace, WorkspaceId};

#[derive(Clone)]
pub struct WorkspaceView {
    views: Views,
    workspace: Workspace,
}

impl WorkspaceView {
    pub(super) fn of(views: Views, workspace: Workspace) -> Self {
        Self { views, workspace }
    }

    pub fn id(&self) -> WorkspaceId {
        self.workspace.id
    }

    pub fn name(&self) -> &str {
        self.workspace.name.as_str()
    }

    /// The storage its files lie in, by its configured name.
    pub fn storage(&self) -> &str {
        &self.workspace.storage
    }

    /// What it holds, as its owner describes it.
    pub fn description(&self) -> Option<&Description> {
        self.workspace.description.as_ref()
    }

    pub fn classification(&self) -> &Classification {
        &self.workspace.classification
    }

    /// Whether its storage gives it a shell: what agents can do with it;
    /// `false` for a storage no longer configured.
    pub fn has_shell(&self) -> bool {
        self.views
            .workspace_manager()
            .storages()
            .any(|(name, has_shell)| name == self.workspace.storage && has_shell)
    }

    /// The largest file that may be put into it.
    pub fn max_file_bytes(&self) -> u64 {
        self.views.workspace_manager().max_file_bytes()
    }

    /// Whether the viewer may change it, share it and attach it to a
    /// session (`UpdateWorkspace`): its owner.
    pub async fn may_change(&self) -> bool {
        self.views
            .access_policy()
            .may_change_workspace(self.views.context(), &self.workspace.security_attributes)
            .await
    }

    /// Whether the viewer may change its files (`WriteWorkspace`, issue
    /// 23) - in a channel, besides, only as far as the channel lets them use
    /// it at all, which reaching it there asked.
    pub async fn may_write(&self) -> bool {
        self.views
            .access_policy()
            .may_write_workspace(self.views.context(), &self.workspace.security_attributes)
            .await
    }

    pub async fn snapshot(&self) -> WorkspaceSnapshot {
        let can_change = self.may_change().await;
        let access = match can_change {
            true => self.views.access_snapshots(&self.workspace.security_attributes.acl).await,
            false => Vec::new(),
        };
        WorkspaceSnapshot {
            id: self.workspace.id,
            name: self.workspace.name.to_string(),
            scope: self.views.scope_snapshot(&self.workspace.name.scope).await,
            storage: self.workspace.storage.clone(),
            description: self.workspace.description.clone(),
            classification: self.workspace.classification.clone(),
            has_shell: self.has_shell(),
            can_change,
            can_write: self.may_write().await,
            access,
        }
    }
}
