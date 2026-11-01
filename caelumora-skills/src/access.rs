//! Who may change and use a skill (issue 22): the pure questions about a
//! skill, an extension of `AccessQuestions` (issue 323) - each translated
//! into an action, decided by the policy alone. Every `AccessPolicy` has
//! them by the blanket implementation, so that no policy answers them
//! otherwise.

use crate::skill::Skill;
use async_trait::async_trait;
use caelumora_model::security::{Grant, SecurityAttributes};
use caelumora_model::store::Stored;
use caelumora_security::{AccessPolicy, ActionType, CustomVerb, Requirement, SecurityContext};

/// Loading a skill into a turn (issue 22): what the agent then follows,
/// asked in the turn's context - the agent for its owner, whoever asked as
/// initiator. Apart from reading, so that a policy can let a skill be read
/// but not used, or the other way round; granted as reading by default.
/// Whether its classification may flow into the channel is asked besides
/// (`may_publish`).
pub const USE: CustomVerb = CustomVerb { name: "Use", requires: Requirement::Grant(Grant::Read) };

#[async_trait]
pub trait SkillAccess {
    /// Whether `context` may change the skill with `skill` attributes - its
    /// name, description, body, classification - and delete it
    /// (`UpdateSkill`, issue 22).
    async fn may_change_skill(&self, context: &SecurityContext, skill: &SecurityAttributes)
        -> bool;

    /// Whether `context` may load the skill with `skill` attributes into a
    /// turn (`UseSkill`, issue 22).
    async fn may_use_skill(&self, context: &SecurityContext, skill: &SecurityAttributes) -> bool;
}

#[async_trait]
impl<T: AccessPolicy + ?Sized> SkillAccess for T {
    async fn may_change_skill(
        &self,
        context: &SecurityContext,
        skill: &SecurityAttributes,
    ) -> bool {
        self.check(context, ActionType::update(Skill::KIND, skill)).await
    }

    async fn may_use_skill(&self, context: &SecurityContext, skill: &SecurityAttributes) -> bool {
        self.check(context, ActionType::custom(Skill::KIND, &USE, skill)).await
    }
}
