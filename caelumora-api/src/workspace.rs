//! The workspace as an agent's work uses it (issue 516), cut into a reader - what of the workspace
//! of its session the work may use in its channel it reads, and its files to read
//! (`WorkspaceReader`) - and a writer - its files to change and its shell (`WorkspaceWriter`) -,
//! and what using a workspace fails with (`WorkspaceError`). The kernel binds both to every piece
//! of work and asks in the work's context at every use (`caelumora_kernel::workspace::extension`).

use crate::error::StorageGone;
use crate::terminal::{Shell, Terminal, TerminalError, TerminalReader};
use async_trait::async_trait;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::security::Refused;
use caelumora_domain::store::StoreError;
use caelumora_domain::value::text::Description;
use caelumora_domain::value::ValueError;
use caelumora_domain::workspace::{Workspace, WorkspaceId};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    /// No such workspace, or not one the caller may use - one case, so
    /// that nothing can be probed.
    #[error("there is no workspace {0}")]
    NotFound(WorkspaceId),
    /// A storage the caller named, for a workspace to be made in, that is not configured.
    #[error("no storage named '{0}' is configured")]
    UnknownStorage(String),
    /// The storage a workspace lies in, since removed from the configuration.
    #[error(transparent)]
    StorageGone(#[from] StorageGone),
    #[error("a workspace belongs to a user")]
    OwnerNotAUser,
    #[error("the classification may not be given")]
    ClassificationRefused,
    #[error("you are not cleared for the classification of the workspace")]
    NotCleared,
    /// What its access list grants the caller is reading, not changing its
    /// files (issue 23).
    #[error("you may read the workspace {id}, not change its files")]
    ReadOnly {
        id: WorkspaceId,
        #[source]
        refused: Refused,
    },
    /// No workspace the caller may use in the channel they act in: none is
    /// attached, the channel does not use it, they may not read the channel
    /// or the workspace, or the two do not flow both ways - one case, so
    /// that nothing can be probed (issue 296).
    #[error("there is no workspace you can use in this channel")]
    NotUsableInChannel,
    /// Its storage may not hold what is so classified (issue 251): a place
    /// with a shell or a remote side is cleared for so much.
    #[error("the storage '{0}' may not hold a workspace so classified")]
    StorageNotCleared(String),
    #[error(transparent)]
    Invalid(#[from] ValueError),
    #[error(transparent)]
    Terminal(#[from] TerminalError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub type WorkspaceResult<T> = Result<T, WorkspaceError>;

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

    /// The workspace as it is now and its files to read - asked as reading them is asked -,
    /// through a terminal that reads alone.
    ///
    /// # Errors
    /// As `WorkspaceController::reader_in`.
    async fn terminal(&self) -> WorkspaceResult<(Workspace, Arc<dyn TerminalReader>)>;
}

/// The workspace of its session an agent's work acts on in its channel: what a tool of a turn
/// changes its files by and runs its commands by, each use asked in the work's context as it is
/// made. Reading goes through the reader, in a tool too.
#[async_trait]
pub trait WorkspaceWriter: Send + Sync {
    /// The workspace and its terminal, to change its files - asked as changing them is asked
    /// (`WriteWorkspace`). A handle that carries the right to change them, and writes no file
    /// larger than the workspaces take.
    ///
    /// # Errors
    /// As `WorkspaceController::terminal_in`.
    async fn terminal(&self) -> WorkspaceResult<(Workspace, Arc<dyn Terminal>)>;

    /// The workspace and its shell as the work's channel sees it, the shell `None` where its
    /// storage runs no commands.
    ///
    /// # Errors
    /// As `WorkspaceController::shell_in`.
    async fn shell(&self) -> WorkspaceResult<(Workspace, Option<Arc<dyn Shell>>)>;
}
