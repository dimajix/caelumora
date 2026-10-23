//! In whose name something is done (issues 255, 341): the
//! `SecurityContext` every operation is asked in.

use crate::actor::ActorId;
use crate::session::SessionId;
use crate::channel::ChannelId;
use crate::role::RoleId;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

/// Who acts, who answers for it, with whose rights, and for whom (issues
/// 255, 341): what every operation is asked in.
///
/// - `caller_id` is who acts - a user, an agent, the system.
/// - `owner_id` is who answers for the caller: a user or the system
///   themselves, an agent its owner. Its clearance bounds what the caller
///   may see, and it is asked the questions of a call.
/// - `principal_id` is in whose name the caller acts - whose rights it
///   exercises: a user themselves, an agent its owner, the system itself.
///   An agent is its owner's delegate in all it does, so today the
///   principal is always the owner; the two are apart so that a check says
///   which of them it means.
/// - `initiator_id` is who caused it - the root of the chain: whoever asked
///   the agent, or whoever asked whoever asked it, and the agent itself when
///   nobody did. For the record, never for rights.
///
/// - `roles` are the owner's roles, and `limiting_roles` those the caller is
///   limited to - an agent's personality's (issue 179) -, set where
///   the context is built (issue 304): what a decision needs to know of the
///   subject is in the context, not looked up while it is made. A role
///   counts if the owner holds it and the limit, if any, names it
///   (`has_role`); a clearance holds if both clear (see CONFIDENTIALITY.md).
///
/// An empty context names nobody and has no rights.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityContext {
    pub initiator_id: ActorId,
    pub caller_id: ActorId,
    pub owner_id: ActorId,
    pub principal_id: ActorId,
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    pub roles: Vec<RoleId>,
    pub limiting_roles: Option<Vec<RoleId>>,
}

impl SecurityContext {
    /// The context of `actor` acting for themselves, in the channel
    /// `channel_id` of session `session_id`: caller, owner, principal and
    /// initiator at once.
    pub fn new(actor: ActorId, session_id: SessionId, channel_id: ChannelId) -> Self {
        Self {
            initiator_id: actor,
            caller_id: actor,
            owner_id: actor,
            principal_id: actor,
            session_id,
            channel_id,
            roles: Vec::new(),
            limiting_roles: None,
        }
    }

    /// This context with the owner's `roles`, and the `limiting_roles` the
    /// caller is limited to, if any.
    pub fn with_roles(self, roles: Vec<RoleId>, limiting_roles: Option<Vec<RoleId>>) -> Self {
        Self { roles, limiting_roles, ..self }
    }

    /// Whether `role` counts for the context: the owner holds it, and the
    /// caller's limit, if any, names it.
    pub fn has_role(&self, role: RoleId) -> bool {
        self.roles.contains(&role) && self.limiting_roles.as_ref().is_none_or(|limit| limit.contains(&role))
    }

    /// The roles that count for the context: those of the owner that
    /// `has_role` accepts.
    pub fn counting_roles(&self) -> Vec<RoleId> {
        self.roles.iter().copied().filter(|role| self.has_role(*role)).collect()
    }

    /// The context of the runtime acting on its own - for work that follows
    /// from an action already permitted, or that no user started (initial
    /// data, records kept in step with events). The ownership policy allows
    /// it everything, so it is used deliberately and never as a fallback.
    pub fn system() -> Self {
        Self::new(ActorId::System, SessionId::default(), ChannelId::default())
    }

    /// This context continued by `caller_id`, answered for by `owner_id`,
    /// acting with the rights of `principal_id` - an agent taking up a
    /// request, owned by and acting for its owner - for the same initiator,
    /// in the same place. Without roles: whoever continues it gives the new
    /// owner's (`with_roles`), so that the initiator lends nothing.
    pub fn next(&self, caller_id: ActorId, owner_id: ActorId, principal_id: ActorId) -> Self {
        Self {
            initiator_id: self.initiator_id,
            caller_id,
            owner_id,
            principal_id,
            session_id: self.session_id,
            channel_id: self.channel_id,
            roles: Vec::new(),
            limiting_roles: None,
        }
    }
}
