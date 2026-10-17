//! The `broadcast_policy` component: asks every agent to respond.

use crate::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use crate::moderator::ResponderPolicy;
use async_trait::async_trait;
use caelumora_model::actor::{ActorDescription, ActorId, Envelope};
use caelumora_model::message::Message;
use std::sync::Arc;

pub const BROADCAST_POLICY_KIND: &str = "broadcast_policy";

/// Asks every agent to respond.
#[derive(Debug, Default)]
pub struct BroadcastPolicy;

#[async_trait]
impl ResponderPolicy for BroadcastPolicy {
    async fn choose(&self, _event: &Envelope, _messages: &[Message], candidates: Vec<ActorDescription>) -> Vec<ActorId> {
        candidates.into_iter().map(|candidate| candidate.id).collect()
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(BROADCAST_POLICY_KIND).provides::<dyn ResponderPolicy>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ResponderPolicy>(Arc::new(BroadcastPolicy));
            Ok(facets)
        },
    );
}
