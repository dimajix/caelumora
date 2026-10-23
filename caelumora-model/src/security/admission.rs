//! Which entities a context is admitted to by their security attributes
//! alone (issue 332): the policy's rule for a verb on entities, in a form a
//! store can apply to a whole list as well as to one entity.

use crate::actor::ActorId;
use crate::group::GroupId;
use crate::role::RoleId;
use crate::store::Stored;
use super::{Grant, Memberships, SecurityAttributes, Subject};

/// Which entities a context may do one verb with, in the words of
/// `SecurityAttributes`: whom an owner or an entry has to name, and what an
/// entry has to grant. Given by the policy (`AccessPolicy::admission`),
/// which decides an entity by it as well, so that a list a store narrows by
/// it and the decision on one entity cannot come apart.
///
/// The default admits nobody.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Admission {
    /// Who is admitted as themselves - by owning it, by an entry naming them
    /// or a group of theirs: the principal, and the caller where it takes
    /// part itself (issue 264).
    pub actors: Vec<ActorId>,
    /// Whether what the system or nobody owns admits as well.
    pub unowned: bool,
    /// What an entry of the access list must cover to admit; `None` where
    /// owning alone does.
    pub grant: Option<Grant>,
    /// The roles an entry may name: those that count for the context
    /// (`SecurityContext::counting_roles`).
    pub roles: Vec<RoleId>,
    /// Whether an entry for everyone admits.
    pub everyone: bool,
    /// Whether an entry for a group admits the members it had when it was
    /// deleted, as it admits those of a group that is there: for telling
    /// them of a deletion (`Verb::HearDeleted`), since the groups that
    /// reached it may have gone with it.
    pub deleted_groups: bool,
}

impl Admission {
    /// Whether an entity with `attributes` is admitted. The entries are
    /// matched in their order, and `memberships` is asked only for a group
    /// an entry names.
    pub async fn admits(&self, attributes: &SecurityAttributes, memberships: &dyn Memberships) -> bool {
        if self.actors.contains(&attributes.owner_id) || (self.unowned && attributes.unowned()) {
            return true;
        }
        let Some(needed) = self.grant else { return false };
        for entry in attributes.acl.iter().filter(|entry| entry.grant.covers(needed)) {
            let admitted = match entry.subject {
                Subject::Actor(actor) => self.actors.contains(&actor),
                Subject::Role(role) => self.roles.contains(&role),
                Subject::Everyone => self.everyone,
                Subject::Group(group) => self.admits_members_of(group, memberships).await,
            };
            if admitted {
                return true;
            }
        }
        false
    }

    /// Whether one of the actors belongs to `group` - to a deleted one only
    /// where `deleted_groups` says so.
    async fn admits_members_of(&self, group: GroupId, memberships: &dyn Memberships) -> bool {
        let Some(group) = memberships.group(group).await else { return false };
        (group.deleted().is_none() || self.deleted_groups) && self.actors.iter().any(|actor| group.members.contains(actor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::testing::*;

    fn reading(actors: Vec<ActorId>) -> Admission {
        Admission { actors, unowned: true, grant: Some(Grant::Read), ..Admission::default() }
    }

    #[tokio::test]
    async fn an_entity_is_admitted_by_its_owner_or_an_entry_covering_the_grant() {
        let (alice, bob, carol) = (user(), user(), user());
        let none = Known::default();
        let hers = shared(alice, &[bob]);
        assert!(reading(vec![alice]).admits(&hers, &none).await, "its owner");
        assert!(reading(vec![bob]).admits(&hers, &none).await, "whom an entry names");
        assert!(!reading(vec![carol]).admits(&hers, &none).await, "nobody else");
        let writing = Admission { grant: Some(Grant::Write), ..reading(vec![bob]) };
        assert!(!writing.admits(&hers, &none).await, "an entry covering what is needed");
        let owning = Admission { grant: None, unowned: false, ..reading(vec![bob]) };
        assert!(!owning.admits(&hers, &none).await, "no entry where owning alone admits");
        assert!(reading(vec![]).admits(&SecurityAttributes::owned_by(ActorId::System), &none).await, "the system's, where unowned admits");
        assert!(!owning.admits(&SecurityAttributes::default(), &none).await, "but not where it does not");
        assert!(!Admission::default().admits(&hers, &none).await, "the default admits nobody");
    }

    #[tokio::test]
    async fn an_entry_admits_by_a_role_that_counts_everyone_and_a_group() {
        let (alice, bob) = (user(), user());
        let role = RoleId::new();
        let by_role = SecurityAttributes::owned_by(alice).granting(Subject::Role(role), Grant::Read);
        let none = Known::default();
        assert!(Admission { roles: vec![role], ..reading(vec![bob]) }.admits(&by_role, &none).await, "a role that counts");
        assert!(!reading(vec![bob]).admits(&by_role, &none).await, "not one that does not");
        let open = SecurityAttributes::owned_by(alice).granting(Subject::Everyone, Grant::Read);
        assert!(Admission { everyone: true, ..reading(vec![bob]) }.admits(&open, &none).await, "an entry for everyone");
        assert!(!reading(vec![bob]).admits(&open, &none).await, "where it admits");

        let team = GroupId::new();
        let shared_with = SecurityAttributes::owned_by(alice).granting(Subject::Group(team), Grant::Read);
        let known = Known { members: vec![(bob, team)], ..Known::default() };
        assert!(reading(vec![bob]).admits(&shared_with, &known).await, "a member of the group named");
        assert!(!reading(vec![user()]).admits(&shared_with, &known).await, "not who is no member");
    }

    #[tokio::test]
    async fn a_deleted_group_admits_nobody_but_where_a_deletion_is_told() {
        let (alice, bob) = (user(), user());
        let team = GroupId::new();
        let shared_with = SecurityAttributes::owned_by(alice).granting(Subject::Group(team), Grant::Read);
        let known = Known { members: vec![(bob, team)], deleted: vec![team] };
        assert!(!reading(vec![bob]).admits(&shared_with, &known).await, "its entries grant nothing once it is deleted");
        assert!(Admission { deleted_groups: true, ..reading(vec![bob]) }.admits(&shared_with, &known).await, "but its members hear of what went with it");
    }
}
