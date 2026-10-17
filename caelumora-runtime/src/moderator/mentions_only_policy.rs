//! The `mentions_only_policy` component: asks nobody, so that agents respond
//! only when a user mentions them (issue 44).

use crate::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use crate::moderator::ResponderPolicy;
use async_trait::async_trait;
use caelumora_model::actor::{ActorDescription, ActorId, Envelope};
use caelumora_model::message::Message;
use std::sync::Arc;

pub const MENTIONS_ONLY_POLICY_KIND: &str = "mentions_only_policy";

/// Asks nobody: agents respond only when a user mentions them, and a message
/// that mentions no agent gets no answer (issue 44).
#[derive(Debug, Default)]
pub struct MentionsOnlyPolicy;

#[async_trait]
impl ResponderPolicy for MentionsOnlyPolicy {
    async fn choose(&self, _event: &Envelope, _messages: &[Message], _candidates: Vec<ActorDescription>) -> Vec<ActorId> {
        Vec::new()
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(MENTIONS_ONLY_POLICY_KIND).provides::<dyn ResponderPolicy>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ResponderPolicy>(Arc::new(MentionsOnlyPolicy));
            Ok(facets)
        },
    );
}
