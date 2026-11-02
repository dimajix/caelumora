//! `Memberships`: what a policy is told of the groups an access list names.

use async_trait::async_trait;
use caelumora_domain::group::{Group, GroupId};
use std::fmt::Debug;

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
