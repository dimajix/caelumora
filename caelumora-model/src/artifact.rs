//! An artifact (issue 271): a result an agent published, kept as fixed
//! versions beyond the workspace it was copied from. It belongs to the user
//! the agent acts for and lies in a scope - the session's, made with the
//! session and gone with it, until its owner keeps it in their own scope or
//! the global one. One classification holds for all its versions: the one
//! its first version was published with. Its content lies in the storage
//! the artifact names - configured, not stored -, one piece per version;
//! the document holds what is known of them.

use crate::actor::ActorId;
use crate::confidentiality::Classification;
use crate::digest::Digest;
use crate::removal::Removal;
use crate::scope::Scoped;
use crate::security::{Secured, SecurityAttributes};
use crate::session::{ChannelId, SessionId};
use crate::user::UserId;
use crate::value::name::ScopedName;
use crate::value::text::Description;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

define_id!(ArtifactId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: ArtifactId,
    /// What it is found by and where it lies: a file name, unique in its
    /// scope (issue 250). The scope says how long it is kept, not whose it
    /// is: it is its owner's wherever it lies.
    #[serde(flatten)]
    pub name: ScopedName,
    /// The name of the storage its content lies in, as the configuration
    /// names it; fixed when the artifact is made.
    pub storage: String,
    /// What it is, as its publisher or owner describes it; `None` if nobody
    /// said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Description>,
    /// How every version is classified: the workspace's its first version
    /// was copied from, fixed.
    pub classification: Classification,
    /// Its versions, oldest first, numbered from 1 without gaps; never
    /// empty, and none removed but with the artifact.
    pub versions: Vec<ArtifactVersion>,
    /// Owned by the user the publishing agent acts for, with an empty access
    /// list: it is shared through the channels it is published in.
    pub security_attributes: SecurityAttributes,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Set when it was deleted (issue 164): it stays stored, its content
    /// kept, hidden from every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
}

/// One version of an artifact: what was published, where and by whom.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactVersion {
    /// 1 for the first, one more for each later one.
    pub number: u32,
    pub size: u64,
    /// The digest of its content (`digest::content`).
    pub digest: Digest,
    /// What its producer said it is; `None` where it said nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// The channel it was published in: whoever may read that channel may
    /// download it.
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    /// The agent that published it.
    pub published_by: ActorId,
    pub published_at: DateTime<Utc>,
}

impl Artifact {
    /// A new artifact `id` of `owner`, of `name` where it lies, in
    /// `storage`, classified `classification`, with its first version
    /// `first`, whose number is set to 1. The id is given, since its content
    /// is stored by it before the document is.
    pub fn new(id: ArtifactId, name: ScopedName, storage: impl Into<String>, classification: Classification, owner: UserId, mut first: ArtifactVersion) -> Self {
        let now = Utc::now();
        first.number = 1;
        Self {
            id,
            name,
            storage: storage.into(),
            description: None,
            classification,
            versions: vec![first],
            security_attributes: SecurityAttributes::owned_by(ActorId::User(owner)),
            created_at: now,
            updated_at: now,
            deleted: None,
        }
    }

    /// The user it belongs to; `None` only for a document that could not be
    /// stored (`Stored::validate`).
    pub fn owner(&self) -> Option<UserId> {
        match self.security_attributes.owner_id {
            ActorId::User(user_id) => Some(user_id),
            _ => None,
        }
    }

    /// Its version `number`, if it has one.
    pub fn version(&self, number: u32) -> Option<&ArtifactVersion> {
        self.versions.iter().find(|v| v.number == number)
    }

    /// Its latest version; `None` only for a document that could not be
    /// stored.
    pub fn latest(&self) -> Option<&ArtifactVersion> {
        self.versions.last()
    }

    /// Adds `version` as its next one, numbered after the latest, and
    /// answers its number.
    pub fn add_version(&mut self, mut version: ArtifactVersion) -> u32 {
        version.number = self.latest().map_or(1, |latest| latest.number + 1);
        let number = version.number;
        self.versions.push(version);
        self.updated_at = Utc::now();
        number
    }
}

impl crate::removal::Stored for Artifact {
    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    /// An artifact is a user's own, as a workspace is, and has its versions
    /// numbered from 1 without gaps.
    fn validate(&self) -> Result<(), String> {
        crate::scope::validate_users_own("artifact", &self.security_attributes, &self.name.scope)?;
        if self.versions.is_empty() {
            return Err("an artifact has at least one version".to_string());
        }
        if self.versions.iter().zip(1..).any(|(version, number)| version.number != number) {
            return Err("an artifact's versions are numbered from 1 without gaps".to_string());
        }
        Ok(())
    }
}

impl crate::removal::Deletable for Artifact {
    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }
}

impl Secured for Artifact {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl Scoped for Artifact {
    fn scoped_name(&self) -> ScopedName {
        self.name.clone()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::removal::Stored;
    use crate::scope::Scope;

    /// A version of `content`, published in a channel of `session_id`.
    pub(crate) fn version(content: &[u8], session_id: SessionId) -> ArtifactVersion {
        ArtifactVersion {
            number: 0,
            size: content.len() as u64,
            digest: crate::digest::content(content),
            media_type: None,
            session_id,
            channel_id: ChannelId::new(),
            published_by: ActorId::User(UserId::new()),
            published_at: Utc::now(),
        }
    }

    fn artifact(scope: Scope, owner: UserId) -> Artifact {
        Artifact::new(ArtifactId::new(), ScopedName::new("report.csv", scope).unwrap(), "files", Classification::CHANNEL_DEFAULT, owner, version(b"a,b", SessionId::new()))
    }

    #[test]
    fn an_artifact_lies_where_a_workspace_may_and_belongs_to_a_user() {
        let alice = UserId::new();
        let session_id = SessionId::new();
        for scope in [Scope::SessionScope { session_id }, Scope::UserScope { user_id: alice }, Scope::GlobalScope] {
            assert!(artifact(scope.clone(), alice).validate().is_ok(), "{scope:?}");
        }
        for scope in [Scope::UserScope { user_id: UserId::new() }, Scope::ChannelScope { session_id, channel_id: ChannelId::new() }] {
            assert!(artifact(scope.clone(), alice).validate().is_err(), "not in {scope:?}");
        }
        let systems = Artifact { security_attributes: SecurityAttributes::owned_by(ActorId::System), ..artifact(Scope::GlobalScope, alice) };
        assert!(systems.validate().is_err(), "nobody's");
    }

    #[test]
    fn versions_are_numbered_from_one_and_kept() {
        let session_id = SessionId::new();
        let mut kept = artifact(Scope::SessionScope { session_id }, UserId::new());
        assert_eq!(kept.latest().map(|v| v.number), Some(1));
        assert_eq!(kept.add_version(version(b"a,b\n1,2", session_id)), 2);
        assert_eq!((kept.version(1).map(|v| v.size), kept.version(2).map(|v| v.size)), (Some(3), Some(7)));
        assert!(kept.validate().is_ok());

        let gapped = Artifact { versions: vec![kept.versions[1].clone()], ..kept.clone() };
        assert!(gapped.validate().is_err(), "not from 2");
        let empty = Artifact { versions: vec![], ..kept };
        assert!(empty.validate().is_err(), "not without one");
    }

    #[test]
    fn an_artifact_is_written_without_what_it_does_not_have() {
        let made = artifact(Scope::GlobalScope, UserId::new());
        let written = serde_json::to_value(&made).unwrap();
        assert!(written.get("description").is_none() && written.get("deleted").is_none(), "{written}");
        assert!(written["versions"][0].get("media_type").is_none());
        let read: Artifact = serde_json::from_value(written).unwrap();
        assert_eq!((read.id, read.name, read.versions), (made.id, made.name, made.versions));
    }
}
