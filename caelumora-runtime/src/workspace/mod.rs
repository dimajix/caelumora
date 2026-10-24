//! Workspaces and their places (issues 251, 253). Their lifecycle - made,
//! deleted, restored and purged, each with its place in its storage - is
//! the `WorkspaceManager`'s (`manager`). What a workspace is - its name,
//! description, classification and access list - its controller changes
//! (`controller::WorkspaceController`, issue 344). Reading the documents is
//! the views' (`view::WorkspaceView`).
//!
//! A workspace is used in one of two ways, each asked by its controller
//! before anything else (issue 251, decisions 10 to 12):
//!
//! - *directly*, wherever the caller acts (`WorkspaceController::terminal`):
//!   `GetWorkspace`, and the caller cleared for its classification (issue
//!   23);
//! - *in a channel* of the session it is attached to
//!   (`WorkspaceController::terminal_in`): by a current participant of the
//!   channel, if the channel uses the workspace and the workspace and the
//!   channel flow both ways - what a command prints reaches the channel,
//!   and what was seen there may go into a command.
//!
//! A workspace lies in a scope as an agent does (issue 251, decision 13):
//! what lies in a session's scope is used within that session only
//! (`Scope::admits`), and is deleted, restored and purged with it
//! (`…_lying_in`, called by `scoped`) - its place removed as it is purged.
//!
//! The commands run in workspaces are ended by the `CommandReaper`
//! (`reaper`) once nobody may run them any longer (issue 254).

mod manager;
pub(crate) mod reaper;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use manager::WorkspaceManager;

use crate::terminal::{TerminalError, TerminalProvider};
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy, ConfidentialityQuestions};
use caelumora_model::security::{ActionType, Enforcement, Refused, SecurityContext, AccessPolicy, WRITE};
use caelumora_model::store::{StoreError, Stored};
use caelumora_model::value::ValueError;
use caelumora_model::workspace::{Workspace, WorkspaceId};

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    /// No such workspace, or not one the caller may use - one case, so
    /// that nothing can be probed.
    #[error("there is no workspace {0}")]
    NotFound(WorkspaceId),
    #[error("no storage named '{0}' is configured")]
    UnknownStorage(String),
    #[error("a workspace belongs to a user")]
    OwnerNotAUser,
    #[error("the classification may not be given")]
    ClassificationRefused,
    #[error("you are not cleared for the classification of the workspace")]
    NotCleared,
    /// What its access list grants the caller is reading, not changing its
    /// files (issue 23).
    #[error("you may read the workspace {id}, not change its files")]
    ReadOnly { id: WorkspaceId, #[source] refused: Refused },
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

/// `error` of the store about workspace `id`: not found is one case with
/// not readable.
pub(crate) fn store_error(id: WorkspaceId) -> impl FnOnce(StoreError) -> WorkspaceError {
    move |error| match error {
        StoreError::NotFound { .. } => WorkspaceError::NotFound(id),
        error => WorkspaceError::Store(error),
    }
}

/// Whether `classification` may be given at all, and whether the storage
/// `storage`, served by `provider`, may hold what is so classified - what
/// is so classified flows into its destination, asked as putting it there
/// (issue 251) - and whoever `ctx` acts for is cleared for it, as they
/// would have to be to read a channel so classified: the flow, the
/// `ConfidentialityPolicy`'s (issue 304).
///
/// # Errors
/// `ClassificationRefused`, `StorageNotCleared` or `NotCleared`.
pub(crate) fn require_classification(policy: &dyn ConfidentialityPolicy, ctx: &SecurityContext, storage: &str, provider: &dyn TerminalProvider, classification: &Classification) -> WorkspaceResult<()> {
    policy.require_classifiable(classification, || WorkspaceError::ClassificationRefused)?;
    policy.require_storable(ctx, classification, provider.destination(), || WorkspaceError::StorageNotCleared(storage.to_string()))?;
    policy.require_cleared_for(ctx, classification, || WorkspaceError::NotCleared)
}

/// Whether whoever `ctx` acts for may change the files of `workspace`
/// (`WriteWorkspace`, issue 23), once they reached it.
///
/// # Errors
/// `ReadOnly` if they may not.
pub(crate) async fn require_write(access_policy: &dyn AccessPolicy, ctx: &SecurityContext, workspace: &Workspace) -> WorkspaceResult<()> {
    access_policy.require(ctx, ActionType::custom(Workspace::KIND, &WRITE, &workspace.security_attributes), |refused| WorkspaceError::ReadOnly { id: workspace.id, refused }).await
}
