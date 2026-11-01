//! How the kernel serves the workspace an agent's work uses (issue 516): an extension of the
//! kernel's, its reader and its writer (`caelumora_api::workspace`) bound to the session of every
//! piece of work as the work reads it, and the workspace's controller asking in the work's context
//! at every use of a terminal or a shell (`WorkspaceController::terminal_in`, `shell_in`).

use crate::controller::WorkspaceController;
use crate::session::SessionLoop;
use crate::view::{ChannelView, SessionView, WorkspaceView};
use async_trait::async_trait;
use caelumora_api::terminal::{Shell, Terminal, TerminalReader};
use caelumora_api::workspace::{WorkspaceError, WorkspaceReader, WorkspaceResult, WorkspaceWriter};
use caelumora_model::channel::Channel;
use caelumora_model::confidentiality::Classification;
use caelumora_model::session::{ChannelId, Session};
use caelumora_model::value::text::Description;
use caelumora_model::workspace::{Workspace, WorkspaceId};
use caelumora_security::SecurityContext;
use std::sync::Arc;
use tracing::warn;

/// The workspace a piece of work may use in its channel, found as it is bound: the reader.
pub(crate) struct UsableWorkspace {
    channel: ChannelView,
    workspace: WorkspaceView,
    controller: Arc<WorkspaceController>,
}

impl UsableWorkspace {
    /// The workspace the work that reads `session` may use in its channel
    /// (`ChannelView::workspace`); `None` where there is none it may use, where it reads from no
    /// channel the running session holds, or where the work may not read the channel - and where
    /// reading the workspace failed, which is logged.
    pub(crate) async fn of_work(session: &SessionView) -> Option<Self> {
        let at = session.views().viewer().place()?.channel;
        let controller = session.running()?.workspace().ok()?;
        let channel = session.channel(at).await.ok()?;
        let workspace = match channel.workspace().await {
            Ok(workspace) => workspace?,
            Err(e) => {
                warn!(channel = %at, "The workspace of a channel could not be read: {e}");
                return None;
            }
        };
        Some(Self { channel, workspace, controller })
    }
}

#[async_trait]
impl WorkspaceReader for UsableWorkspace {
    fn id(&self) -> WorkspaceId {
        self.workspace.id()
    }

    fn name(&self) -> &str {
        self.workspace.name()
    }

    fn description(&self) -> Option<&Description> {
        self.workspace.description()
    }

    fn classification(&self) -> &Classification {
        self.workspace.classification()
    }

    fn has_shell(&self) -> bool {
        self.workspace.has_shell()
    }

    async fn may_write(&self) -> bool {
        self.workspace.may_write().await
    }

    async fn terminal(&self) -> WorkspaceResult<(Workspace, Arc<dyn TerminalReader>)> {
        let session = self.channel.session().as_session();
        let context = self.channel.views().context();
        self.controller.reader_in(context, session, self.channel.document()).await
    }
}

/// What a piece of work acts on the workspace of its channel by, asking nothing until it acts: the
/// writer.
pub(crate) struct WorkspaceUse {
    context: SecurityContext,
    session: Arc<SessionLoop>,
    channel: ChannelId,
}

impl WorkspaceUse {
    /// The use of the workspace of its channel by the work that reads `session`; `None` where it
    /// reads from no channel, or its session does not run.
    pub(crate) fn of_work(session: &SessionView) -> Option<Self> {
        let views = session.views();
        Some(Self {
            context: views.context().clone(),
            session: Arc::clone(session.running()?),
            channel: views.viewer().place()?.channel,
        })
    }

    /// The session's document and the channel's, as they are now.
    fn documents(&self) -> WorkspaceResult<(Arc<Session>, Arc<Channel>)> {
        let channel =
            self.session.channel(self.channel).ok_or(WorkspaceError::NotUsableInChannel)?;
        Ok((self.session.session(), channel.channel()))
    }
}

#[async_trait]
impl WorkspaceWriter for WorkspaceUse {
    async fn terminal(&self) -> WorkspaceResult<(Workspace, Arc<dyn Terminal>)> {
        let (session, channel) = self.documents()?;
        let controller = self.session.workspace()?;
        controller.terminal_in(&self.context, &session, &channel).await
    }

    async fn shell(&self) -> WorkspaceResult<(Workspace, Option<Arc<dyn Shell>>)> {
        let (session, channel) = self.documents()?;
        let controller = self.session.workspace()?;
        controller.shell_in(&self.context, &session, &channel).await
    }
}
