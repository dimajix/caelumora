//! What an entity carries of who may reach it (issues 11, 23): its owner
//! and its access list, and what a policy learns besides of the groups the
//! list names.

use crate::actor::ActorId;
use crate::group::{Group, GroupId};
use crate::role::RoleId;
use crate::store::query::FieldValue;
use crate::user::UserId;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

/// Who an entity belongs to and who else may reach it - what a policy
/// decides by (issues 11, 23).
///
/// The owner may do everything with it, implicitly and unrevocably; the
/// access list only adds - allow-only, the union of its entries, without
/// precedence. Every entity has an owner (issue 491): what a seed file
/// leaves without attributes is the system's (`systems`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecurityAttributes {
    pub owner_id: ActorId,
    /// May be left out where the attributes are written by hand - a seed
    /// file (issue 108).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acl: Vec<AclEntry>,
}

impl SecurityAttributes {
    /// Attributes of an entity owned by `owner`, with an empty access list.
    pub const fn owned_by(owner: ActorId) -> Self {
        Self { owner_id: owner, acl: Vec::new() }
    }

    /// Attributes of what the system owns, with an empty access list: what a
    /// seed file leaves out.
    pub const fn systems() -> Self {
        Self::owned_by(ActorId::System)
    }

    /// These attributes, granting `subject` what `grant` covers besides.
    pub fn granting(mut self, subject: Subject, grant: Grant) -> Self {
        self.acl.push(AclEntry { subject, grant });
        self
    }

    /// Nothing if the attributes may be stored as they are: the owner is
    /// someone who answers for the entity - a user, an agent or the system,
    /// never nobody or an approver (issue 491) -, and an entry names a
    /// person, never an agent - an agent acts with its owner's rights (issue
    /// 255), and an entry for it would let it reach what the person
    /// responsible for it may not - nor the system or nobody.
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.owner_id, ActorId::User(_) | ActorId::Agent(_) | ActorId::System) {
            return Err(format!(
                "an entity is owned by a user, an agent or the system, not {}",
                self.owner_id
            ));
        }
        for entry in &self.acl {
            if let Subject::Actor(actor) = entry.subject {
                if !matches!(actor, ActorId::User(_)) {
                    return Err(format!(
                        "an access list names users, roles and groups, not {actor}"
                    ));
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
    /// its `Memberships`, while the group is there; once it is deleted, for
    /// being told of a deletion alone (`Verb::HearDeleted`).
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
    /// Every grant there is.
    pub const ALL: [Grant; 3] = [Grant::Read, Grant::Write, Grant::Join];

    /// Whether this grant covers what `needed` asks for: each itself, and
    /// writing reading.
    pub fn covers(self, needed: Grant) -> bool {
        self == needed || (self == Grant::Write && needed == Grant::Read)
    }

    /// The grants that cover `needed` (`covers`): what an entry must grant,
    /// as a database that keeps the entries asks for them (issue 332).
    pub fn covering(needed: Grant) -> impl Iterator<Item = Grant> {
        Grant::ALL.into_iter().filter(move |grant| grant.covers(needed))
    }
}

/// What a policy needs to know of actors that the attributes it is handed
/// do not say (issue 23): the group an access list names (issue 286), with
/// who belongs to it and whether it was deleted. A fact, not a decision -
/// whether a deleted group still admits is the policy's (`Admission`).
/// Implemented over the stores, read in the system's name: one group at a
/// time, never what an actor is a member of.
#[async_trait]
pub trait Memberships: Debug + Send + Sync {
    /// The group `id` - also one that was deleted, with the members it had;
    /// none that does not exist.
    async fn group(&self, id: GroupId) -> Option<Group>;
}

/// Knows of no membership: for a policy without access lists
/// naming them - tests, and what is built before a store is.
#[derive(Debug, Default)]
pub struct NoMemberships;

#[async_trait]
impl Memberships for NoMemberships {
    async fn group(&self, _id: GroupId) -> Option<Group> {
        None
    }
}

/// An entity that carries `SecurityAttributes`.
pub trait Secured {
    fn security_attributes(&self) -> &SecurityAttributes;

    /// The user it belongs to; `None` if its owner is no user - for a kind that must be a user's,
    /// only a document that could not be stored (`Stored::validate`).
    fn owner(&self) -> Option<UserId> {
        self.security_attributes().owner_id.as_user_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentId;
    use crate::approval::ApproverId;
    use crate::security::testing::*;
    use crate::store::query::meets_exactly_its_equals;
    use uuid::Uuid;

    #[test]
    fn an_access_list_names_people_roles_and_groups_never_an_agent() {
        let alice = user();
        let agent = ActorId::Agent(AgentId::new());
        assert_eq!(shared(alice, &[user()]).validate(), Ok(()));
        let place = Subject::Group(GroupId::new());
        assert_eq!(
            SecurityAttributes::owned_by(alice)
                .granting(Subject::Role(RoleId::new()), Grant::Read)
                .granting(place, Grant::Write)
                .validate(),
            Ok(())
        );
        for nobody in [agent, ActorId::System, ActorId::None] {
            assert!(
                SecurityAttributes::owned_by(alice)
                    .granting(Subject::Actor(nobody), Grant::Read)
                    .validate()
                    .is_err(),
                "not {nobody}"
            );
        }
        let written: SecurityAttributes =
            serde_json::from_value(serde_json::json!({"owner_id": alice})).unwrap();
        assert!(written.acl.is_empty(), "the list may be left out where it is written by hand");
    }

    /// Issue 491: everything has an owner, who answers for it.
    #[test]
    fn an_entity_is_owned_by_a_user_an_agent_or_the_system() {
        let agent = ActorId::Agent(AgentId::new());
        for owner in [user(), agent, ActorId::System] {
            assert_eq!(SecurityAttributes::owned_by(owner).validate(), Ok(()), "{owner}");
        }
        for nobody in [ActorId::None, ActorId::Approver(ApproverId::new())] {
            assert!(SecurityAttributes::owned_by(nobody).validate().is_err(), "not {nobody}");
        }
    }

    #[test]
    fn the_grants_covering_one_are_those_that_cover_it() {
        // Every grant is in `ALL`: a new one does not compile here until it is.
        let place = |grant: Grant| match grant {
            Grant::Read => 0,
            Grant::Write => 1,
            Grant::Join => 2,
        };
        for grant in Grant::ALL {
            assert_eq!(Grant::ALL[place(grant)], grant);
        }
        for needed in Grant::ALL {
            let covering: Vec<Grant> = Grant::covering(needed).collect();
            assert_eq!(
                covering,
                Grant::ALL.into_iter().filter(|grant| grant.covers(needed)).collect::<Vec<_>>(),
                "for {needed:?}"
            );
        }
        assert_eq!(
            Grant::covering(Grant::Read).collect::<Vec<_>>(),
            [Grant::Read, Grant::Write],
            "reading by a grant to read or to write"
        );
    }

    /// A subject as a query compares it (issue 293).
    #[test]
    fn a_subject_meets_exactly_the_subjects_equal_to_it() {
        let id = Uuid::new_v4();
        meets_exactly_its_equals(&[
            Subject::Actor(ActorId::User(UserId(id))),
            Subject::Actor(ActorId::Agent(AgentId(id))),
            Subject::Actor(ActorId::System),
            Subject::Role(RoleId(id)),
            Subject::Role(RoleId::new()),
            Subject::Group(GroupId(id)),
            Subject::Group(GroupId::new()),
            Subject::Everyone,
        ]);
    }
}
