//! What an agent's work does with artifacts (issue 516): publishes a file as a version of one -
//! the plugin's service an application reaches, bound by the plugin to every reading
//! (`SessionReader::extension`), so that it acts in the reader's name. The `artifact_tool`
//! component names this trait, never the `ArtifactManager` or the views.

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

/// Publishes artifacts in the reader's name.
#[async_trait]
pub trait ArtifactPublisher: Send + Sync {
    /// Whether artifacts can be published: a storage of content is configured.
    fn is_configured(&self) -> bool;

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

/// The publisher of the views a reading stands on, in their viewer's name: what the plugin binds.
#[async_trait]
impl ArtifactPublisher for Views {
    fn is_configured(&self) -> bool {
        self.artifact_manager().is_ok_and(|artifacts| artifacts.is_configured())
    }

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
