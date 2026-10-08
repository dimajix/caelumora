//! The `random_policy` component: asks one agent to respond, chosen at
//! random.

use crate::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use crate::moderator::ResponderPolicy;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use rand::prelude::StdRng;
use rand::seq::IndexedRandom;
use std::sync::{Arc, Mutex};

pub const RANDOM_POLICY_KIND: &str = "random_policy";

/// Asks one agent to respond, chosen at random.
#[derive(Debug)]
pub struct RandomPolicy {
    rng: Mutex<StdRng>,
}

impl RandomPolicy {
    pub fn new() -> Self {
        Self { rng: Mutex::new(rand::make_rng()) }
    }
}

#[async_trait]
impl ResponderPolicy for RandomPolicy {
    async fn choose(&self, _sender: ActorId, candidates: Vec<ActorId>) -> Vec<ActorId> {
        let mut rng = self.rng.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        candidates.choose(&mut *rng).copied().into_iter().collect()
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(RANDOM_POLICY_KIND).provides::<dyn ResponderPolicy>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ResponderPolicy>(Arc::new(RandomPolicy::new()));
            Ok(facets)
        },
    );
}
