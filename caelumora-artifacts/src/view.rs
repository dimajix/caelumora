//! Artifacts as a viewer reaches them (issues 271, 23, 316): their own and
//! those shared with them by name, among their artifacts; those published
//! in a channel they read, through the channel - the plugin's views, an
//! extension of the runtime's (`ArtifactViews`) and of the channel's view
//! (`ArtifactChannelViews`, issue 328). An artifact is read by its list,
//! wherever the viewer acts and cleared for its classification, with the
//! versions whose own list they are read by as well
//! (`Artifact::version_attributes`). Nothing of its content is reachable
//! past it but through `content`, which hands out only a version it holds.

use crate::access::ArtifactAccess;
use crate::artifact::{Artifact, ArtifactField, ArtifactId, ArtifactSnapshot, ArtifactVersion, PublicationSnapshot};
use crate::manager::{ArtifactError, ArtifactManager, ArtifactResult};
use async_trait::async_trait;
use caelumora_model::security::AccessQuestions;
use caelumora_model::snapshot::DeletedSnapshot;
use caelumora_model::confidentiality::Reading;
use caelumora_model::store::query::{Filter, Query};
use caelumora_model::store::{StoreResultExt, Stored};
use caelumora_runtime::terminal::ByteStream;
use caelumora_runtime::view::{ChannelView, Views};
use std::collections::BTreeMap;
use std::sync::Arc;
use tracing::warn;
use std::mem;

/// The artifacts the views reach (issue 328): what the runtime's `Views`
/// offer of artifacts, from the plugin.
#[async_trait]
pub trait ArtifactViews {
    /// The viewer's artifacts (issues 271, 23, 293, 308, 316): those they
    /// own and those shared with them by name - an entry of the artifact's
    /// list naming them, one of their roles or a team they belong to
    /// (`Views::subjects`) -, each with the versions they may read, if they
    /// are cleared for it. What they read only as a participant of a channel is
    /// found through that channel (`ArtifactChannelViews::artifacts`), so
    /// the participants of a session or channel are not named. The store
    /// is asked for these alone, in the viewer's name.
    ///
    /// # Errors
    /// `Store` if the store cannot list them.
    async fn artifacts(&self) -> ArtifactResult<Vec<ArtifactView>>;

    /// Artifact `id`, with the versions the viewer may read, if they may
    /// read it and are cleared for it (issues 271, 23, 316).
    ///
    /// # Errors
    /// `NotFound` if they may not read it - one they are not cleared for is
    /// the same -; `Store` if the store cannot read it.
    async fn artifact(&self, id: ArtifactId) -> ArtifactResult<ArtifactView>;

    /// The artifacts the viewer deleted and may read as such (issue 271).
    ///
    /// # Errors
    /// `Store` if the store cannot list them.
    async fn deleted_artifacts(&self) -> ArtifactResult<Vec<DeletedSnapshot<ArtifactId>>>;

    /// The one owner of artifacts, which the plugin offers as a service.
    ///
    /// # Errors
    /// `NotJoined` if the runtime was built without the plugin.
    fn artifact_manager(&self) -> ArtifactResult<&Arc<ArtifactManager>>;
}

#[async_trait]
impl ArtifactViews for Views {
    async fn artifacts(&self) -> ArtifactResult<Vec<ArtifactView>> {
        let context = self.context();
        let named = Filter::is_in(ArtifactField::Subject, self.subjects().await?);
        let theirs = Filter::Or(vec![Filter::eq(ArtifactField::Owner, context.principal_id), Filter::any(ArtifactField::Acl, named)]);
        let artifacts = self.artifact_manager()?.store().list(context, &Query::filtered(theirs)).await?.items;
        let mut readable = Vec::with_capacity(artifacts.len());
        for artifact in artifacts {
            readable.push(readable_by(self, artifact).await);
        }
        Ok(readable)
    }

    async fn artifact(&self, id: ArtifactId) -> ArtifactResult<ArtifactView> {
        match self.artifact_manager()?.store().get(self.context(), id).await.optional()? {
            Some(artifact) => Ok(readable_by(self, artifact).await),
            None => Err(ArtifactError::NotFound(id)),
        }
    }

    async fn deleted_artifacts(&self) -> ArtifactResult<Vec<DeletedSnapshot<ArtifactId>>> {
        let artifacts = self.artifact_manager()?.store().list_deleted(self.context(), &Query::all()).await?.items;
        let access_policy = self.access_policy();
        let mut deleted = Vec::with_capacity(artifacts.len());
        for artifact in artifacts {
            let Some(removal) = artifact.deleted.clone() else { continue };
            let attributes = &artifact.security_attributes;
            deleted.push(DeletedSnapshot {
                id: artifact.id,
                name: artifact.name.to_string(),
                deleted: removal,
                can_restore: access_policy.may_restore(self.context(), Artifact::KIND, attributes).await,
                can_purge: access_policy.may_purge(self.context(), Artifact::KIND, attributes).await,
            });
        }
        Ok(deleted)
    }

    fn artifact_manager(&self) -> ArtifactResult<&Arc<ArtifactManager>> {
        self.service::<ArtifactManager>().ok_or(ArtifactError::NotJoined)
    }
}

/// The artifacts a channel's view reaches (issue 328): what was published
/// in it, from the plugin. Whether the viewer reads the channel is the
/// channel's to say (`ChannelView::reading`).
#[async_trait]
pub trait ArtifactChannelViews {
    /// The artifacts with a version published in this channel that the
    /// viewer may read, each with the versions published here alone - for
    /// a reader of the whole channel, since what was published in it is
    /// part of what was said there (issues 271, 23). Reading them goes
    /// through the artifact's list, which publishing granted the channel's
    /// participants (issue 316); the content of a version is reached by the
    /// artifact, never through the channel.
    ///
    /// # Errors
    /// `Store` if the store cannot list them, `NotJoined` without the
    /// plugin.
    async fn artifacts(&self) -> ArtifactResult<Vec<ArtifactView>>;
}

#[async_trait]
impl ArtifactChannelViews for ChannelView {
    async fn artifacts(&self) -> ArtifactResult<Vec<ArtifactView>> {
        if *self.reading() != Reading::Whole {
            return Ok(Vec::new());
        }
        let views = self.views();
        let (session, channel) = (self.session().id(), self.id());
        // Those with a version published here, which the store is asked for
        // alone (issue 293).
        let here = Filter::And(vec![
            Filter::eq(ArtifactField::VersionSession, session),
            Filter::eq(ArtifactField::VersionChannel, channel),
        ]);
        let all = views.artifact_manager()?.store().list(views.context(), &Query::filtered(Filter::any(ArtifactField::Versions, here))).await?.items;
        let mut published = Vec::new();
        for mut artifact in all {
            artifact.versions.retain(|version| version.session_id == session && version.channel_id == channel);
            let view = readable_by(views, artifact).await;
            if !view.artifact().versions.is_empty() {
                published.push(view);
            }
        }
        Ok(published)
    }
}

/// `artifact`, as the store handed it to the viewer of `views` - who reads
/// it -, with the versions whose own list lets them read them as well
/// (issue 316).
async fn readable_by(views: &Views, mut artifact: Artifact) -> ArtifactView {
    let context = views.context();
    let mut readable = Vec::with_capacity(artifact.versions.len());
    for version in mem::take(&mut artifact.versions) {
        if views.access_policy().may_read_artifact(context, &artifact.version_attributes(&version)).await {
            readable.push(version);
        }
    }
    artifact.versions = readable;
    ArtifactView { views: views.clone(), artifact }
}

#[derive(Clone)]
pub struct ArtifactView {
    views: Views,
    /// With the versions the viewer may read alone.
    artifact: Artifact,
}

impl ArtifactView {
    /// The artifact, with the versions the viewer may read alone.
    pub fn artifact(&self) -> &Artifact {
        &self.artifact
    }

    /// The content of version `number`, if it is one the viewer may read -
    /// asked when the view was made -, with its size.
    ///
    /// # Errors
    /// `VersionNotFound` if it is no version they may read, or does not exist; `StorageGone` or
    /// `Content`; `NotJoined` without the plugin.
    pub async fn content(&self, number: u32) -> ArtifactResult<(u64, ByteStream)> {
        if self.artifact.version(number).is_none() {
            return Err(ArtifactError::VersionNotFound { artifact: self.artifact.id, version: number });
        }
        self.views.artifact_manager()?.content_of(&self.artifact, number).await
    }

    /// The artifact as the viewer sees it: whether they may change it or add
    /// to it, and for its owner whom it and its versions are shared with -
    /// not where its versions were published, as where artifacts are
    /// listed.
    pub async fn snapshot(&self) -> ArtifactSnapshot {
        self.snapshot_of(BTreeMap::new()).await
    }

    /// The artifact as the viewer sees it, as `snapshot`, with where its
    /// versions were published and by whom (issue 291) - where one artifact
    /// is read, since every version's channel is reached for it.
    pub async fn snapshot_with_publications(&self) -> ArtifactSnapshot {
        let mut publications = BTreeMap::new();
        for version in &self.artifact.versions {
            if let Some(publication) = self.publication(version).await {
                publications.insert(version.number, publication);
            }
        }
        self.snapshot_of(publications).await
    }

    async fn snapshot_of(&self, publications: BTreeMap<u32, PublicationSnapshot>) -> ArtifactSnapshot {
        let context = self.views.context();
        let access_policy = self.views.access_policy();
        let can_change = access_policy.may_change_artifact(context, &self.artifact.security_attributes).await;
        let can_write = access_policy.may_write_artifact(context, &self.artifact.security_attributes).await;
        let (mut access, mut version_access) = (Vec::new(), BTreeMap::new());
        if can_change {
            access = self.views.access_snapshots(&self.artifact.security_attributes.acl).await;
            for version in &self.artifact.versions {
                version_access.insert(version.number, self.views.access_snapshots(&version.acl).await);
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
        let published_by = match channel.participant(version.published_by).await {
            Ok(participant) => participant.name().map(str::to_string),
            Err(_) => None,
        };
        Some(PublicationSnapshot { session_name: channel.session().name().to_string(), channel_name: channel.name().to_string(), published_by })
    }
}
