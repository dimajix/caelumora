//! An artifact (issue 271): a result an agent published, kept as fixed versions beyond the
//! workspace it was copied from. It belongs to the user the agent acts for and lies in the scope of
//! the project its session's results go to (issue 467), as long as the project: a session's own
//! goes with the session, a shared one outlives it. One classification holds for all its versions:
//! the one its first version was published with. Its content lies in the storage the artifact
//! names, configured, not stored, one piece per version; the document holds what is known of them.

use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::Classification;
use caelumora_model::define_id;
use caelumora_model::digest::Digest;
use caelumora_model::project::ProjectId;
use caelumora_model::removal::Removal;
use caelumora_model::scope::{Scope, Scoped, ScopedField};
use caelumora_model::security::{AclEntry, Grant, Secured, SecurityAttributes, Subject};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::snapshot::AccessSnapshot;
use caelumora_model::store::query::Field;
use caelumora_model::store::{EntityKind, NoEntity, Stored};
use caelumora_model::user::UserId;
use caelumora_model::value::name::ScopedName;
use caelumora_model::value::text::Description;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::iter;

define_id!(ArtifactId);

/// A change of an artifact other than its first version, which makes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArtifactUpdateEvent {
    /// A version after the first was published.
    VersionPublished(u32),
    /// Its access list changed. Without the list: only its owner sees it.
    AccessChanged,
    /// What it is, as its owner describes it, as it now stands; `None`
    /// takes the description away.
    DescriptionChanged(Option<String>),
}

/// What a query of artifacts may name (issue 293), each where it lies in
/// an artifact as it is stored. Those of a version lie within an element of
/// `Versions`, those of an entry within an element of `Acl` or `VersionAcl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactField {
    /// Where it lies: a `Scope`.
    Scope,
    /// Who owns it: an `ActorId`.
    Owner,
    /// Its own access list: entries.
    Acl,
    /// Its versions.
    Versions,
    /// Of a version: the session it was published in.
    VersionSession,
    /// Of a version: the channel it was published in.
    VersionChannel,
    /// Of a version: its own access list - entries.
    VersionAcl,
    /// Of an entry of an access list: whom it names, a `Subject`.
    Subject,
}

impl ScopedField for ArtifactField {
    const SCOPE: Self = ArtifactField::Scope;
}

impl Field for ArtifactField {
    fn path(self) -> &'static [&'static str] {
        match self {
            ArtifactField::Scope => &["scope"],
            ArtifactField::Owner => &["security_attributes", "owner_id"],
            ArtifactField::Acl => &["security_attributes", "acl"],
            ArtifactField::Versions => &["versions"],
            ArtifactField::VersionSession => &["session_id"],
            ArtifactField::VersionChannel => &["channel_id"],
            ArtifactField::VersionAcl => &["acl"],
            ArtifactField::Subject => &["subject"],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: ArtifactId,
    /// What it is found by and where it lies: a file name, unique in its project (issue 250), in
    /// the project's scope (issue 467). The project says how long it is kept, not whose it is: it
    /// is its owner's.
    #[serde(flatten)]
    pub name: ScopedName,
    /// The name of the storage its content lies in, as the configuration
    /// names it; fixed when the artifact is made.
    pub storage: String,
    /// What it is as a whole, as its owner describes it; `None` if they did
    /// not. What a publication says goes to its version (issue 23).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Description>,
    /// How every version is classified: the workspace's its first version
    /// was copied from, fixed.
    pub classification: Classification,
    /// Its versions, oldest first, numbered from 1 without gaps; never
    /// empty, and none removed but with the artifact.
    pub versions: Vec<ArtifactVersion>,
    /// Owned by the user the publishing agent acts for. Its list grants
    /// reading and adding to the artifact (issue 316): the participants of
    /// every channel a version was published in, and whom its owner shares
    /// it with. Reading a version needs the version's list besides.
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
    /// What it is, as it was described when it was published; `None` if
    /// nobody said. The version's own, so that what was said of a draft
    /// reaches nobody who reads a later version alone (issue 23).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Description>,
    /// The channel it was published in.
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    /// The agent that published it.
    pub published_by: ActorId,
    pub published_at: DateTime<Utc>,
    /// Who reads this version, of those who read the artifact (issues 23,
    /// 316): reading it needs both lists, as a file in HDFS needs its
    /// directory's. As it is published, the participants of the channel it was
    /// published in. Grants reading alone; the artifact's owner and its list
    /// are the artifact's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acl: Vec<AclEntry>,
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

    /// The project it lies in (issue 467); `None` only for a document that could not be stored
    /// (`Stored::validate`).
    pub fn project(&self) -> Option<ProjectId> {
        self.name.scope.project()
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

    /// Who reads version `version` of those who read the artifact (issue
    /// 316): its owner, and whom the version's own list grants it - as
    /// attributes a policy decides by. Reading the version needs the
    /// artifact's attributes to grant it as well.
    pub fn version_attributes(&self, version: &ArtifactVersion) -> SecurityAttributes {
        SecurityAttributes { owner_id: self.security_attributes.owner_id, acl: version.acl.clone() }
    }

    /// Its access list and those of its versions, to be changed alike - as
    /// entries naming a group that is gone are dropped (issue 347).
    pub fn access_lists_mut(&mut self) -> impl Iterator<Item = &mut Vec<AclEntry>> {
        iter::once(&mut self.security_attributes.acl).chain(self.versions.iter_mut().map(|version| &mut version.acl))
    }

    /// Lets `subject` read the artifact, unless its list grants it already
    /// (issue 316): whoever is to read a version has to read the artifact.
    pub fn grant_reading(&mut self, subject: Subject) {
        grant_reading(&mut self.security_attributes.acl, subject);
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

impl Stored for Artifact {
    const KIND: EntityKind = EntityKind::new("artifact");
    type Id = ArtifactId;
    type Within = NoEntity;
    type Field = ArtifactField;

    fn id(&self) -> ArtifactId {
        self.id
    }

    fn unique_name(&self) -> Option<ScopedName> {
        Some(self.scoped_name())
    }

    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }

    /// An artifact is a user's and lies in a project's scope (issue 467), and has its versions
    /// numbered from 1 without gaps; a version's list names whom an artifact's may (issue 23) and
    /// grants reading alone - reading the version, of those who read the artifact (issue 316).
    fn validate(&self) -> Result<(), String> {
        if self.owner().is_none() {
            return Err("an artifact belongs to a user".to_string());
        }
        if !matches!(self.name.scope, Scope::ProjectScope { .. }) {
            return Err("an artifact lies in a project's scope".to_string());
        }
        for version in &self.versions {
            self.version_attributes(version).validate()?;
            if version.acl.iter().any(|entry| entry.grant != Grant::Read) {
                return Err("a version's list grants reading alone; changing is the artifact's to grant".to_string());
            }
        }
        if self.versions.is_empty() {
            return Err("an artifact has at least one version".to_string());
        }
        if self.versions.iter().zip(1..).any(|(version, number)| version.number != number) {
            return Err("an artifact's versions are numbered from 1 without gaps".to_string());
        }
        Ok(())
    }

    fn classification(&self) -> Option<&Classification> {
        Some(&self.classification)
    }
}

impl ArtifactVersion {
    /// Lets `subject` read this version, unless its list grants it already.
    pub fn grant_reading(&mut self, subject: Subject) {
        grant_reading(&mut self.acl, subject);
    }
}

/// Adds reading by `subject` to `acl`, unless an entry grants it already.
fn grant_reading(acl: &mut Vec<AclEntry>, subject: Subject) {
    if !acl.iter().any(|entry| entry.subject == subject) {
        acl.push(AclEntry { subject, grant: Grant::Read });
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

/// An artifact as the viewer sees it (issue 271): its owner, among their
/// artifacts - with every version -, or a reader of a channel - with the
/// versions published there alone.
#[derive(Debug, Clone)]
pub struct ArtifactSnapshot {
    /// With the versions the viewer may read alone (issue 23).
    pub artifact: Artifact,
    /// Whether the viewer may change it - describe it, share it, delete it (`UpdateArtifact`): its
    /// owner.
    pub can_change: bool,
    /// Whether the viewer may add a version (`WriteArtifact`, issue 23).
    pub can_write: bool,
    /// Its own access list, for whoever may change it - empty for
    /// everybody else.
    pub access: Vec<AccessSnapshot>,
    /// The lists of its versions, by number, for whoever may change it.
    pub version_access: BTreeMap<u32, Vec<AccessSnapshot>>,
    /// Where its versions were published, by number (issue 291) - asked
    /// for where one artifact is read
    /// (`ArtifactView::snapshot_with_publications`): those whose channel the
    /// viewer may read alone.
    pub publications: BTreeMap<u32, PublicationSnapshot>,
}

/// Where a version of an artifact was published and by whom, by name (issue
/// 291), as a reader of its channel knows them.
#[derive(Debug, Clone, PartialEq)]
pub struct PublicationSnapshot {
    pub session_name: String,
    pub channel_name: String,
    /// `None` for a publisher that no longer exists.
    pub published_by: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::AgentId;
    use caelumora_model::digest;
    use caelumora_model::group::GroupId;

    /// A version of `content`, published in a channel of `session_id`.
    fn version(content: &[u8], session_id: SessionId) -> ArtifactVersion {
        ArtifactVersion {
            number: 0,
            size: content.len() as u64,
            digest: digest::content(content),
            media_type: None,
            description: None,
            session_id,
            channel_id: ChannelId::new(),
            published_by: ActorId::User(UserId::new()),
            published_at: Utc::now(),
            acl: Vec::new(),
        }
    }

    fn artifact(scope: Scope, owner: UserId) -> Artifact {
        Artifact::new(ArtifactId::new(), ScopedName::new("report.csv", scope).unwrap(), "files", Classification::CHANNEL_DEFAULT, owner, version(b"a,b", SessionId::new()))
    }

    /// Issue 467: an artifact lies in a project's scope and nowhere else, and belongs to a user.
    #[test]
    fn an_artifact_lies_in_a_project_and_belongs_to_a_user() {
        let alice = UserId::new();
        let session_id = SessionId::new();
        let project_id = ProjectId::new();
        let project = Scope::ProjectScope { project_id };
        assert!(artifact(project.clone(), alice).validate().is_ok());
        assert_eq!(artifact(project.clone(), alice).project(), Some(project_id), "the project it lies in");
        for scope in [Scope::SessionScope { session_id }, Scope::UserScope { user_id: alice }, Scope::GlobalScope, Scope::ChannelScope { session_id, channel_id: ChannelId::new() }] {
            assert!(artifact(scope.clone(), alice).validate().is_err(), "not in {scope:?}");
        }
        let systems = Artifact { security_attributes: SecurityAttributes::owned_by(ActorId::System), ..artifact(project, alice) };
        assert!(systems.validate().is_err(), "nobody's");
    }

    #[test]
    fn versions_are_numbered_from_one_and_kept() {
        let session_id = SessionId::new();
        let mut kept = artifact(Scope::ProjectScope { project_id: ProjectId::new() }, UserId::new());
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
    fn a_version_is_read_by_whom_its_own_list_grants_it_of_the_artifacts_readers() {
        let (alice, bob) = (UserId::new(), UserId::new());
        let session_id = SessionId::new();
        let drafts = GroupId::of_channel(ChannelId::new());
        let team = GroupId::of_channel(ChannelId::new());
        let read_by = |group| AclEntry { subject: Subject::Group(group), grant: Grant::Read };
        let mut shared = artifact(Scope::ProjectScope { project_id: ProjectId::new() }, alice);
        shared.versions[0].acl = vec![read_by(drafts)];
        shared.add_version(ArtifactVersion { acl: vec![read_by(team)], ..version(b"final", session_id) });
        shared.security_attributes = shared.security_attributes.clone().granting(Subject::Actor(ActorId::User(bob)), Grant::Read);
        let bobs = AclEntry { subject: Subject::Actor(ActorId::User(bob)), grant: Grant::Read };
        assert_eq!(shared.version_attributes(&shared.versions[0]).acl, [read_by(drafts)], "the draft: its own list");
        assert_eq!(shared.version_attributes(&shared.versions[1]).acl, [read_by(team)], "the final one: not the drafts' readers, nor the artifact's");
        assert_eq!(shared.version_attributes(&shared.versions[1]).owner_id, ActorId::User(alice), "its owner, the artifact's");
        assert_eq!(shared.security_attributes.acl, [bobs], "the artifact's list is its own");
        shared.grant_reading(Subject::Actor(ActorId::User(bob)));
        shared.grant_reading(Subject::Group(team));
        assert_eq!(shared.security_attributes.acl, [bobs, read_by(team)], "reading granted once, where it was not");
        assert!(shared.validate().is_ok());
        let mut written = shared.clone();
        written.versions[1].acl = vec![AclEntry { grant: Grant::Write, ..read_by(team) }];
        assert!(written.validate().is_err(), "a version's list grants reading alone");
        let mut agent = shared;
        agent.versions[1].acl = vec![AclEntry { subject: Subject::Actor(ActorId::Agent(AgentId::new())), grant: Grant::Read }];
        assert!(agent.validate().is_err(), "and names no agent");
    }

    #[test]
    fn an_artifact_is_written_without_what_it_does_not_have() {
        let made = artifact(Scope::ProjectScope { project_id: ProjectId::new() }, UserId::new());
        let written = serde_json::to_value(&made).unwrap();
        assert!(written.get("description").is_none() && written.get("deleted").is_none(), "{written}");
        assert!(written["versions"][0].get("media_type").is_none());
        let read: Artifact = serde_json::from_value(written).unwrap();
        assert_eq!((read.id, read.name, read.versions), (made.id, made.name, made.versions));
    }
}
