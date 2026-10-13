//! An artifact as a viewer reaches it (issues 271, 23, 316): read by its
//! list, wherever they act and cleared for its classification, with the
//! versions whose own list they are read by as well
//! (`Artifact::version_attributes`). Nothing of its content is reachable
//! past it but through `content`, which hands out only a version it holds.

use super::Views;
use crate::artifact::{ArtifactError, ArtifactResult};
use crate::terminal::ByteStream;
use caelumora_model::artifact::Artifact;
use caelumora_model::security::AccessQuestions;
use caelumora_model::artifact::ArtifactVersion;
use caelumora_model::snapshot::{ArtifactSnapshot, Detail, PublicationSnapshot};
use std::collections::BTreeMap;
use tracing::warn;

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
    /// to it, for its owner whom it and its versions are shared with, and
    /// with `Detail::PUBLICATIONS` where its versions were published.
    pub async fn snapshot(&self, detail: Detail) -> ArtifactSnapshot {
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
        let mut publications = BTreeMap::new();
        if detail.contains(Detail::PUBLICATIONS) {
            for version in &self.artifact.versions {
                if let Some(publication) = self.publication(version).await {
                    publications.insert(version.number, publication);
                }
            }
        }
        ArtifactSnapshot { artifact: self.artifact.clone(), can_change, can_write, access, version_access, publications }
    }

    /// Where `version` was published and by whom, if the viewer may read
    /// the channel - reached as every channel is (`Views::channel_of`), so
    /// that a version shared beyond it names nothing of it. A channel that
    /// cannot be read is logged and named as one the viewer may not read.
    async fn publication(&self, version: &ArtifactVersion) -> Option<PublicationSnapshot> {
        let channel = match self.views.channel_of(version.session_id, version.channel_id).await {
            Ok(reached) => reached.there()?,
            Err(e) => {
                warn!(artifact = %self.artifact.id, version = version.number, "Where the version was published cannot be read: {e}");
                return None;
            }
        };
        let published_by = match channel.member(version.published_by).await {
            Ok(member) => member.name().map(str::to_string),
            Err(_) => None,
        };
        Some(PublicationSnapshot { session_name: channel.session().name().to_string(), channel_name: channel.name().to_string(), published_by })
    }
}
