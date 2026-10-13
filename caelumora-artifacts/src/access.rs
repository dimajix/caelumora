//! Who may change, read and add to an artifact (issues 271, 23): the pure
//! questions about an artifact, an extension of `AccessQuestions` (issue
//! 323) - each translated into an action, decided by the policy alone.
//! Every `SecurityManager` has them by the blanket implementation, so that
//! no policy answers them otherwise.

use crate::artifact::Artifact;
use async_trait::async_trait;
use caelumora_model::removal::Stored;
use caelumora_model::security::{ActionType, SecurityAttributes, SecurityContext, SecurityManager, WRITE};

#[async_trait]
pub trait ArtifactAccess {
    /// Whether `context` may change an artifact with these attributes -
    /// keep it in another scope, describe it, share it, delete it
    /// (`UpdateArtifact`, issue 271).
    async fn may_change_artifact(&self, context: &SecurityContext, artifact: &SecurityAttributes) -> bool;

    /// Whether `context` may read what has these attributes - an artifact,
    /// or one of its versions (`GetArtifact`, issue 23).
    async fn may_read_artifact(&self, context: &SecurityContext, artifact: &SecurityAttributes) -> bool;

    /// Whether `context` may add a version to an artifact with these
    /// attributes (`WriteArtifact`, issue 23).
    async fn may_write_artifact(&self, context: &SecurityContext, artifact: &SecurityAttributes) -> bool;
}

#[async_trait]
impl<T: SecurityManager + ?Sized> ArtifactAccess for T {
    async fn may_change_artifact(&self, context: &SecurityContext, artifact: &SecurityAttributes) -> bool {
        self.check(context, ActionType::update(Artifact::KIND, artifact)).await
    }

    async fn may_read_artifact(&self, context: &SecurityContext, artifact: &SecurityAttributes) -> bool {
        self.check(context, ActionType::get(Artifact::KIND, artifact)).await
    }

    async fn may_write_artifact(&self, context: &SecurityContext, artifact: &SecurityAttributes) -> bool {
        self.check(context, ActionType::custom(Artifact::KIND, &WRITE, artifact)).await
    }
}
