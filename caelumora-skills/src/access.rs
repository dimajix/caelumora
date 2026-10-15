//! Who may change and use a skill (issue 22): the pure questions about a
//! skill, an extension of `AccessQuestions` (issue 323) - each translated
//! into an action, decided by the policy alone. Every `AccessPolicy` has
//! them by the blanket implementation, so that no policy answers them
//! otherwise.

use crate::skill::{Skill, USE};
use async_trait::async_trait;
use caelumora_model::store::Stored;
use caelumora_model::security::{ActionType, SecurityAttributes, SecurityContext, AccessPolicy};

#[async_trait]
pub trait SkillAccess {
    /// Whether `context` may change the skill with `skill` attributes - its
    /// name, description, body, classification - and delete it
    /// (`UpdateSkill`, issue 22).
    async fn may_change_skill(&self, context: &SecurityContext, skill: &SecurityAttributes) -> bool;

    /// Whether `context` may load the skill with `skill` attributes into a
    /// turn (`UseSkill`, issue 22).
    async fn may_use_skill(&self, context: &SecurityContext, skill: &SecurityAttributes) -> bool;
}

#[async_trait]
impl<T: AccessPolicy + ?Sized> SkillAccess for T {
    async fn may_change_skill(&self, context: &SecurityContext, skill: &SecurityAttributes) -> bool {
        self.check(context, ActionType::update(Skill::KIND, skill)).await
    }

    async fn may_use_skill(&self, context: &SecurityContext, skill: &SecurityAttributes) -> bool {
        self.check(context, ActionType::custom(Skill::KIND, &USE, skill)).await
    }
}
