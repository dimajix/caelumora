//! What the tests of the module share: actors, their contexts, attributes
//! that share an entity, the groups they belong to, and what the ownership
//! policy answers.

use super::{
    AccessPolicy, ActionType, Grant, Memberships, OwnershipPolicy, SecurityAttributes,
    SecurityContext, Subject,
};
use crate::actor::ActorId;
use crate::channel::ChannelId;
use crate::group::{Group, GroupId};
use crate::removal::Removal;
use crate::scope::Scope;
use crate::session::SessionId;
use crate::user::UserId;
use crate::value::name::ScopedName;
use async_trait::async_trait;

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
    readers.iter().fold(SecurityAttributes::owned_by(owner), |attributes, reader| {
        attributes.granting(Subject::Actor(*reader), Grant::Read)
    })
}

/// Memberships as a test sets them: who belongs to which group, and which
/// groups were deleted. Every group asked about is there, if only empty.
#[derive(Debug, Default)]
pub(super) struct Known {
    pub(super) members: Vec<(ActorId, GroupId)>,
    pub(super) deleted: Vec<GroupId>,
}

#[async_trait]
impl Memberships for Known {
    async fn group(&self, id: GroupId) -> Option<Group> {
        let members = self
            .members
            .iter()
            .filter(|(_, group)| *group == id)
            .map(|(actor, _)| *actor)
            .collect();
        let deleted = self.deleted.contains(&id).then(|| Removal::now(ActorId::System));
        let name = ScopedName::new("known", Scope::GlobalScope).ok()?;
        Some(Group { id, members, deleted, ..Group::team(name, UserId::new()) })
    }
}
