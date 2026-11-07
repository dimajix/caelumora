//! What a turn stages for a system service, committed by the service (issue 518): the writer the
//! kernel or a plugin binds to the work is handed the changes of its kind (`StagedChange`) when the
//! turn completes (`Committing`), and what committing them fails with (`CommitError`).

use crate::reader::ViewError;
use async_trait::async_trait;
use caelumora_domain::message::MessageId;
use caelumora_domain::security::Refused;
use caelumora_domain::store::StoreError;
use caelumora_domain::turn::StagedChange;

/// A writer that is handed what a turn staged for it when the turn completes.
#[async_trait]
pub trait Committing<C: StagedChange>: Send + Sync {
    /// Applies `changes`, in the order they were staged, of a turn that ended with message
    /// `after`, in the work's name, each checked: all of them, or none.
    ///
    /// # Errors
    /// `Invalid` for a change that is not the work's to make, `Refused` for one the policy
    /// refuses, `Store` if they cannot be kept.
    async fn commit(&self, changes: Vec<C>, after: MessageId) -> CommitResult<()>;
}

/// What committing what a turn staged fails with (`Committing::commit`,
/// `ActorRuntime::commit_staged`).
#[derive(Debug, thiserror::Error)]
pub enum CommitError {
    /// Nothing commits what was staged of the kind here: no writer is bound to commit it, or
    /// none is offered where the work is - its channel gone, say.
    #[error("nothing commits what was staged as '{0}' here")]
    NotOffered(String),
    /// The session the work reads could not be reached to commit in: refused, or failed.
    #[error("the session of the work cannot be reached")]
    Unreachable(#[source] ViewError),
    /// What was staged of the kind is not what its writer commits.
    #[error("what was staged as '{kind}' cannot be read")]
    Unreadable {
        kind: String,
        #[source]
        source: serde_json::Error,
    },
    /// A change that is not the work's to make - of an entry it may not change, classified above
    /// what it may write -, in words for the log.
    #[error("{0}")]
    Invalid(String),
    /// A change the policy refuses in the work's name.
    #[error("{what} is refused")]
    Refused {
        what: String,
        #[source]
        refused: Refused,
    },
    /// What was committed could not be kept.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// What committing what a turn staged answers.
pub type CommitResult<T> = Result<T, CommitError>;
