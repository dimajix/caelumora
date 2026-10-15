use crate::actor::ActorId;
use crate::group::GroupId;
use crate::role::RoleId;
use crate::store::query::FieldValue;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

/// Who an entity belongs to and who else may reach it - what a policy
/// decides by (issues 11, 23).
///
/// The owner may do everything with it, implicitly and unrevocably; the
/// access list only adds - allow-only, the union of its entries, without
/// precedence. An entity without an owner (`ActorId::None`, as in the demo
/// data of a seed file) belongs to the system.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SecurityAttributes {
    pub owner_id: ActorId,
    /// May be left out where the attributes are written by hand - a seed
    /// file (issue 108).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acl: Vec<AclEntry>,
}

impl SecurityAttributes {
    /// Attributes of an entity owned by `owner`, with an empty access list.
    pub fn owned_by(owner: ActorId) -> Self {
        Self { owner_id: owner, acl: Vec::new() }
    }

    /// These attributes, granting `subject` what `grant` covers besides.
    pub fn granting(mut self, subject: Subject, grant: Grant) -> Self {
        self.acl.push(AclEntry { subject, grant });
        self
    }

    /// Nothing if the attributes may be stored as they are: an entry names
    /// a person, never an agent - an agent acts with its owner's rights
    /// (issue 255), and an entry for it would let it reach what the person
    /// responsible for it may not - nor the system or nobody.
    pub fn validate(&self) -> Result<(), String> {
        for entry in &self.acl {
            if let Subject::Actor(actor) = entry.subject {
                if !matches!(actor, ActorId::User(_)) {
                    return Err(format!("an access list names users, roles and groups, not {actor}"));
                }
            }
        }
        Ok(())
    }
}

/// One entry of an access list (issue 23): whom it grants what.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AclEntry {
    pub subject: Subject,
    pub grant: Grant,
}

/// Whom an entry of an access list grants something (issue 23). Matched
/// against the context's principal - an agent's owner -, and for what is
/// read also against its caller, an agent for what it takes part in itself
/// (issue 264).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subject {
    /// A user.
    Actor(ActorId),
    /// Whoever holds the role - an agent its owner's roles, narrowed by its
    /// personality's limiting roles.
    Role(RoleId),
    /// The members of a group (issues 286, 304) - a team, or the current
    /// participants of a session or a channel -, as the policy learns from
    /// its `Memberships`.
    Group(GroupId),
    /// Every user (issue 221) - an agent by its owner -, as for a session
    /// or channel anybody may join.
    Everyone,
}

/// A subject as a query compares it, after serde's tags:
/// `{"actor": {"User": "…"}}`, `{"role": "…"}`.
impl From<Subject> for FieldValue {
    fn from(subject: Subject) -> Self {
        match subject {
            Subject::Actor(actor) => FieldValue::tagged("actor", actor),
            Subject::Role(role) => FieldValue::tagged("role", role),
            Subject::Group(group) => FieldValue::tagged("group", group),
            Subject::Everyone => FieldValue::Text("everyone".to_string()),
        }
    }
}

/// What an entry of an access list grants (issue 23). What is none of
/// them - changing the attributes, deleting, reclassifying - is the
/// owner's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grant {
    /// Getting, listing, downloading, using.
    Read,
    /// Changing the content, besides reading it: a workspace's files, an
    /// artifact's versions.
    Write,
    /// Taking oneself into a session or channel (issue 221, `JOIN`) - and
    /// nothing else: who may join reads nothing of it before they did, and
    /// who reads it may not join by that. A personal entry is an invitation,
    /// used up as it is taken.
    Join,
}

impl Grant {
    /// Whether this grant covers what `needed` asks for: each itself, and
    /// writing reading.
    pub fn covers(self, needed: Grant) -> bool {
        self == needed || (self == Grant::Write && needed == Grant::Read)
    }
}

/// What a policy needs to know of actors that the attributes it is handed
/// do not say (issue 23): who belongs to a group an access list names
/// (issue 286). A fact, not a decision
/// - the decision stays the policy's. Implemented over the stores, read in
/// the system's name: one group at a time, never what an actor is a member
/// of.
#[async_trait]
pub trait Memberships: Debug + Send + Sync {
    /// The members of `group` - also of one that was deleted, whose members
    /// are told of the deletion; none of one that does not exist.
    async fn members_of(&self, group: GroupId) -> Vec<ActorId>;
}

/// Knows of no membership: for a policy without access lists
/// naming them - tests, and what is built before a store is.
#[derive(Debug, Default)]
pub struct NoMemberships;

#[async_trait]
impl Memberships for NoMemberships {
    async fn members_of(&self, _group: GroupId) -> Vec<ActorId> {
        Vec::new()
    }
}

/// An entity that carries `SecurityAttributes`.
pub trait Secured {
    fn security_attributes(&self) -> &SecurityAttributes;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::testing::*;

    #[test]
    fn an_access_list_names_people_roles_and_groups_never_an_agent() {
        let alice = user();
        let agent = ActorId::Agent(crate::agent::AgentId::new());
        assert_eq!(shared(alice, &[user()]).validate(), Ok(()));
        let place = Subject::Group(GroupId::new());
        assert_eq!(SecurityAttributes::owned_by(alice).granting(Subject::Role(RoleId::new()), Grant::Read).granting(place, Grant::Write).validate(), Ok(()));
        for nobody in [agent, ActorId::System, ActorId::None] {
            assert!(SecurityAttributes::owned_by(alice).granting(Subject::Actor(nobody), Grant::Read).validate().is_err(), "not {nobody}");
        }
        let written: SecurityAttributes = serde_json::from_value(serde_json::json!({"owner_id": alice})).unwrap();
        assert!(written.acl.is_empty(), "the list may be left out where it is written by hand");
    }

    /// A subject as a query compares it (issue 293).
    #[test]
    fn a_subject_meets_exactly_the_subjects_equal_to_it() {
        let id = uuid::Uuid::new_v4();
        crate::store::query::meets_exactly_its_equals(&[
            Subject::Actor(ActorId::User(crate::user::UserId(id))), Subject::Actor(ActorId::Agent(crate::agent::AgentId(id))),
            Subject::Actor(ActorId::System), Subject::Role(RoleId(id)), Subject::Role(RoleId::new()),
            Subject::Group(GroupId(id)), Subject::Group(GroupId::new()), Subject::Everyone,
        ]);
    }
}
