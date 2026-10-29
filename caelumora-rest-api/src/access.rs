//! Access lists as they are written and shown (issue 23): whom an entry grants what.

use caelumora_model::actor::ActorId;
use caelumora_model::group::GroupId;
use caelumora_model::project::ProjectId;
use caelumora_model::role::RoleId;
use caelumora_model::scope::Scope;
use caelumora_model::security::{AclEntry, Grant, Subject};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::snapshot;
use caelumora_model::user::UserId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An entry of an access list (issue 23) as it is written and shown: whom
/// it grants `read`, `write` or `join` - a user, a role, the participants of a
/// session or a channel, or the members of a project (issue 467), by `kind`
/// and `id`, or everyone, without an id (issue 221) -, and how whom is called
/// where it is shown and known.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccessEntry {
    pub kind: AccessKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,
    pub grant: Grant,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccessKind {
    User,
    Role,
    /// The participants of a session, by the session's id.
    Session,
    /// The participants of a channel, by the channel's id.
    Channel,
    /// The members of a project (issue 467), by the project's id.
    Project,
    /// A team (issue 286), by its id.
    Group,
    /// Every user (issue 221), without an id.
    Everyone,
}

impl AccessEntry {
    /// The entry as it is shown; `None` for one that names no user, role
    /// or place, which no list stores.
    pub fn of(entry: snapshot::AccessSnapshot) -> Option<Self> {
        let (kind, id) = match entry.subject {
            Subject::Actor(ActorId::User(user)) => (AccessKind::User, Some(user.0)),
            Subject::Actor(_) => return None,
            Subject::Role(role) => (AccessKind::Role, Some(role.0)),
            // The participants of a session or channel, and the members of a
            // project, are named by it: their group has its id (issues 286,
            // 467).
            Subject::Group(group) => match entry.scope {
                Some(Scope::SessionScope { .. }) => (AccessKind::Session, Some(group.0)),
                Some(Scope::ChannelScope { .. }) => (AccessKind::Channel, Some(group.0)),
                Some(Scope::ProjectScope { .. }) => (AccessKind::Project, Some(group.0)),
                Some(Scope::UserScope { .. } | Scope::GlobalScope | Scope::GroupScope { .. })
                | None => (AccessKind::Group, Some(group.0)),
            },
            Subject::Everyone => (AccessKind::Everyone, None),
        };
        Some(Self { kind, id, grant: entry.grant, name: entry.name })
    }

    /// The entry as it is stored; its name, if written, is ignored. `None`
    /// for one whose kind needs an id it does not have.
    pub fn into_acl(self) -> Option<AclEntry> {
        let subject = match (self.kind, self.id) {
            (AccessKind::Everyone, _) => Subject::Everyone,
            (_, None) => return None,
            (AccessKind::User, Some(id)) => Subject::Actor(ActorId::User(UserId(id))),
            (AccessKind::Role, Some(id)) => Subject::Role(RoleId(id)),
            (AccessKind::Session, Some(id)) => Subject::Group(GroupId::of_session(SessionId(id))),
            (AccessKind::Channel, Some(id)) => Subject::Group(GroupId::of_channel(ChannelId(id))),
            (AccessKind::Project, Some(id)) => Subject::Group(GroupId::of_project(ProjectId(id))),
            (AccessKind::Group, Some(id)) => Subject::Group(GroupId(id)),
        };
        Some(AclEntry { subject, grant: self.grant })
    }

    /// `entries` as they are stored.
    ///
    /// # Errors
    /// What is wrong with the first that names nobody.
    pub fn acl_of(entries: Vec<AccessEntry>) -> Result<Vec<AclEntry>, String> {
        entries
            .into_iter()
            .map(|entry| {
                let kind = entry.kind;
                entry.into_acl().ok_or_else(|| format!("an entry of kind {kind:?} needs an id"))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::project::ProjectId;

    /// Issue 467: the members of a project are named by the project, as the participants of a
    /// session are by the session.
    #[test]
    fn the_members_of_a_project_are_named_by_it() {
        let project_id = ProjectId::new();
        let group = GroupId::of_project(project_id);
        let shown = AccessEntry::of(snapshot::AccessSnapshot {
            subject: Subject::Group(group),
            grant: Grant::Read,
            name: None,
            scope: Some(Scope::ProjectScope { project_id }),
        });
        assert_eq!(
            shown.as_ref().map(|entry| (entry.kind, entry.id)),
            Some((AccessKind::Project, Some(project_id.0)))
        );
        assert_eq!(
            shown.and_then(AccessEntry::into_acl),
            Some(AclEntry { subject: Subject::Group(group), grant: Grant::Read }),
            "stored as it was"
        );
    }
}
