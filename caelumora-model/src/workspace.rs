//! A workspace (issue 253): a tree of files that belongs to a user, is
//! attached to sessions and is shared by the people and agents of a
//! session. One classification holds for the whole tree (issue 169): its
//! files carry no label of their own. Where the files lie is the storage the
//! workspace names - configured, not stored - and its files are no part of
//! the document: the place is the record.

use crate::actor::ActorId;
use crate::confidentiality::Classification;
use crate::removal::Removal;
use crate::scope::{Scope, Scoped};
use crate::security::{Secured, SecurityAttributes};
use crate::user::UserId;
use crate::value::name::{Name, ScopedName};
use crate::value::text::Description;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
    /// Who may use it besides its owner, as it now stands.
    ContributorsChanged(Vec<ActorId>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    /// What it is called, unique among its owner's workspaces (issue 250):
    /// it lies in its owner's scope, derived from the owner as a channel's
    /// is from its session, so that the two cannot disagree.
    pub name: Name,
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
    /// Owned by the user it was made for; its contributors may use it -
    /// read and write its files, attach it to a session - but not change
    /// it.
    pub security_attributes: SecurityAttributes,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Set when it was deleted (issue 164): it stays stored, its files kept,
    /// hidden from every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
}


impl Workspace {
    /// A new workspace of `owner`, in `storage`, classified
    /// `classification`, without contributors or a description.
    pub fn new(name: Name, storage: impl Into<String>, classification: Classification, owner: UserId) -> Self {
        let now = Utc::now();
        Self {
            id: WorkspaceId::new(),
            name,
            storage: storage.into(),
            description: None,
            classification,
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
}

impl crate::removal::Stored for Workspace {
    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    /// A workspace is someone's: the system's would lie in no user's scope.
    fn validate(&self) -> Result<(), String> {
        match self.owner() {
            Some(_) => Ok(()),
            None => Err("a workspace belongs to a user".to_string()),
        }
    }
}

impl crate::removal::Deletable for Workspace {
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
        let scope = self.owner().map_or(Scope::GlobalScope, |user_id| Scope::UserScope { user_id });
        ScopedName { name: self.name.clone(), scope }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::removal::Stored;

    #[test]
    fn a_workspace_lies_in_its_owners_scope_and_belongs_to_a_user() {
        let alice = UserId::new();
        let workspace = Workspace::new(Name::new("Docs").unwrap(), "main", Classification::CHANNEL_DEFAULT, alice);
        assert_eq!(workspace.scoped_name().scope, Scope::UserScope { user_id: alice }, "derived from the owner");
        assert_eq!(workspace.owner(), Some(alice));
        assert!(workspace.validate().is_ok());

        let systems = Workspace { security_attributes: SecurityAttributes::owned_by(ActorId::System), ..workspace };
        assert!(systems.validate().is_err(), "nobody's is not stored");
    }

    #[test]
    fn a_workspace_is_written_without_what_it_does_not_have() {
        let workspace = Workspace::new(Name::new("Docs").unwrap(), "main", Classification::CHANNEL_DEFAULT, UserId::new());
        let written = serde_json::to_value(&workspace).unwrap();
        assert!(written.get("description").is_none() && written.get("deleted").is_none(), "{written}");
        let read: Workspace = serde_json::from_value(written).unwrap();
        assert_eq!((read.id, read.name, read.storage), (workspace.id, workspace.name, workspace.storage));
    }
}
