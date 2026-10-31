//! What an agent's work does with artifacts (issue 516): publishes a file as a version of one - the
//! plugin's service a library reaches, a writer bound by the plugin to the views of every piece of
//! work where a storage of content is configured, and handed out where the work acts
//! (`Turn::writer`), so that it acts in the work's context. The `artifact_tool` component names
//! this trait, never the `ArtifactManager` or the views.

use crate::artifact::Artifact;
use crate::manager::ArtifactResult;
use crate::view::ArtifactViews;
use async_trait::async_trait;
use caelumora_model::channel::Channel;
use caelumora_model::confidentiality::Classification;
use caelumora_model::store::Address;
use caelumora_model::value::name::ScopedName;
use caelumora_runtime::terminal::ByteStream;
use caelumora_runtime::view::Views;

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

/// The publisher of the views of a piece of work, in their viewer's name: what the plugin binds.
#[async_trait]
impl ArtifactPublisher for Views {
    async fn publish(
        &self,
        channel: Address<Channel>,
        content: (u64, ByteStream),
        classification: &Classification,
        name: &ScopedName,
        description: Option<&str>,
    ) -> ArtifactResult<(Artifact, u32)> {
        let artifacts = self.artifact_manager()?;
        artifacts.publish(self.context(), channel, content, classification, name, description).await
    }
}
