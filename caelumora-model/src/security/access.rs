use crate::store::{EntityKind, Stored};
use async_trait::async_trait;
use std::fmt::Debug;
use crate::agent::Agent;
use crate::channel::Channel;
use crate::group::Group;
use crate::session::Session;
use crate::workspace::Workspace;
use super::{ActionType, Refused, SecurityAttributes, SecurityContext, WRITE};

#[async_trait]
pub trait AccessPolicy : Debug + Sync + Send {
    async fn check(&self, context: &SecurityContext, action: ActionType<'_>) -> bool;
}

/// The pure questions "may they?" that are asked from outside an entity -
/// by clients deciding what to offer, by readers deciding what to show -
/// each with its name, in one place (issue 71). They only translate a
/// question into an `ActionType`: the decision is `AccessPolicy::check`'s
/// alone, and since every `AccessPolicy` gets them by the blanket
/// implementation, no policy can answer them differently.
///
/// Operations that check their permission as part of what they do (a
/// channel's `cancel`, a store's `update_channel`) keep their check; a pure
/// question goes here, not onto the entity.
///
/// What a client is shown or offered - may it change a channel, read an
/// agent's context - comes from a view (`crate::view`, issue 80) that the
/// runtime builds with these questions; clients do not ask them themselves.
#[async_trait]
pub trait AccessQuestions {
    /// Whether `context` may read the session with `session` attributes -
    /// its participants, its channels (`GetSession`).
    async fn may_read_session(&self, context: &SecurityContext, session: &SecurityAttributes) -> bool;

    /// Whether `context` may read the channel with `channel` attributes -
    /// its members, its history, its events (`GetChannel`).
    async fn may_read_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool;

    /// Whether `context` may change the channel with `channel` attributes -
    /// rename it, add and remove participants (`UpdateChannel`).
    async fn may_change_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool;

    /// Whether `context` may change the session with `session` attributes -
    /// rename it, change its privacy (`UpdateSession`).
    async fn may_change_session(&self, context: &SecurityContext, session: &SecurityAttributes) -> bool;

    /// Whether `context` may change a workspace with these attributes - its
    /// name, description, classification, access list - and attach it to a
    /// session (`UpdateWorkspace`, issue 253).
    async fn may_change_workspace(&self, context: &SecurityContext, workspace: &SecurityAttributes) -> bool;

    /// Whether `context` may change the files of a workspace with these
    /// attributes (`WriteWorkspace`, issue 23).
    async fn may_write_workspace(&self, context: &SecurityContext, workspace: &SecurityAttributes) -> bool;

    /// Whether `context` may read a workspace with these attributes - see
    /// it and read its files (`GetWorkspace`, issue 23): asked where a
    /// workspace is reached, as a channel is (issue 304).
    async fn may_read_workspace(&self, context: &SecurityContext, workspace: &SecurityAttributes) -> bool;

    /// Whether `context` may see what the agent with `agent` attributes saw
    /// when it wrote a message (`ReadAgentContext`, issue 41).
    async fn may_read_agent_context(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool;

    /// Whether `context` may see which tools the agent with `agent`
    /// attributes has now in the channel with `channel` attributes
    /// (`ReadAgentTools`, issue 194).
    async fn may_read_agent_tools(&self, context: &SecurityContext, agent: &SecurityAttributes, channel: &SecurityAttributes) -> bool;

    /// Whether `context` may see which components the agent with `agent`
    /// attributes was built from (`ReadAgentComponents`, issue 270).
    async fn may_read_agent_components(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool;

    /// Whether `context` may approve the tool calls of the agent with
    /// `agent` attributes in the channel with `channel` attributes
    /// (`ApproveToolCall`, issue 14).
    async fn may_approve(&self, context: &SecurityContext, agent: &SecurityAttributes, channel: &SecurityAttributes) -> bool;

    /// Whether `context` may rename the agent with `agent` attributes
    /// (`UpdateAgent`, issue 227).
    async fn may_change_agent(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool;

    /// Whether `context` may delete the agent with `agent` attributes
    /// (`DeleteAgent`, issue 164).
    async fn may_delete_agent(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool;

    /// Whether `context` may ask an agent of the channel with `channel`
    /// attributes to answer again instead of one of its messages, or of its
    /// answer under way (`RetryMessage`, issues 234, 235) - asked to offer
    /// it; the channel asks again as it admits the request (issue 304).
    async fn may_retry_message(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool;

    /// Whether `context` may end the answers under way in the channel with
    /// `channel` attributes, or one agent's (`CancelChannel`, issues 235,
    /// 243) - asked by the channel as it cancels.
    async fn may_cancel_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool;

    /// Whether `context` may take back the deletion of an entity of `kind`
    /// with `attributes` (`Restore`, issue 164).
    async fn may_restore(&self, context: &SecurityContext, kind: EntityKind, attributes: &SecurityAttributes) -> bool;

    /// Whether `context` may remove an entity of `kind` with `attributes`
    /// for good (`Purge`, issue 164).
    async fn may_purge(&self, context: &SecurityContext, kind: EntityKind, attributes: &SecurityAttributes) -> bool;

    /// Whether `context` may rename a group with these attributes, change
    /// its members and delete it (`UpdateGroup`, issue 286).
    async fn may_change_group(&self, context: &SecurityContext, group: &SecurityAttributes) -> bool;

}

#[async_trait]
impl<T: AccessPolicy + ?Sized> AccessQuestions for T {
    async fn may_read_session(&self, context: &SecurityContext, session: &SecurityAttributes) -> bool {
        self.check(context, ActionType::get(Session::KIND, session)).await
    }

    async fn may_read_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::get(Channel::KIND, channel)).await
    }

    async fn may_change_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::update(Channel::KIND, channel)).await
    }

    async fn may_change_session(&self, context: &SecurityContext, session: &SecurityAttributes) -> bool {
        self.check(context, ActionType::update(Session::KIND, session)).await
    }

    async fn may_change_workspace(&self, context: &SecurityContext, workspace: &SecurityAttributes) -> bool {
        self.check(context, ActionType::update(Workspace::KIND, workspace)).await
    }

    async fn may_write_workspace(&self, context: &SecurityContext, workspace: &SecurityAttributes) -> bool {
        self.check(context, ActionType::custom(Workspace::KIND, &WRITE, workspace)).await
    }

    async fn may_read_workspace(&self, context: &SecurityContext, workspace: &SecurityAttributes) -> bool {
        self.check(context, ActionType::get(Workspace::KIND, workspace)).await
    }

    async fn may_read_agent_context(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool {
        self.check(context, ActionType::ReadAgentContext(agent)).await
    }

    async fn may_read_agent_tools(&self, context: &SecurityContext, agent: &SecurityAttributes, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::ReadAgentTools { agent, channel }).await
    }

    async fn may_read_agent_components(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool {
        self.check(context, ActionType::ReadAgentComponents(agent)).await
    }

    async fn may_approve(&self, context: &SecurityContext, agent: &SecurityAttributes, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::ApproveToolCall { agent, channel }).await
    }

    async fn may_change_agent(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool {
        self.check(context, ActionType::update(Agent::KIND, agent)).await
    }

    async fn may_delete_agent(&self, context: &SecurityContext, agent: &SecurityAttributes) -> bool {
        self.check(context, ActionType::delete(Agent::KIND, agent)).await
    }

    async fn may_retry_message(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::RetryMessage(channel)).await
    }

    async fn may_cancel_channel(&self, context: &SecurityContext, channel: &SecurityAttributes) -> bool {
        self.check(context, ActionType::CancelChannel(channel)).await
    }

    async fn may_restore(&self, context: &SecurityContext, kind: EntityKind, attributes: &SecurityAttributes) -> bool {
        self.check(context, ActionType::restore(kind, attributes)).await
    }

    async fn may_purge(&self, context: &SecurityContext, kind: EntityKind, attributes: &SecurityAttributes) -> bool {
        self.check(context, ActionType::purge(kind, attributes)).await
    }

    async fn may_change_group(&self, context: &SecurityContext, group: &SecurityAttributes) -> bool {
        self.check(context, ActionType::update(Group::KIND, group)).await
    }

}

/// The one way an operation checks its own permission (issue 95): asked as
/// part of what it does, a refusal ends it with the operation's own error,
/// and every refusal is logged alike - which action, for whom, at `debug!`:
/// a refusal is detail of one operation, and some are routine, such as a
/// store answering "not found" for what its caller may not see. Pure
/// questions, which only decide what to offer, show or filter, are
/// `AccessQuestions`. By blanket implementation on every `AccessPolicy`,
/// like those.
#[async_trait]
pub trait Enforcement {
    /// Nothing if `context` may do `action`; the caller's error made by
    /// `refusal` of what was refused if not. The error carries the
    /// [`Refused`] as its source, unless it deliberately says less, such as
    /// "not found".
    async fn require<E, F>(&self, context: &SecurityContext, action: ActionType<'_>, refusal: F) -> Result<(), E>
    where
        E: Send,
        F: FnOnce(Refused) -> E + Send;
}

#[async_trait]
impl<T: AccessPolicy + ?Sized> Enforcement for T {
    async fn require<E, F>(&self, context: &SecurityContext, action: ActionType<'_>, refusal: F) -> Result<(), E>
    where
        E: Send,
        F: FnOnce(Refused) -> E + Send,
    {
        if self.check(context, action).await {
            return Ok(());
        }
        let refused = action.refused();
        tracing::debug!("Refused {} for {}", refused.action, context.principal_id);
        Err(refusal(refused))
    }
}

/// Allows every action to everybody: for tests and development only. What
/// may flow where is not its to decide (issue 304) - the
/// `ConfidentialityPolicy` applies beside it all the same.
#[derive(Debug, Default)]
pub struct AllowAllPolicy;

impl AllowAllPolicy {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AccessPolicy for AllowAllPolicy {
    async fn check(&self, _context: &SecurityContext, _action: ActionType<'_>) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::testing::*;
    use crate::security::OwnershipPolicy;

    #[tokio::test]
    async fn access_questions_ask_the_policy() {
        let (alice, bob) = (user(), user());
        let hers = shared(alice, &[bob]);
        let access_policy = OwnershipPolicy::new();
        assert!(access_policy.may_read_channel(&context(bob), &hers).await, "a participant reads the channel");
        assert!(access_policy.may_change_channel(&context(alice), &hers).await, "the owner changes it");
        assert!(!access_policy.may_change_channel(&context(bob), &hers).await, "a participant does not");
        assert!(access_policy.may_change_session(&context(alice), &hers).await, "the owner changes a session");
        assert!(!access_policy.may_change_session(&context(bob), &hers).await, "a participant does not");
        assert!(!access_policy.may_read_agent_context(&context(bob), &hers).await, "what an agent saw is its owner's alone");
        assert!(allows(alice, ActionType::StopSession(&hers)).await, "the owner stops a session");
        assert!(!allows(bob, ActionType::StopSession(&hers)).await, "a participant does not");

    }

    #[tokio::test]
    async fn require_answers_a_refusal_with_the_callers_error() {
        #[derive(Debug, PartialEq)]
        struct Denied(Refused);
        let (owner, stranger) = (user(), user());
        let context = |actor| SecurityContext::new(actor, Default::default(), Default::default());
        let hers = SecurityAttributes::owned_by(owner);
        let access_policy = OwnershipPolicy::new();

        assert_eq!(access_policy.require(&context(owner), ActionType::delete(Session::KIND, &hers), Denied).await, Ok(()));
        let refused = Refused { subject: "session", action: "DeleteSession".to_string() };
        assert_eq!(access_policy.require(&context(stranger), ActionType::delete(Session::KIND, &hers), Denied).await, Err(Denied(refused)), "what was refused, named by its kind");
    }
}
