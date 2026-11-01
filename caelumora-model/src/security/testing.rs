//! Actors and the attributes that share an entity with them, as the tests of this crate and of
//! others write them.

use super::{SecurityAttributes, Subject};
use crate::actor::ActorId;
use crate::user::UserId;

/// A new user, as an actor.
pub fn user() -> ActorId {
    ActorId::User(UserId::new())
}

/// Owned by `owner`, read by `readers` (issue 23).
pub fn shared(owner: ActorId, readers: &[ActorId]) -> SecurityAttributes {
    SecurityAttributes::owned_by(owner).granting_read(readers.iter().copied().map(Subject::Actor))
}
