//! An artifact as a viewer reaches it (issues 271, 23): with the versions
//! they may read - by the artifact's list or by a version's own
//! (`Artifact::version_attributes`) -, wherever they act and cleared for its
//! classification. Nothing of its content is reachable past it but through
//! `content`, which hands out only a version it holds.

use super::Views;
use crate::artifact::{ArtifactError, ArtifactResult};
use crate::terminal::ByteStream;
use caelumora_model::artifact::Artifact;
use caelumora_model::security::AccessQuestions;
use caelumora_model::snapshot::ArtifactSnapshot;
use std::collections::BTreeMap;

#[derive(Clone)]
pub struct ArtifactView {
    views: Views,
    /// With the versions the viewer may read alone.
    artifact: Artifact,
}

impl ArtifactView {
    pub(super) fn of(views: Views, artifact: Artifact) -> Self {
        Self { views, artifact }
    }

    /// The artifact, with the versions the viewer may read alone.
    pub fn artifact(&self) -> &Artifact {
        &self.artifact
    }

    /// The content of version `number`, if it is one the viewer may read -
    /// asked when the view was made -, with the artifact and its size.
    ///
    /// # Errors
    /// `VersionNotFound` if it is no version they may read, or does not
    /// exist; `UnknownStorage` or `Unavailable`.
    pub async fn content(&self, number: u32) -> ArtifactResult<(u64, ByteStream)> {
        if self.artifact.version(number).is_none() {
            return Err(ArtifactError::VersionNotFound { artifact: self.artifact.id, version: number });
        }
        self.views.artifact_manager().content_of(&self.artifact, number).await
    }

    /// The artifact as the viewer sees it: whether they may change it or add
    /// to it, and for its owner whom it and its versions are shared with.
    pub async fn snapshot(&self) -> ArtifactSnapshot {
        let context = self.views.context();
        let security = self.views.security();
        let can_change = security.may_change_artifact(context, &self.artifact.security_attributes).await;
        let can_write = security.may_write_artifact(context, &self.artifact.security_attributes).await;
        let (mut access, mut version_access) = (Vec::new(), BTreeMap::new());
        if can_change {
            access = self.views.access_snapshots(&self.artifact.security_attributes.acl).await;
            for version in &self.artifact.versions {
                version_access.insert(version.number, self.views.access_snapshots(&version.acl).await);
            }
        }
        ArtifactSnapshot { artifact: self.artifact.clone(), can_change, can_write, access, version_access }
    }
}
