//! A group (issues 286, 304): a list of actors that an access list names as
//! one subject (`Subject::Group`), so that what is shared with a team, or
//! with whoever takes part in a session or a channel, follows who belongs to
//! it without a copy of its members in every list.
//!
//! There are two kinds, told apart by where a group lies:
//!
//! - a **team**, which a user makes and keeps: users only, in the global
//!   scope or its owner's;
//! - the **participants** of a session or a channel: users and agents, kept
//!   by that session or channel alone (`ChannelController`, the
//!   `SessionManager`), lying in its scope and gone with it. It has the id of
//!   what it is the participants of (`GroupId::of_session`,
//!   `GroupId::of_channel`) - a one-to-one relation by a shared key -, so
//!   that whoever names it needs nothing but the session's or channel's id.
//!
//! A group is read by its owner and by its members: its own access list
//! names the group itself (`Group::new`). Groups hold no groups.

use crate::actor::ActorId;
use crate::channel::{Channel, Participant};
use crate::removal::{Deletable, NoEntity, Removal, Stored};
use crate::scope::{Scope, Scoped, ScopedField};
use crate::security::{Grant, Secured, SecurityAttributes, Subject};
use crate::session::{ChannelId, Session, SessionId};
use crate::store::query::{Field, FieldValue, Filter};
use crate::user::UserId;
use crate::value::name::{Name, ScopedName};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::store::EntityKind;

define_id!(GroupId);

impl GroupId {
    /// The group of the participants of session `session`: the session's
    /// own id.
    pub fn of_session(session: SessionId) -> Self {
        Self(session.0)
    }

    /// The group of the participants of channel `channel`: the channel's
    /// own id.
    pub fn of_channel(channel: ChannelId) -> Self {
        Self(channel.0)
    }
}

/// What a query of groups may name (issue 308). `Member` is an element of
/// `Members`, the whole of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupField {
    /// Where it lies: a `Scope`.
    Scope,
    /// Who owns it: an `ActorId`.
    Owner,
    /// Who belongs to it.
    Members,
    /// A member, within `Members`: an `ActorId`.
    Member,
}

impl ScopedField for GroupField {
    const SCOPE: Self = GroupField::Scope;
}

impl Field for GroupField {
    fn path(self) -> &'static [&'static str] {
        match self {
            GroupField::Scope => &["scope"],
            GroupField::Owner => &["security_attributes", "owner_id"],
            GroupField::Members => &["members"],
            GroupField::Member => &[],
        }
    }
}

/// A change of a group, as it was asked (issue 248).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GroupUpdateEvent {
    /// Renamed.
    NameChanged(String),
    /// Someone joined or left it; who belongs to it is told only to who may
    /// read it.
    MembersChanged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: GroupId,
    /// What it is found by and where it lies (issue 250): a team in the
    /// global scope or its owner's, the participants of a session or a
    /// channel in that session's or channel's scope.
    #[serde(flatten)]
    pub name: ScopedName,
    /// Who belongs to it: users, and for the participants of a session or
    /// channel its agents too.
    pub members: Vec<ActorId>,
    /// Its owner - the user who made a team, the owner of the session or
    /// channel whose participants it is - and an access list that names the
    /// group itself, so that its members read it.
    pub security_attributes: SecurityAttributes,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Set when it was deleted (issue 164).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
}

impl Group {
    /// The name of every group of participants, in its session's or
    /// channel's scope.
    pub const PARTICIPANTS: &'static str = "participants";

    /// A new team of `owner`, of `name` where it lies, with nobody in it
    /// yet; read by its owner and its members.
    pub fn team(name: ScopedName, owner: UserId) -> Self {
        Self::new(GroupId::new(), name, ActorId::User(owner), Vec::new())
    }

    /// The group of the participants of `session`, as they are now: its
    /// users and agents, owned by the session's owner.
    pub fn of_session(session: &Session) -> Self {
        let name = ScopedName { name: Self::participants_name(), scope: Scope::SessionScope { session_id: session.id } };
        Self::new(GroupId::of_session(session.id), name, session.security_attributes.owner_id, Self::members_among(&session.participants))
    }

    /// The group of the participants of `channel`, as they are now: its
    /// users and agents - not its approver -, owned by the channel's owner.
    pub fn of_channel(channel: &Channel) -> Self {
        let name = ScopedName { name: Self::participants_name(), scope: Scope::ChannelScope { session_id: channel.session_id, channel_id: channel.id } };
        Self::new(GroupId::of_channel(channel.id), name, channel.security_attributes.owner_id, Self::members_among(&channel.participants))
    }

    fn new(id: GroupId, name: ScopedName, owner: ActorId, members: Vec<ActorId>) -> Self {
        let now = Utc::now();
        Self {
            id,
            name,
            members,
            security_attributes: SecurityAttributes::owned_by(owner).granting(Subject::Group(id), Grant::Read),
            created_at: now,
            updated_at: now,
            deleted: None,
        }
    }

    fn participants_name() -> Name {
        Name::fixed(Self::PARTICIPANTS)
    }

    /// The members among `participants`: users and agents, not approvers.
    pub fn members_among(participants: &[Participant]) -> Vec<ActorId> {
        participants.iter()
            .filter(|p| matches!(p, Participant::User(_) | Participant::Agent(_)))
            .map(Participant::actor_id)
            .collect()
    }

    /// Whether it is the participants of a session or a channel, kept by
    /// that alone - not a team anyone changes by hand.
    pub fn is_participants(&self) -> bool {
        matches!(self.name.scope, Scope::SessionScope { .. } | Scope::ChannelScope { .. })
    }

    /// The teams, as a query (issue 317): the groups that lie where a team
    /// lies - the global scope or a user's -, not the participants of a
    /// session or channel (`is_participants`).
    pub fn teams() -> Filter<GroupField> {
        Filter::is_in(GroupField::Scope, [FieldValue::variant("GlobalScope"), FieldValue::variant("UserScope")])
    }

    /// Whether `actor` belongs to it.
    pub fn has_member(&self, actor: ActorId) -> bool {
        self.members.contains(&actor)
    }
}

impl Stored for Group {
    const KIND: EntityKind = EntityKind::new("group");
    type Id = GroupId;
    type Within = NoEntity;
    type Field = GroupField;

    fn id(&self) -> GroupId {
        self.id
    }

    fn unique_name(&self) -> Option<ScopedName> {
        Some(self.scoped_name())
    }

    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    /// The participants of a session or channel lie in its scope, with its
    /// id, and hold users and agents; a team belongs to a user, lies in the
    /// global scope or its owner's, and holds users alone (issue 255: an
    /// agent is reached through its owner). No group holds the system or
    /// nobody.
    fn validate(&self) -> Result<(), String> {
        if self.members.iter().any(|m| !matches!(m, ActorId::User(_) | ActorId::Agent(_))) {
            return Err("a group holds users and agents alone".to_string());
        }
        let owner = self.security_attributes.owner_id;
        match &self.name.scope {
            Scope::SessionScope { session_id } if self.id == GroupId::of_session(*session_id) => Ok(()),
            Scope::ChannelScope { channel_id, .. } if self.id == GroupId::of_channel(*channel_id) => Ok(()),
            Scope::SessionScope { .. } | Scope::ChannelScope { .. } => Err("a group in a session's or channel's scope is its participants, with its id".to_string()),
            _ if !matches!(owner, ActorId::User(_)) => Err("a team belongs to a user".to_string()),
            _ if self.members.iter().any(|m| matches!(m, ActorId::Agent(_))) => Err("a team holds users alone".to_string()),
            Scope::GlobalScope => Ok(()),
            Scope::UserScope { user_id } if ActorId::User(*user_id) == owner => Ok(()),
            Scope::UserScope { .. } => Err("a team lies in no other user's scope than its owner's".to_string()),
            Scope::GroupScope { .. } => Err("a team lies in the global scope or its owner's".to_string()),
        }
    }
}

impl Deletable for Group {
    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }
}

impl Secured for Group {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl Scoped for Group {
    fn scoped_name(&self) -> ScopedName {
        self.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentId;

    fn team(scope: Scope, owner: UserId) -> Group {
        Group::team(ScopedName::new("project", scope).unwrap(), owner)
    }

    #[test]
    fn a_team_lies_in_the_global_scope_or_its_owners_and_holds_users() {
        let alice = UserId::new();
        for scope in [Scope::GlobalScope, Scope::UserScope { user_id: alice }] {
            assert!(team(scope.clone(), alice).validate().is_ok(), "{scope:?}");
        }
        assert!(team(Scope::UserScope { user_id: UserId::new() }, alice).validate().is_err(), "another's scope");
        assert!(team(Scope::SessionScope { session_id: SessionId::new() }, alice).validate().is_err(), "a session's scope is its participants'");
        let mut with_agent = team(Scope::GlobalScope, alice);
        with_agent.members.push(ActorId::Agent(AgentId::new()));
        assert!(with_agent.validate().is_err(), "an agent is reached through its owner");
        let mut with_system = team(Scope::GlobalScope, alice);
        with_system.members.push(ActorId::System);
        assert!(with_system.validate().is_err());
    }

    /// The query for teams (issue 317) finds exactly the groups that are
    /// not the participants of a session or channel.
    #[test]
    fn the_query_for_teams_finds_what_is_no_group_of_participants() {
        let alice = UserId::new();
        let session = SessionId::new();
        let channel = Channel::build(session, ActorId::User(alice), Name::new("main").unwrap(), Default::default(), Default::default(), None, vec![Participant::User(alice)]);
        let session_group = Group { id: GroupId::of_session(session), name: ScopedName { name: Group::participants_name(), scope: Scope::SessionScope { session_id: session } }, ..team(Scope::GlobalScope, alice) };
        for group in [team(Scope::GlobalScope, alice), team(Scope::UserScope { user_id: alice }, alice), Group::of_channel(&channel), session_group] {
            let document = serde_json::to_value(&group).unwrap();
            assert_eq!(Group::teams().matches(&document), !group.is_participants(), "{:?}", group.name.scope);
        }
    }

    #[test]
    fn the_name_of_participants_is_a_name() {
        assert!(Name::new(Group::PARTICIPANTS).is_ok());
    }

    #[test]
    fn a_team_is_read_by_its_members() {
        let group = team(Scope::GlobalScope, UserId::new());
        assert!(group.security_attributes.acl.iter().any(|entry| entry.subject == Subject::Group(group.id) && entry.grant == Grant::Read));
    }

    #[test]
    fn the_participants_of_a_channel_have_its_id_and_hold_its_users_and_agents() {
        let (alice, agent) = (UserId::new(), AgentId::new());
        let channel = Channel::build(SessionId::new(), ActorId::User(alice), Name::new("main").unwrap(), Default::default(), Default::default(), None,
            vec![Participant::User(alice), Participant::Agent(agent)]);
        let group = Group::of_channel(&channel);
        assert_eq!(group.id, GroupId::of_channel(channel.id));
        assert_eq!(group.members, [ActorId::User(alice), ActorId::Agent(agent)], "not the approver");
        assert!(group.is_participants());
        assert!(group.validate().is_ok());
        let stray = Group { id: GroupId::new(), ..group };
        assert!(stray.validate().is_err(), "only with the channel's id");
    }
}
