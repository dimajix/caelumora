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
//! (`scoped`, the `WorkspaceManager` its `ScopeMember`) - its place removed
//! before it is purged.
//!
//! The commands run in workspaces are ended by the `CommandReaper` (`reaper`) once nobody may run
//! them any longer (issue 254). An agent's work uses the workspace of its channel through the
//! kernel's extensions, a reader and a writer (`extension`, issue 516).

mod extension;
mod manager;
pub(crate) mod reaper;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub(crate) use extension::{UsableWorkspace, WorkspaceUse};
pub use manager::WorkspaceManager;

use crate::terminal::TerminalProvider;
use caelumora_api::workspace::{WorkspaceError, WorkspaceResult};
use caelumora_model::confidentiality::{
    Classification, ConfidentialityPolicy, ConfidentialityQuestions,
};
use caelumora_model::security::{AccessPolicy, ActionType, Enforcement, SecurityContext, WRITE};
use caelumora_model::store::{StoreError, Stored};
use caelumora_model::workspace::{Workspace, WorkspaceId};

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
pub(crate) fn require_classification(
    policy: &dyn ConfidentialityPolicy,
    ctx: &SecurityContext,
    storage: &str,
    provider: &dyn TerminalProvider,
    classification: &Classification,
) -> WorkspaceResult<()> {
    policy.require_classifiable(classification, || WorkspaceError::ClassificationRefused)?;
    policy.require_storable(ctx, classification, provider.destination(), || {
        WorkspaceError::StorageNotCleared(storage.to_string())
    })?;
    policy.require_cleared_for(ctx, classification, || WorkspaceError::NotCleared)
}

/// Whether whoever `ctx` acts for may change the files of `workspace`
/// (`WriteWorkspace`, issue 23), once they reached it.
///
/// # Errors
/// `ReadOnly` if they may not.
pub(crate) async fn require_write(
    access_policy: &dyn AccessPolicy,
    ctx: &SecurityContext,
    workspace: &Workspace,
) -> WorkspaceResult<()> {
    access_policy
        .require(
            ctx,
            ActionType::custom(Workspace::KIND, &WRITE, &workspace.security_attributes),
            |refused| WorkspaceError::ReadOnly { id: workspace.id, refused },
        )
        .await
}
