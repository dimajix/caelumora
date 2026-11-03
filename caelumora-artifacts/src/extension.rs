//! The part of the kernel's API that is artifacts' (issue 516): what an agent's work does with
//! them, and what that fails with. The work publishes a file as a version of one - the plugin's
//! service a library reaches, a writer bound by the plugin to the views of every piece of work
//! where a storage of content is configured, and handed out where the work acts (`Turn::writer`),
//! so that it acts in the work's context. The `artifacts` component names this trait, never the
//! `ArtifactManager` or the views. Its error, `ArtifactError`, is the one error of artifacts, which
//! the manager, the views and the routes answer in as well.

use crate::artifact::{Artifact, ArtifactId};
use async_trait::async_trait;
use caelumora_api::error::ContentError;
use caelumora_api::error::StorageGone;
use caelumora_api::terminal::ByteStream;
use caelumora_domain::channel::Channel;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::store::{Address, StoreError};
use caelumora_domain::value::name::ScopedName;
use caelumora_domain::value::ValueError;

/// Publishes artifacts as the work it is bound to reads: in its name, for its initiator.
#[async_trait]
pub trait ArtifactPublisher: Send + Sync {
    /// Publishes `content` - its size and its bytes -, classified `classification`, as a version
    /// of the artifact `name`, in the channel at `channel`, saying what it is in `description`
    /// (`ArtifactManager::publish`). Answers the artifact as it is now, and the number of the new
    /// version.
    ///
    /// # Errors
    /// As `ArtifactManager::publish`.
    async fn publish(
        &self,
        channel: Address<Channel>,
        content: (u64, ByteStream),
        classification: &Classification,
        name: &ScopedName,
        description: Option<&str>,
    ) -> ArtifactResult<(Artifact, u32)>;
}

/// What the operations on artifacts fail with (issue 271).
#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("no artifact {0}")]
    NotFound(ArtifactId),
    #[error("no version {version} of artifact {artifact}")]
    VersionNotFound { artifact: ArtifactId, version: u32 },
    #[error("no storage of content is configured for artifacts")]
    NotConfigured,
    #[error("an artifact is published by a user, or for one")]
    OwnerNotAUser,
    /// What is so classified may not be put into the storage.
    #[error("the storage of content '{0}' may not hold what is so classified")]
    StorageNotCleared(String),
    /// A later version from a workspace classified otherwise than the first.
    #[error("the artifact '{0}' is classified otherwise than the workspace; publish it under another name")]
    ClassifiedOtherwise(String),
    /// Published under a name in a scope an artifact does not lie in: the caller vouches for a
    /// project's (issue 467).
    #[error("an artifact lies in a project's scope")]
    InvalidScope,
    #[error(transparent)]
    Invalid(#[from] ValueError),
    /// What the storage of content reported: a version too large, other than it was said to be,
    /// or a storage that failed.
    #[error(transparent)]
    Content(#[from] ContentError),
    /// The storage a version lies in, since removed from the configuration.
    #[error(transparent)]
    StorageGone(#[from] StorageGone),
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The kernel was built without the plugin, so there is no manager to ask.
    #[error("artifacts are not joined to this kernel")]
    NotJoined,
}

pub type ArtifactResult<T> = Result<T, ArtifactError>;
