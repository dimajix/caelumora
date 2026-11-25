//! Which entities a context is admitted to by their security attributes
//! alone (issue 332): the policy's rule for a verb on entities, in a form a
//! store can apply to a whole list as well as to one entity.

use crate::Memberships;
use caelumora_domain::actor::ActorId;
use caelumora_domain::group::GroupId;
use caelumora_domain::role::RoleId;
use caelumora_domain::security::{Grant, SecurityAttributes, Subject};
use caelumora_domain::store::Stored;

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
    /// Whether what the system owns admits as well.
    pub systems: bool,
    /// What an entry of the access list must cover to admit; `None` where
    /// owning alone does.
    pub grant: Option<Grant>,
    /// The roles an entry may name: the owner's (`SecurityContext::roles`).
    pub roles: Vec<RoleId>,
    /// The roles the caller is limited to, if any (`SecurityContext::limiting_roles`): where
    /// set, an entry covering the grant has to name one of them as well - the same entry as for
    /// `roles`, or another (issue 605).
    pub limiting_roles: Option<Vec<RoleId>>,
    /// Whether an entry for everyone admits.
    pub everyone: bool,
    /// Whether an entry for a group admits the members it had when it was
    /// deleted, as it admits those of a group that is there: for telling
    /// them of a deletion (`Verb::HearDeleted`), since the groups that
    /// reached it may have gone with it.
    pub deleted_groups: bool,
}

impl Admission {
    /// Whether an entity with `attributes` is admitted. `memberships` is
    /// asked only for a group an entry names, once nothing else admits it.
    pub(crate) async fn admits(
        &self,
        attributes: &SecurityAttributes,
        memberships: &dyn Memberships,
    ) -> bool {
        if self.admits_without_groups(attributes) {
            return true;
        }
        for group in self.groups_named(attributes) {
            if self.admits_members_of(group, memberships).await {
                return true;
            }
        }
        false
    }

    /// Whether an entity with `attributes` may be admitted, told without
    /// the groups: an entry for a group counts as admitting. More than
    /// `admits`, never less - what a storage that does not keep the groups
    /// narrows a list by (issue 332).
    pub fn may_admit(&self, attributes: &SecurityAttributes) -> bool {
        self.admits_without_groups(attributes) || self.groups_named(attributes).next().is_some()
    }

    /// Whether the owner of an entity with `attributes`, or entries of its list naming no group,
    /// admit it.
    fn admits_without_groups(&self, attributes: &SecurityAttributes) -> bool {
        let systems = self.systems && attributes.owner_id == ActorId::System;
        if self.actors.contains(&attributes.owner_id) || systems {
            return true;
        }
        let named = self.covering(attributes).any(|subject| match subject {
            Subject::Actor(actor) => self.actors.contains(&actor),
            Subject::Everyone => self.everyone,
            Subject::Role(_) | Subject::Group(_) => false,
        });
        named || self.admits_by_roles(attributes)
    }

    /// Whether the entries of `attributes` covering the grant name one of the owner's roles and,
    /// where the caller is limited, one of the limit's - each asked by itself, so that two entries
    /// may admit together (issue 605).
    fn admits_by_roles(&self, attributes: &SecurityAttributes) -> bool {
        let names_one_of = |roles: &[RoleId]| {
            self.covering(attributes)
                .any(|subject| matches!(subject, Subject::Role(role) if roles.contains(&role)))
        };
        names_one_of(&self.roles) && self.limiting_roles.as_deref().is_none_or(names_one_of)
    }

    /// The groups the entries of `attributes` covering the grant name.
    fn groups_named<'a>(
        &'a self,
        attributes: &'a SecurityAttributes,
    ) -> impl Iterator<Item = GroupId> + 'a {
        self.covering(attributes).filter_map(|subject| match subject {
            Subject::Group(group) => Some(group),
            Subject::Actor(_) | Subject::Role(_) | Subject::Everyone => None,
        })
    }

    /// Whom the entries of `attributes` covering the grant name - none
    /// where owning alone admits.
    fn covering<'a>(
        &'a self,
        attributes: &'a SecurityAttributes,
    ) -> impl Iterator<Item = Subject> + 'a {
        let needed = self.grant;
        attributes
            .acl
            .iter()
            .filter(move |entry| needed.is_some_and(|needed| entry.grant.covers(needed)))
            .map(|entry| entry.subject)
    }

    /// Whether one of the actors belongs to `group` - to a deleted one only
    /// where `deleted_groups` says so.
    async fn admits_members_of(&self, group: GroupId, memberships: &dyn Memberships) -> bool {
        let Some(group) = memberships.group(group).await else { return false };
        (group.deleted().is_none() || self.deleted_groups)
            && self.actors.iter().any(|actor| group.members.contains(actor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;

    fn reading(actors: Vec<ActorId>) -> Admission {
        Admission { actors, systems: true, grant: Some(Grant::Read), ..Admission::default() }
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
        let owning = Admission { grant: None, systems: false, ..reading(vec![bob]) };
        assert!(!owning.admits(&hers, &none).await, "no entry where owning alone admits");
        assert!(
            reading(vec![]).admits(&SecurityAttributes::systems(), &none).await,
            "the system's, where the system's admit"
        );
        assert!(
            !owning.admits(&SecurityAttributes::systems(), &none).await,
            "but not where it does not"
        );
        assert!(!Admission::default().admits(&hers, &none).await, "the default admits nobody");
    }

    #[tokio::test]
    async fn an_entry_admits_by_a_role_of_the_owner_everyone_and_a_group() {
        let (alice, bob) = (user(), user());
        let role = RoleId::new();
        let by_role =
            SecurityAttributes::owned_by(alice).granting(Subject::Role(role), Grant::Read);
        let none = Known::default();
        assert!(
            Admission { roles: vec![role], ..reading(vec![bob]) }.admits(&by_role, &none).await,
            "a role of the owner"
        );
        assert!(!reading(vec![bob]).admits(&by_role, &none).await, "not one they do not hold");
        let open = SecurityAttributes::owned_by(alice).granting(Subject::Everyone, Grant::Read);
        assert!(
            Admission { everyone: true, ..reading(vec![bob]) }.admits(&open, &none).await,
            "an entry for everyone"
        );
        assert!(!reading(vec![bob]).admits(&open, &none).await, "where it admits");

        let team = GroupId::new();
        let shared_with =
            SecurityAttributes::owned_by(alice).granting(Subject::Group(team), Grant::Read);
        let known = Known { members: vec![(bob, team)], ..Known::default() };
        assert!(
            reading(vec![bob]).admits(&shared_with, &known).await,
            "a member of the group named"
        );
        assert!(!reading(vec![user()]).admits(&shared_with, &known).await, "not who is no member");
    }

    /// Issue 605: a limit is asked beside the owner's roles - an entry has to name one of each,
    /// the same entry or another, and the owner need not hold a role of the limit.
    #[tokio::test]
    async fn a_limited_caller_is_admitted_by_a_role_of_its_owner_and_one_of_its_limit() {
        let (alice, bob) = (user(), user());
        let (head, accountant) = (RoleId::new(), RoleId::new());
        let none = Known::default();
        let limited = Admission {
            roles: vec![head],
            limiting_roles: Some(vec![accountant]),
            ..reading(vec![bob])
        };
        let granting = |roles: &[RoleId]| {
            roles.iter().fold(SecurityAttributes::owned_by(alice), |attributes, role| {
                attributes.granting(Subject::Role(*role), Grant::Read)
            })
        };
        assert!(limited.admits(&granting(&[head, accountant]), &none).await, "two entries");
        assert!(!limited.admits(&granting(&[head]), &none).await, "not the owner's role alone");
        assert!(!limited.admits(&granting(&[accountant]), &none).await, "nor the limit's alone");
        let unlimited = Admission { limiting_roles: None, ..limited.clone() };
        assert!(unlimited.admits(&granting(&[head]), &none).await, "unlimited, the owner's");
        let shared_with_bob = granting(&[]).granting(Subject::Actor(bob), Grant::Read);
        assert!(limited.admits(&shared_with_bob, &none).await, "the limit asks no roles of others");
    }

    #[test]
    fn an_entity_may_be_admitted_by_any_group_an_entry_names() {
        let (alice, bob) = (user(), user());
        let shared_with = SecurityAttributes::owned_by(alice)
            .granting(Subject::Group(GroupId::new()), Grant::Read);
        assert!(reading(vec![bob]).may_admit(&shared_with), "by a group, told without it");
        assert!(
            !Admission { grant: Some(Grant::Write), ..reading(vec![bob]) }.may_admit(&shared_with),
            "but only by an entry covering the grant"
        );
        assert!(reading(vec![bob]).may_admit(&shared(alice, &[bob])), "and by what admits it");
        assert!(!reading(vec![bob]).may_admit(&shared(alice, &[])), "not by what does not");
    }

    #[tokio::test]
    async fn a_deleted_group_admits_nobody_but_where_a_deletion_is_told() {
        let (alice, bob) = (user(), user());
        let team = GroupId::new();
        let shared_with =
            SecurityAttributes::owned_by(alice).granting(Subject::Group(team), Grant::Read);
        let known = Known { members: vec![(bob, team)], deleted: vec![team] };
        assert!(
            !reading(vec![bob]).admits(&shared_with, &known).await,
            "its entries grant nothing once it is deleted"
        );
        assert!(
            Admission { deleted_groups: true, ..reading(vec![bob]) }
                .admits(&shared_with, &known)
                .await,
            "but its members hear of what went with it"
        );
    }
}
