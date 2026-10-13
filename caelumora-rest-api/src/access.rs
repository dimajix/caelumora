//! Access lists as they are written and shown (issue 23): whom an entry grants what.

use caelumora_model::snapshot;
use caelumora_model::actor::ActorId;
use caelumora_model::group::GroupId;
use caelumora_model::role::RoleId;
use caelumora_model::scope::Scope;
use caelumora_model::security::{AclEntry, Grant, Subject};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::user::UserId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An entry of an access list (issue 23) as it is written and shown: whom
/// it grants `read` or `write` - a user, a role, or the members of a
/// session or a channel, by `kind` and `id` -, and how whom is called where
/// it is shown and known.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccessEntry {
    pub kind: AccessKind,
    pub id: Uuid,
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
    /// A team (issue 286), by its id.
    Group,
}

impl AccessEntry {
    /// The entry as it is shown; `None` for one that names no user, role
    /// or place, which no list stores.
    pub fn of(entry: snapshot::AccessSnapshot) -> Option<Self> {
        let (kind, id) = match entry.subject {
            Subject::Actor(ActorId::User(user)) => (AccessKind::User, user.0),
            Subject::Actor(_) => return None,
            Subject::Role(role) => (AccessKind::Role, role.0),
            // The participants of a session or channel are named by it: their
            // group has its id (issue 286).
            Subject::Group(group) => match entry.scope {
                Some(Scope::SessionScope { .. }) => (AccessKind::Session, group.0),
                Some(Scope::ChannelScope { .. }) => (AccessKind::Channel, group.0),
                _ => (AccessKind::Group, group.0),
            },
        };
        Some(Self { kind, id, grant: entry.grant, name: entry.name })
    }

    /// The entry as it is stored; its name, if written, is ignored.
    pub fn into_acl(self) -> AclEntry {
        let subject = match self.kind {
            AccessKind::User => Subject::Actor(ActorId::User(UserId(self.id))),
            AccessKind::Role => Subject::Role(RoleId(self.id)),
            AccessKind::Session => Subject::Group(GroupId::of_session(SessionId(self.id))),
            AccessKind::Channel => Subject::Group(GroupId::of_channel(ChannelId(self.id))),
            AccessKind::Group => Subject::Group(GroupId(self.id)),
        };
        AclEntry { subject, grant: self.grant }
    }
}
