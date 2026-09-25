use crate::moderator_impl::ResponderPolicy;
use caelumora_model::actor::ActorId;
use rand::prelude::StdRng;
use rand::seq::IndexedRandom;

/// Asks one agent to respond, chosen at random.
#[derive(Debug)]
pub struct RandomPolicy {
    rng: StdRng,
}

impl RandomPolicy {
    pub fn new() -> Self {
        Self { rng: rand::make_rng() }
    }
}

impl ResponderPolicy for RandomPolicy {
    fn choose(&mut self, candidates: Vec<ActorId>) -> Vec<ActorId> {
        candidates.choose(&mut self.rng).copied().into_iter().collect()
    }
}
