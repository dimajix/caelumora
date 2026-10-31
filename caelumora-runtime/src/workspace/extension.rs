//! The workspace as an agent's work uses it (issue 516): an extension of the core's, cut into a
//! reader - what of the workspace of its session the work may use in its channel it reads, and its
//! files to read - and a writer - its files to change and its shell. The core binds both to the
//! session of every piece of work as the work reads it, and the workspace's controller asks in the
//! work's context at every use of a terminal or a shell (`WorkspaceController::terminal_in`,
//! `shell_in`).

use super::{WorkspaceError, WorkspaceResult};
use crate::controller::WorkspaceController;
use crate::session::SessionLoop;
use crate::terminal::{Shell, Terminal};
use crate::view::{ChannelView, SessionView, WorkspaceView};
use async_trait::async_trait;
use caelumora_model::channel::Channel;
use caelumora_model::confidentiality::Classification;
use caelumora_model::security::{Grant, SecurityContext};
use caelumora_model::session::{ChannelId, Session};
use caelumora_model::value::text::Description;
use caelumora_model::workspace::{Workspace, WorkspaceId};
use std::sync::Arc;
use tracing::warn;

/// The workspace of its session an agent's work may use in its channel, as the work reads it.
#[async_trait]
pub trait WorkspaceReader: Send + Sync {
    /// The workspace used.
    fn id(&self) -> WorkspaceId;

    /// Its name, unique where it lies.
    fn name(&self) -> &str;

    /// What it holds, in a line; `None` if nobody said.
    fn description(&self) -> Option<&Description>;

    /// How its files are classified.
    fn classification(&self) -> &Classification;

    /// Whether commands can be run in it (issue 344).
    fn has_shell(&self) -> bool;

    /// Whether the work may change its files.
    async fn may_write(&self) -> bool;

    /// Its terminal, to read its files - asked as reading them is asked. A handle: it is the
    /// storage's terminal, which can do more than reading, and the work is trusted to read alone
    /// (issue 516).
    ///
    /// # Errors
    /// As `WorkspaceController::terminal_in`.
    async fn terminal(&self) -> WorkspaceResult<Arc<dyn Terminal>>;
}

/// The workspace of its session an agent's work acts on in its channel: what a tool of a turn
/// reads and changes its files by and runs its commands by, each use asked in the work's context
/// as it is made. What only reads goes through the reader.
#[async_trait]
pub trait WorkspaceWriter: Send + Sync {
    /// The workspace and its terminal, for what `need` asks - reading its files, or changing them
    /// too -, a tool's one way to them. A handle, as the reader's terminal is.
    ///
    /// # Errors
    /// As `WorkspaceController::terminal_in`.
    async fn terminal(&self, need: Grant) -> WorkspaceResult<(Workspace, Arc<dyn Terminal>)>;

    /// The workspace and its shell as the work's channel sees it, the shell `None` where its
    /// storage runs no commands.
    ///
    /// # Errors
    /// As `WorkspaceController::shell_in`.
    async fn shell(&self) -> WorkspaceResult<(Workspace, Option<Arc<dyn Shell>>)>;

    /// The largest file a workspace takes, which what the terminal writes is bound by.
    fn max_file_bytes(&self) -> u64;
}

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

    async fn terminal(&self) -> WorkspaceResult<Arc<dyn Terminal>> {
        let session = self.channel.session().as_session();
        let context = self.channel.views().context();
        let channel = self.channel.document();
        let (_, terminal) =
            self.controller.terminal_in(context, session, channel, Grant::Read).await?;
        Ok(terminal)
    }
}

/// What a piece of work acts on the workspace of its channel by, asking nothing until it acts: the
/// writer.
pub(crate) struct WorkspaceUse {
    context: SecurityContext,
    session: Arc<SessionLoop>,
    channel: ChannelId,
    max_file_bytes: u64,
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
            max_file_bytes: views.workspace_manager().max_file_bytes(),
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
    async fn terminal(&self, need: Grant) -> WorkspaceResult<(Workspace, Arc<dyn Terminal>)> {
        let (session, channel) = self.documents()?;
        let controller = self.session.workspace()?;
        controller.terminal_in(&self.context, &session, &channel, need).await
    }

    async fn shell(&self) -> WorkspaceResult<(Workspace, Option<Arc<dyn Shell>>)> {
        let (session, channel) = self.documents()?;
        let controller = self.session.workspace()?;
        controller.shell_in(&self.context, &session, &channel).await
    }

    fn max_file_bytes(&self) -> u64 {
        self.max_file_bytes
    }
}
