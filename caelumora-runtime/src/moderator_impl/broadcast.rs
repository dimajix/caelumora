use crate::moderator_impl::ResponderPolicy;
use caelumora_model::actor::ActorId;

/// Asks every agent to respond.
#[derive(Debug, Default)]
pub struct BroadcastPolicy;

impl ResponderPolicy for BroadcastPolicy {
    fn choose(&mut self, candidates: Vec<ActorId>) -> Vec<ActorId> {
        candidates
    }
}
