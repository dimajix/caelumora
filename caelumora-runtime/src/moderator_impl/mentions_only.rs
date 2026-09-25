use crate::moderator_impl::ResponderPolicy;
use caelumora_model::actor::ActorId;

/// Asks nobody: agents respond only when a user mentions them, and a message
/// that mentions no agent gets no answer (issue 44).
#[derive(Debug, Default)]
pub struct MentionsOnlyPolicy;

impl ResponderPolicy for MentionsOnlyPolicy {
    fn choose(&mut self, _candidates: Vec<ActorId>) -> Vec<ActorId> {
        Vec::new()
    }
}
