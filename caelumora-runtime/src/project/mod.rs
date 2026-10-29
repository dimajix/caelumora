//! Projects (issue 467): what holds the results of sessions. Making, deleting, restoring and
//! purging a shared project are the `ProjectManager`'s (`manager`), with the sessions that lie in
//! it and what lies in its scope; what a project is - its name, description, members, access list
//! and where it lies - its controller changes (`controller::ProjectController`). A session's own
//! project is the session's: the `SessionManager` makes, deletes, restores and purges it with the
//! session. A session is put into a project or taken out into its own by its
//! `SessionLoop::update_project`, which makes it a new own where its own was lifted.

mod manager;

pub use manager::ProjectManager;

use crate::scoped::ScopeNotEmpty;
use caelumora_model::project::ProjectId;
use caelumora_model::store::StoreError;
use caelumora_model::user::UserId;
use caelumora_model::value::ValueError;

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    /// No such project, or not one the caller may read - one case, so that nothing can be probed.
    #[error("there is no project {0}")]
    NotFound(ProjectId),
    #[error("a project belongs to a user")]
    OwnerNotAUser,
    /// Made or lifted where a shared project does not lie - it lies in the global scope or its
    /// owner's -, or a session's own project asked to be deleted, restored or purged by itself,
    /// which goes with its session, or to be lifted once it is shared.
    #[error(
        "a project lies in the global scope or its owner's; a session's own goes with the session"
    )]
    InvalidScope,
    #[error("there is no user {0}")]
    UnknownMember(UserId),
    #[error(transparent)]
    ScopeNotEmpty(#[from] ScopeNotEmpty),
    #[error(transparent)]
    Invalid(#[from] ValueError),
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A session of the project that could not be deleted, restored or purged with it.
    #[error("a session of the project: {0}")]
    Session(#[source] anyhow::Error),
}

pub type ProjectResult<T> = Result<T, ProjectError>;

/// `error` of the store about project `id`: not found is one case with not readable.
pub(crate) fn store_error(id: ProjectId) -> impl FnOnce(StoreError) -> ProjectError {
    move |error| match error {
        StoreError::NotFound { .. } => ProjectError::NotFound(id),
        error => ProjectError::Store(error),
    }
}
