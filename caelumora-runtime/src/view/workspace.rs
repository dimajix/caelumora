//! A workspace as a viewer reaches it (issue 253): its owner in the list
//! of their workspaces, or a member of a channel in which the session's
//! workspace may be used (issue 251, decision 10). Nothing of its place is
//! reachable past it but through `terminal`, which asks again.

use super::{ViewError, Views};
use crate::terminal::Terminal;
use crate::workspace::{WorkspaceError, WorkspaceResult};
use caelumora_model::confidentiality::Classification;
use caelumora_model::security::AccessQuestions;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::snapshot::WorkspaceSnapshot;
use caelumora_model::value::text::Description;
use caelumora_model::workspace::{Workspace, WorkspaceId};
use std::sync::Arc;
use tracing::warn;

/// How the workspace was reached: what `terminal` asks again, of the
/// session and the channel as they are then.
#[derive(Clone, Copy)]
enum Way {
    /// By its owner, directly.
    Owned,
    /// In a channel of the session it is attached to.
    InChannel { session: SessionId, channel: ChannelId },
}

#[derive(Clone)]
pub struct WorkspaceView {
    views: Views,
    workspace: Workspace,
    way: Way,
}

impl WorkspaceView {
    pub(super) fn owned(views: Views, workspace: Workspace) -> Self {
        Self { views, workspace, way: Way::Owned }
    }

    pub(super) fn in_channel(views: Views, workspace: Workspace, session: SessionId, channel: ChannelId) -> Self {
        Self { views, workspace, way: Way::InChannel { session, channel } }
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

    /// Whether its storage gives it a shell: what agents can do with it.
    pub fn has_shell(&self) -> bool {
        self.views.workspace_manager().has_shell(&self.workspace)
    }

    /// The largest file that may be put into it.
    pub fn max_file_bytes(&self) -> u64 {
        self.views.workspace_manager().max_file_bytes()
    }

    /// Whether the viewer may change it and attach it to a session
    /// (`UpdateWorkspace`): its owner.
    pub async fn may_change(&self) -> bool {
        self.views.security().may_change_workspace(self.views.context(), &self.workspace.security_attributes).await
    }

    /// The terminal of its files, if the viewer may still use it the way
    /// they reached it - asked again, of the workspace, the session and the
    /// channel as they are now, since a view decides nothing.
    ///
    /// # Errors
    /// `NotFound` if they may no longer, `NotCleared`, `UnknownStorage`, or
    /// a `TerminalError` of reaching its place.
    pub async fn terminal(&self) -> WorkspaceResult<Arc<dyn Terminal>> {
        let gone = || WorkspaceError::NotFound(self.workspace.id);
        // Out of reach is one case with gone; a failure to reach is logged.
        let unreached = |error: ViewError| {
            if let ViewError::Failed(cause) = &error {
                warn!(workspace = %self.workspace.id, "Reaching a workspace through its channel failed: {cause:#}");
            }
            gone()
        };
        let workspace = match self.way {
            Way::Owned => self.views.workspace_manager().owned(self.views.context(), self.workspace.id).await?,
            Way::InChannel { session, channel } => {
                let session = self.views.session(session).await.map_err(unreached)?;
                let channel = session.channel(channel).await.map_err(unreached)?;
                channel.workspace().await.map_err(unreached)?
                    .filter(|usable| usable.id() == self.workspace.id)
                    .ok_or_else(gone)?
                    .workspace
            }
        };
        self.views.workspace_manager().terminal(&workspace).await
    }

    pub async fn snapshot(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            id: self.workspace.id,
            name: self.workspace.name.to_string(),
            scope: self.workspace.name.scope.clone(),
            storage: self.workspace.storage.clone(),
            description: self.workspace.description.clone(),
            classification: self.workspace.classification.clone(),
            has_shell: self.has_shell(),
            can_change: self.may_change().await,
        }
    }
}
