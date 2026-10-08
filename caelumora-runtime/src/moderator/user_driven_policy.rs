//! The `user_driven_policy` component: asks every agent to respond to what a
//! user wrote, and nobody to what an agent wrote (issue 175).

use crate::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use crate::moderator::ResponderPolicy;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use std::sync::Arc;

pub const USER_DRIVEN_POLICY_KIND: &str = "user_driven_policy";

/// Asks every agent to respond to a user's message, and nobody to an agent's:
/// the agents answer when a person writes, and are quiet in between. An
/// agent's message is still delivered to the others, it only starts no turn
/// (issue 175).
#[derive(Debug, Default)]
pub struct UserDrivenPolicy;

#[async_trait]
impl ResponderPolicy for UserDrivenPolicy {
    async fn choose(&self, sender: ActorId, candidates: Vec<ActorId>) -> Vec<ActorId> {
        match sender {
            ActorId::User(_) => candidates,
            _ => Vec::new(),
        }
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(USER_DRIVEN_POLICY_KIND).provides::<dyn ResponderPolicy>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ResponderPolicy>(Arc::new(UserDrivenPolicy));
            Ok(facets)
        },
    );
}
