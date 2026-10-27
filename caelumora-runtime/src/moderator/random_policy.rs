//! The `random_policy` component: asks one agent to respond, chosen at
//! random.

use crate::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use crate::moderator::{ResponderPolicy, Undecided};
use async_trait::async_trait;
use caelumora_model::actor::{ActorDescription, ActorId, Envelope};
use caelumora_model::message::Message;
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

impl Default for RandomPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ResponderPolicy for RandomPolicy {
    async fn choose(&self, _event: &Envelope, _messages: &[Message], candidates: Vec<ActorDescription>) -> Result<Vec<ActorId>, Undecided> {
        let mut rng = self.rng.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(candidates.choose(&mut *rng).map(|candidate| candidate.id).into_iter().collect())
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
