//! The wire of artifacts (issue 271): as their owner or a reader of a
//! channel sees them, and how they are kept.

use crate::artifact::{ArtifactId, ArtifactSnapshot};
use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::Classification;
use caelumora_model::digest::Digest;
use caelumora_model::project::ProjectId;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::value::text::Description;
use caelumora_rest_api::access::AccessEntry;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// An artifact (issue 271) as its owner sees it, among their artifacts -
/// with every version -, or a reader of a channel - with the versions
/// published there alone.
#[derive(Debug, Serialize)]
pub struct ArtifactResponse {
    pub id: ArtifactId,
    pub name: String,
    /// The project it lies in (issue 467): gone with it - with its session where that is the
    /// session's own -, kept beyond the session where it is shared. Absent only for an artifact
    /// that could not be stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<ProjectId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Description>,
    pub classification: Classification,
    /// Oldest first; those you may read alone (issue 23).
    pub versions: Vec<ArtifactVersionResponse>,
    /// Whether you may change it - describe it, share it, delete it: its owner.
    pub can_change: bool,
    /// Whether you may add a version to it (issue 23).
    pub can_write: bool,
    /// Whom it is shared with as a whole (issue 23) - only for its owner.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub access: Vec<AccessEntry>,
}

/// A version of an artifact: what was published, where and by whom.
#[derive(Debug, Serialize)]
pub struct ArtifactVersionResponse {
    pub number: u32,
    pub size: u64,
    pub digest: Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// What it is, as it was described when it was published (issue 23).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Description>,
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    pub published_by: ActorId,
    pub published_at: DateTime<Utc>,
    /// The names of where it was published and of who published it (issue
    /// 291) - where one artifact is fetched, for whoever may read that
    /// channel; `published_by_name` is absent too for a publisher gone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_by_name: Option<String>,
    /// Who reads it besides whom the artifact is shared with - the participants
    /// of the channel it was published in - only for its owner.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub access: Vec<AccessEntry>,
}

impl From<ArtifactSnapshot> for ArtifactResponse {
    fn from(view: ArtifactSnapshot) -> Self {
        let ArtifactSnapshot {
            artifact,
            can_change,
            can_write,
            access,
            mut version_access,
            mut publications,
        } = view;
        let project_id = artifact.project();
        let versions = artifact
            .versions
            .into_iter()
            .map(|v| {
                let (session_name, channel_name, published_by_name) =
                    match publications.remove(&v.number) {
                        Some(p) => (Some(p.session_name), Some(p.channel_name), p.published_by),
                        None => (None, None, None),
                    };
                ArtifactVersionResponse {
                    access: version_access
                        .remove(&v.number)
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(AccessEntry::of)
                        .collect(),
                    number: v.number,
                    size: v.size,
                    digest: v.digest,
                    media_type: v.media_type,
                    description: v.description,
                    session_id: v.session_id,
                    channel_id: v.channel_id,
                    published_by: v.published_by,
                    published_at: v.published_at,
                    session_name,
                    channel_name,
                    published_by_name,
                }
            })
            .collect();
        let access = access.into_iter().filter_map(AccessEntry::of).collect();
        Self {
            id: artifact.id,
            name: artifact.name.name.to_string(),
            project_id,
            description: artifact.description,
            classification: artifact.classification,
            versions,
            can_change,
            can_write,
            access,
        }
    }
}

/// How an artifact is shared (issue 316): by default, whom its list newly
/// names reads every version there is as well; `?versions=false` leaves the
/// lists of its versions as they are.
#[derive(Debug, Deserialize)]
pub struct ShareOptions {
    #[serde(default = "every_version")]
    pub versions: bool,
}

fn every_version() -> bool {
    true
}
