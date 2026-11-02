//! The `broadcast_policy` component: asks every agent to respond.

use crate::component::ComponentRegistry;
use crate::moderator::{ResponderPolicy, Undecided};
use async_trait::async_trait;
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_domain::actor::{ActorDescription, ActorId, Envelope};
use caelumora_domain::message::Message;
use caelumora_security::SecurityContext;
use std::sync::Arc;

pub const BROADCAST_POLICY_KIND: &str = "broadcast_policy";

/// Asks every agent to respond.
#[derive(Debug, Default)]
pub struct BroadcastPolicy;

#[async_trait]
impl ResponderPolicy for BroadcastPolicy {
    async fn choose(
        &self,
        _event: &Envelope,
        _context: &SecurityContext,
        _messages: &[Message],
        candidates: Vec<ActorDescription>,
    ) -> Result<Vec<ActorId>, Undecided> {
        Ok(candidates.into_iter().map(|candidate| candidate.id).collect())
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
