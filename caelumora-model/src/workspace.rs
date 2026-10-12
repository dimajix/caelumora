//! A workspace (issue 253): a tree of files that belongs to a user and is
//! attached to sessions, where the people and agents of the channels that
//! may use it share it (issue 251, decision 10). One classification holds for the whole tree (issue 169): its
//! files carry no label of their own. Where the files lie is the storage the
//! workspace names - configured, not stored - and its files are no part of
//! the document: the place is the record.

use crate::actor::ActorId;
use crate::confidentiality::Classification;
use crate::removal::{Deletable, Removal, Stored};
use crate::scope::{Scope, ScopeField, Scoped};
use crate::group::GroupId;
use crate::security::{Grant, Secured, SecurityAttributes, Subject};
use crate::user::UserId;
use crate::value::name::ScopedName;
use crate::value::text::Description;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::store::EntityKind;

define_id!(WorkspaceId);

/// A change of a workspace, as it was asked: what it says is made into the
/// workspace's values as it is applied (issue 248).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkspaceUpdateEvent {
    /// Renamed.
    NameChanged(String),
    /// What it holds, as its owner describes it, as it now stands; `None`
    /// takes the description away.
    DescriptionChanged(Option<String>),
    /// Reclassified as a whole: lowering releases every file in it, raising
    /// retracts nothing already read.
    ClassificationChanged(Classification),
    /// Its access list changed (issue 23). Without the list: only its owner
    /// sees it, while everybody who may read the workspace hears of this.
    AccessChanged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    /// What it is found by and where it lies (issue 251, decision 13): a
    /// session's scope - made with the session and gone with it -, its
    /// owner's - reused across their sessions - or the global one; its
    /// name unique there (issue 250). The scope says where it is found,
    /// not whose it is: it is its owner's wherever it lies.
    #[serde(flatten)]
    pub name: ScopedName,
    /// The name of the storage its files lie in, as the configuration names
    /// it; fixed when the workspace is made. No path: where the storage is
    /// is configuration, and the workspace's place in it is its id.
    pub storage: String,
    /// What it holds, as its owner describes it, for the people and agents
    /// who use it; `None` if nobody said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Description>,
    /// How everything in it is classified (issue 169): one container, whose
    /// files carry no label of their own.
    pub classification: Classification,
    /// Owned by the user it was made for (issue 251, decision 12), and
    /// reached by whom its access list grants it (issue 23): one made for a
    /// session is read and written by the session's members; one of its
    /// owner's grants nobody anything until they share it.
    pub security_attributes: SecurityAttributes,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Set when it was deleted (issue 164): it stays stored, its files kept,
    /// hidden from every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
}


impl Workspace {
    /// A new workspace of `owner`, of `name` where it lies, in `storage`,
    /// classified `classification`, without a description. One that lies
    /// in a session's scope is the session's to work on: its members read
    /// and write it (issue 23) - the group of the session's participants
    /// (issue 304); any other grants nobody anything.
    pub fn new(name: ScopedName, storage: impl Into<String>, classification: Classification, owner: UserId) -> Self {
        let now = Utc::now();
        let owned = SecurityAttributes::owned_by(ActorId::User(owner));
        let security_attributes = match name.scope {
            Scope::SessionScope { session_id } => owned.granting(Subject::Group(GroupId::of_session(session_id)), Grant::Write),
            _ => owned,
        };
        Self {
            id: WorkspaceId::new(),
            name,
            storage: storage.into(),
            description: None,
            classification,
            security_attributes,
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
}

impl Stored for Workspace {
    const KIND: EntityKind = EntityKind::new("workspace");
    type Id = WorkspaceId;
    type Field = ScopeField;

    fn id(&self) -> WorkspaceId {
        self.id
    }

    fn unique_name(&self) -> Option<ScopedName> {
        Some(self.scoped_name())
    }

    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    /// A workspace is a user's - the system's everybody could use - and
    /// lies in a session's scope, its owner's or the global one: not in a
    /// channel's, since a session's channels share one.
    fn validate(&self) -> Result<(), String> {
        crate::scope::validate_users_own("workspace", &self.security_attributes, &self.name.scope)
    }

    fn classification(&self) -> Option<&Classification> {
        Some(&self.classification)
    }
}

impl Deletable for Workspace {
    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }
}

impl Secured for Workspace {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl Scoped for Workspace {
    fn scoped_name(&self) -> ScopedName {
        self.name.clone()
    }
}

/// The workspace a session is given as it is made (issue 266), in place of
/// the one its template names.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkspaceChoice {
    /// None, whatever the template names.
    None,
    /// One of its maker's own, attached as to a running session - theirs to
    /// share and of no other session's scope -, shared with the session's
    /// members as far as `share` says, if it says anything (issue 23).
    Existing { id: WorkspaceId, share: Option<crate::security::Grant> },
    /// One made for the session alone, in its scope and named as it is, as
    /// a template's of the session's scope is made.
    New { storage: String, classification: Classification },
}

#[cfg(test)]
mod tests {
    use super::*;
    use Stored;
    use crate::scope::Scope;
    use crate::security::AclEntry;
    use crate::session::{ChannelId, SessionId};

    fn named(scope: Scope) -> ScopedName {
        ScopedName::new("Docs", scope).unwrap()
    }

    #[test]
    fn a_workspace_lies_in_a_session_s_scope_its_owner_s_or_the_global_one() {
        let alice = UserId::new();
        let session_id = SessionId::new();
        for scope in [Scope::SessionScope { session_id }, Scope::UserScope { user_id: alice }, Scope::GlobalScope] {
            let workspace = Workspace::new(named(scope.clone()), "main", Classification::CHANNEL_DEFAULT, alice);
            assert_eq!(workspace.scoped_name().scope, scope);
            assert!(workspace.validate().is_ok(), "{scope:?}");
        }
        for scope in [Scope::UserScope { user_id: UserId::new() }, Scope::ChannelScope { session_id, channel_id: ChannelId::new() }, Scope::GroupScope { group_id: GroupId::new() }] {
            assert!(Workspace::new(named(scope.clone()), "main", Classification::CHANNEL_DEFAULT, alice).validate().is_err(), "not in {scope:?}");
        }
    }

    #[test]
    fn a_workspace_belongs_to_a_user_and_a_sessions_is_its_members_to_work_on() {
        let alice = UserId::new();
        let workspace = Workspace::new(named(Scope::GlobalScope), "main", Classification::CHANNEL_DEFAULT, alice);
        assert_eq!(workspace.owner(), Some(alice));
        assert!(workspace.security_attributes.acl.is_empty(), "one of its owner's grants nobody anything until shared");
        let session_id = SessionId::new();
        let sessions = Workspace::new(named(Scope::SessionScope { session_id }), "main", Classification::CHANNEL_DEFAULT, alice);
        assert_eq!(sessions.security_attributes.acl, [AclEntry { subject: Subject::Group(GroupId::of_session(session_id)), grant: Grant::Write }], "one made for a session, the group of its participants");
        let shared = Workspace { security_attributes: workspace.security_attributes.clone().granting(Subject::Actor(ActorId::User(UserId::new())), Grant::Read), ..workspace.clone() };
        assert!(shared.validate().is_ok(), "shared with someone");
        let systems = Workspace { security_attributes: SecurityAttributes::owned_by(ActorId::System), ..workspace };
        assert!(systems.validate().is_err(), "nor nobody's");
    }

    #[test]
    fn a_workspace_is_written_without_what_it_does_not_have() {
        let workspace = Workspace::new(named(Scope::GlobalScope), "main", Classification::CHANNEL_DEFAULT, UserId::new());
        let written = serde_json::to_value(&workspace).unwrap();
        assert!(written.get("description").is_none() && written.get("deleted").is_none(), "{written}");
        let read: Workspace = serde_json::from_value(written).unwrap();
        assert_eq!((read.id, read.name, read.storage), (workspace.id, workspace.name, workspace.storage));
    }
}
