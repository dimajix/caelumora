//! In whose name something is done (issues 255, 341): the
//! `SecurityContext` every operation is asked in.

use caelumora_model::actor::ActorId;
use caelumora_model::agent::Agent;
use caelumora_model::approval::Approver;
use caelumora_model::channel::{Channel, ChannelId};
use caelumora_model::role::RoleId;
use caelumora_model::session::SessionId;
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

    /// The context `agent` acts in when nobody asked it, in the channel `channel` of session
    /// `session` (issue 87): begun by the agent itself, answered for by its owner and acting in
    /// the name of its principal (issues 255, 341).
    pub fn of_agent(agent: &Agent, session: SessionId, channel: ChannelId) -> Self {
        let actor = ActorId::Agent(agent.id);
        Self::new(actor, session, channel).next(
            actor,
            agent.security_attributes.owner_id,
            agent.principal_id,
        )
    }

    /// The context `approver` acts in, in `channel`: its own, answered for by the owner of its
    /// channel (issue 341) - what its actor continues a request in, and what the channel admits
    /// its envelopes in (issue 516).
    pub fn of_approver(approver: &Approver, channel: &Channel) -> Self {
        Self {
            owner_id: channel.security_attributes.owner_id,
            ..Self::new(ActorId::Approver(approver.id), channel.session_id, channel.id)
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
        self.roles.contains(&role)
            && self.limiting_roles.as_ref().is_none_or(|limit| limit.contains(&role))
    }

    /// The roles that count for the context: those of the owner that
    /// `has_role` accepts.
    pub fn counting_roles(&self) -> Vec<RoleId> {
        self.roles.iter().copied().filter(|role| self.has_role(*role)).collect()
    }

    /// The context of the kernel acting on its own - for work that follows from an action already
    /// permitted, or that no user started (initial data, records kept in step with events). The
    /// ownership policy allows it everything, so it is used deliberately and never as a fallback.
    pub fn system() -> Self {
        Self::new(ActorId::System, SessionId::default(), ChannelId::default())
    }

    /// Whether this is the system itself: caller, owner and principal the system, as `system`
    /// makes it (issue 477). Only the system itself has the system's rights; the system acting
    /// on someone's behalf - its principal another - has that principal's.
    pub fn is_system(&self) -> bool {
        [self.caller_id, self.owner_id, self.principal_id].iter().all(|id| *id == ActorId::System)
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

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::PersonalityId;
    use caelumora_model::user::UserId;
    use caelumora_model::value::name::ScopedName;

    /// Issue 341: an agent acting when nobody asked it is the caller and
    /// the initiator - it caused what it does -, answered for by its owner
    /// and acting in its owner's name.
    #[test]
    fn an_agent_acting_on_its_own_initiates_it_for_its_owner() {
        let owner = ActorId::User(UserId::new());
        let agent = Agent::new(
            ScopedName::new("helper", Default::default()).unwrap(),
            PersonalityId::new(),
            owner,
        );
        let context = SecurityContext::of_agent(&agent, Default::default(), Default::default());
        let itself = ActorId::Agent(agent.id);
        assert_eq!(
            (context.caller_id, context.initiator_id),
            (itself, itself),
            "it acts, and caused it"
        );
        assert_eq!(
            (context.owner_id, context.principal_id),
            (owner, owner),
            "answered for by its owner, in their name"
        );
    }
}
