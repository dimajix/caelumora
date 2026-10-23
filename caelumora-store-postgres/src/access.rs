//! The access entries of the documents of a kind (issue 332): written with
//! each document, and an admission as a condition of SQL over them.

use crate::filter::Condition;
use caelumora_model::actor::ActorId;
use caelumora_model::store::query::Filter;
use caelumora_model::security::{Admission, Grant, SecurityAttributes, Subject};
use sqlx::postgres::PgArguments;
use sqlx::query::QueryAs;
use sqlx::Postgres;
use std::iter;
use uuid::Uuid;

/// The subject of an entry for everyone, which has no id of its own.
const EVERYONE: Uuid = Uuid::max();

/// The owner of what the system or nobody owns, who have no id of their
/// own (`ActorId::id`).
const UNOWNED: Uuid = Uuid::nil();

/// The relation of the owner's row; an entry's is its grant (`relation`).
const OWNER: &str = "owner";

/// The rows of the access entries of a document with `attributes`: the
/// owner and every entry of its access list, each as its subject and its
/// relation - the subject by its id alone, since the ids of actors, roles
/// and groups do not meet, and a scan that met one by chance would only
/// hand out more, which the store decides again.
pub(crate) fn entries(attributes: &SecurityAttributes) -> (Vec<Uuid>, Vec<String>) {
    let owner = (attributes.owner_id.id(), OWNER.to_string());
    let listed = attributes.acl.iter().map(|entry| (subject(entry.subject), relation(entry.grant).to_string()));
    iter::once(owner).chain(listed).unzip()
}

/// The id an entry names `subject` by.
fn subject(subject: Subject) -> Uuid {
    match subject {
        Subject::Actor(actor) => actor.id(),
        Subject::Role(role) => role.into(),
        Subject::Group(group) => group.into(),
        Subject::Everyone => EVERYONE,
    }
}

/// The relation of an entry granting `grant`.
fn relation(grant: Grant) -> &'static str {
    match grant {
        Grant::Read => "read",
        Grant::Write => "write",
        Grant::Join => "join",
    }
}

/// An admission as a condition of SQL on the id of a document: that one of
/// its access entries admits it - the owner's, an entry for an actor, a
/// role or everyone, or one for a group an actor belongs to, found in the
/// table of the groups by the members its documents list (`Filter::with_member`).
/// It holds for every document `Admission::admits` admits, and for more only
/// where ids of different subjects met by chance.
#[derive(Debug)]
pub(crate) struct Admitted {
    pub(crate) sql: String,
    /// Who may own what is admitted.
    owners: Vec<Uuid>,
    /// The relations an entry must have to admit.
    relations: Vec<String>,
    /// Whom an entry may name to admit, besides a group.
    subjects: Vec<Uuid>,
    /// Of every actor, the groups it belongs to, in the table of the groups.
    groups: Vec<Condition>,
}

impl Admitted {
    /// The condition of `admission` on the column `id` of a kind's table,
    /// over its access entries in `access` and the groups in `groups` - both
    /// tables quoted -, its parameters numbered from `first`.
    pub(crate) fn of(admission: &Admission, access: &str, groups: &str, first: usize) -> Self {
        let owners = admission.actors.iter().map(ActorId::id).chain(admission.unowned.then_some(UNOWNED)).collect();
        let relations: Vec<String> = admission.grant.into_iter().flat_map(Grant::covering).map(|grant| relation(grant).to_string()).collect();
        let subjects = admission.actors.iter().map(ActorId::id)
            .chain(admission.roles.iter().map(|role| Uuid::from(*role)))
            .chain(admission.everyone.then_some(EVERYONE))
            .collect();
        let (owned, related, named) = (format!("${first}"), format!("${}", first + 1), format!("${}", first + 2));
        let mut sql = format!("id IN (SELECT entity_id FROM {access} WHERE (relation = '{OWNER}' AND subject = ANY({owned})) \
            OR (relation = ANY({related}) AND subject = ANY({named}))");
        let mut groups_of = Vec::new();
        let mut next = first + 3;
        // A group admits only by an entry, and only where entries do.
        if admission.grant.is_some() {
            let present = if admission.deleted_groups { "true" } else { "NOT g.deleted" };
            for member in &admission.actors {
                let belonging = Condition::of(&Filter::with_member(*member).by_path(), "g.body", next);
                next = belonging.next();
                sql.push_str(&format!(" UNION ALL SELECT a.entity_id FROM {groups} g JOIN {access} a ON a.subject = g.id AND a.relation = ANY({related}) \
                    WHERE {present} AND {}", belonging.sql));
                groups_of.push(belonging);
            }
        }
        sql.push(')');
        Self { sql, owners, relations, subjects, groups: groups_of }
    }

    /// `query`, which binds the parameters before the condition's first,
    /// with what the condition binds bound after them.
    pub(crate) fn bound<'q, O>(&self, query: QueryAs<'q, Postgres, O, PgArguments>) -> QueryAs<'q, Postgres, O, PgArguments> {
        let query = query.bind(self.owners.clone()).bind(self.relations.clone()).bind(self.subjects.clone());
        self.groups.iter().fold(query, |query, belonging| belonging.bound(query))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::group::GroupId;
    use caelumora_model::role::RoleId;
    use caelumora_model::user::UserId;

    #[test]
    fn a_document_is_entered_by_its_owner_and_each_entry_of_its_list() {
        let (alice, bob) = (UserId::new(), UserId::new());
        let (role, group) = (RoleId::new(), GroupId::new());
        let attributes = SecurityAttributes::owned_by(ActorId::User(alice))
            .granting(Subject::Actor(ActorId::User(bob)), Grant::Read)
            .granting(Subject::Role(role), Grant::Write)
            .granting(Subject::Group(group), Grant::Join)
            .granting(Subject::Everyone, Grant::Read);
        let (subjects, relations) = entries(&attributes);
        assert_eq!(subjects, [alice.into(), bob.into(), role.into(), group.into(), EVERYONE]);
        assert_eq!(relations, [OWNER, "read", "write", "join", "read"]);
        for unowned in [ActorId::System, ActorId::None] {
            assert_eq!(entries(&SecurityAttributes::owned_by(unowned)).0, [UNOWNED], "{unowned} by no id of its own");
        }
    }
}
