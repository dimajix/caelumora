//! Who may reclassify an entry of an agent's memory (issues 133, 518): the pure question about
//! a memory, an extension of `AccessQuestions` (issue 323) - translated into an action, decided by
//! the policy alone. Every `AccessPolicy` has it by the blanket implementation, so that no policy
//! answers it otherwise.

use crate::memory::AgentMemory;
use async_trait::async_trait;
use caelumora_domain::security::SecurityAttributes;
use caelumora_domain::store::Stored;
use caelumora_security::{AccessPolicy, ActionType, CustomVerb, Requirement, SecurityContext};

/// Reclassifying an entry of an agent's memory: released, or raised. The memory's owner's, the
/// agent's owner - for whom the agent acts. Whether the new classification may be given there is
/// asked besides (`may_publish`).
pub const RECLASSIFY: CustomVerb = CustomVerb { name: "Reclassify", requires: Requirement::Owner };

#[async_trait]
pub trait MemoryAccess {
    /// Whether `context` may reclassify an entry of the memory with `memory` attributes
    /// (`ReclassifyMemory`).
    async fn may_reclassify_memory(
        &self,
        context: &SecurityContext,
        memory: &SecurityAttributes,
    ) -> bool;
}

#[async_trait]
impl<T: AccessPolicy + ?Sized> MemoryAccess for T {
    async fn may_reclassify_memory(
        &self,
        context: &SecurityContext,
        memory: &SecurityAttributes,
    ) -> bool {
        self.check(context, ActionType::custom(AgentMemory::KIND, &RECLASSIFY, memory)).await
    }
}
