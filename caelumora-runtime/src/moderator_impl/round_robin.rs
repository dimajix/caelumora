use crate::moderator_impl::ResponderPolicy;
use caelumora_model::actor::ActorId;

/// Asks one agent to respond, taking turns in the order of their ids.
#[derive(Debug, Default)]
pub struct RoundRobinPolicy {
    next_agent_index: usize,
}

impl ResponderPolicy for RoundRobinPolicy {
    fn choose(&mut self, mut candidates: Vec<ActorId>) -> Vec<ActorId> {
        if candidates.is_empty() {
            self.next_agent_index = 0;
            return candidates;
        }

        candidates.sort_unstable_by_key(|id| id.id());
        let index = self.next_agent_index % candidates.len();
        self.next_agent_index = (index + 1) % candidates.len();
        vec![candidates[index]]
    }
}
