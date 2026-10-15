//! What the tests of the module share: actors, their contexts, attributes
//! that share an entity, and what the ownership policy answers.

use crate::actor::ActorId;
use crate::session::SessionId;
use crate::channel::ChannelId;
use crate::user::UserId;
use super::{ActionType, AccessPolicy, Grant, OwnershipPolicy, SecurityAttributes, SecurityContext, Subject};

pub(super) fn user() -> ActorId {
    ActorId::User(UserId::new())
}

pub(super) fn context(actor: ActorId) -> SecurityContext {
    SecurityContext::new(actor, SessionId::default(), ChannelId::default())
}

pub(super) async fn allows(actor: ActorId, action: ActionType<'_>) -> bool {
    OwnershipPolicy::new().check(&context(actor), action).await
}

/// Owned by `owner`, read by `readers` (issue 23).
pub(super) fn shared(owner: ActorId, readers: &[ActorId]) -> SecurityAttributes {
    readers.iter().fold(SecurityAttributes::owned_by(owner), |attributes, reader| attributes.granting(Subject::Actor(*reader), Grant::Read))
}
