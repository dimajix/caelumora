//! The `round_robin_policy` component: the agents take turns, in the order
//! of their ids. The default policy of a channel.

use crate::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use crate::moderator_impl::ResponderPolicy;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use std::sync::{Arc, Mutex};

pub const ROUND_ROBIN_POLICY_KIND: &str = "round_robin_policy";

/// Asks one agent to respond, taking turns in the order of their ids.
#[derive(Debug, Default)]
pub struct RoundRobinPolicy {
    next_agent_index: Mutex<usize>,
}

#[async_trait]
impl ResponderPolicy for RoundRobinPolicy {
    async fn choose(&self, _sender: ActorId, mut candidates: Vec<ActorId>) -> Vec<ActorId> {
        let mut next_agent_index = self.next_agent_index.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if candidates.is_empty() {
            *next_agent_index = 0;
            return candidates;
        }
        candidates.sort_unstable_by_key(|id| id.id());
        let index = *next_agent_index % candidates.len();
        *next_agent_index = (index + 1) % candidates.len();
        vec![candidates[index]]
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(ROUND_ROBIN_POLICY_KIND).provides::<dyn ResponderPolicy>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ResponderPolicy>(Arc::new(RoundRobinPolicy::default()));
            Ok(facets)
        },
    );
}
