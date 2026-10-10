//! A workspace as a viewer reaches it (issue 253): directly, as its access
//! list lets them wherever they act (issue 23), or - an agent - in a
//! channel in which the session's workspace may be used (issue 251,
//! decision 10). Nothing of its place is reachable past it but through
//! `terminal`, which asks again.

use super::{ViewError, Views};
use crate::artifact::{ArtifactError, ArtifactResult};
use crate::terminal::{CommandState, Terminal, TerminalError, WorkspacePath};
use crate::workspace::{WorkspaceError, WorkspaceResult};
use caelumora_model::artifact::Artifact;
use caelumora_model::confidentiality::Classification;
use caelumora_model::security::{AccessQuestions, Grant};
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
    /// Directly, by its access list (issue 23).
    Direct,
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
    pub(super) fn direct(views: Views, workspace: Workspace) -> Self {
        Self { views, workspace, way: Way::Direct }
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

    /// Whether the viewer may change it, share it and attach it to a
    /// session (`UpdateWorkspace`): its owner.
    pub async fn may_change(&self) -> bool {
        self.views.security().may_change_workspace(self.views.context(), &self.workspace.security_attributes).await
    }

    /// Whether the viewer may change its files (`WriteWorkspace`, issue
    /// 23) - in a channel, besides, only as far as the channel lets them use
    /// it at all.
    pub async fn may_write(&self) -> bool {
        self.views.security().may_write_workspace(self.views.context(), &self.workspace.security_attributes).await
    }

    /// The terminal of its files for what `need` asks - reading them, or
    /// changing them too (issue 23) -, if the viewer may still use it the
    /// way they reached it: asked again, of the workspace, the session and
    /// the channel as they are now, since a view decides nothing.
    ///
    /// # Errors
    /// `NotFound` if they may no longer, `ReadOnly` if they may not change
    /// its files where `need` asks to, `NotCleared`, `UnknownStorage`, or a
    /// `TerminalError` of reaching its place.
    pub async fn terminal(&self, need: Grant) -> WorkspaceResult<Arc<dyn Terminal>> {
        let gone = || WorkspaceError::NotFound(self.workspace.id);
        // Out of reach is one case with gone; a failure to reach is logged.
        let unreached = |error: ViewError| {
            if let ViewError::Failed(cause) = &error {
                warn!(workspace = %self.workspace.id, "Reaching a workspace through its channel failed: {cause:#}");
            }
            gone()
        };
        let workspace = match self.way {
            Way::Direct => self.views.workspace_manager().reached(self.views.context(), self.workspace.id, need).await?,
            Way::InChannel { session, channel } => {
                let session = self.views.session(session).await.map_err(unreached)?;
                let channel = session.channel(channel).await.map_err(unreached)?;
                let usable = channel.workspace().await.map_err(unreached)?
                    .filter(|usable| usable.id() == self.workspace.id)
                    .ok_or_else(gone)?;
                if need == Grant::Write && !usable.may_write().await {
                    return Err(WorkspaceError::ReadOnly(self.workspace.id));
                }
                usable.workspace
            }
        };
        self.views.workspace_manager().terminal(&workspace).await
    }

    /// Publishes its file `at` as a version of the artifact `name` (issue
    /// 271), in the viewer's name, in the channel they reached it through:
    /// an artifact is published where it is offered, so only a workspace
    /// reached through a channel publishes. Its terminal is reached as
    /// `terminal` reaches it, asked again. Answers the artifact and the
    /// number of the new version.
    ///
    /// # Errors
    /// `NotInChannel` for a workspace reached directly, `Workspace` if it
    /// may no longer be used, or what `ArtifactManager::publish` reports.
    pub async fn publish_artifact(&self, at: &WorkspacePath, name: &str, description: Option<&str>) -> ArtifactResult<(Artifact, u32)> {
        let Way::InChannel { session, channel } = self.way else { return Err(ArtifactError::NotInChannel) };
        let terminal = self.terminal(Grant::Read).await?;
        let content = terminal.read(at).await?;
        self.views.artifact_manager().publish(self.views.context(), content, &self.workspace.classification, name, description, session, channel).await
    }

    /// Ends the command `id` of its shell, started in the channel it was
    /// reached through, with everything the command started (issue 294), in
    /// the viewer's name: a command is seen only in its channel, and who may
    /// change the files there - who could have started it - may end it. Its
    /// terminal is reached as `terminal` reaches it for writing, asked
    /// again. Answers what it printed since `offset`, and how it stands.
    ///
    /// # Errors
    /// `NotInChannel` for a workspace reached directly, what `terminal`
    /// reports, or `Terminal(UnknownCommand)` if no command `id` is known
    /// in the channel - as where its storage gives it no shell.
    pub async fn kill_command(&self, id: &str, offset: u64) -> WorkspaceResult<CommandState> {
        let Way::InChannel { channel, .. } = self.way else { return Err(WorkspaceError::NotInChannel) };
        let terminal = self.terminal(Grant::Write).await?;
        let shell = terminal.shell(&channel.to_string()).ok_or_else(|| TerminalError::UnknownCommand(id.to_string()))?;
        Ok(shell.kill(id, offset).await?)
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
            scope: self.workspace.name.scope.clone(),
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
