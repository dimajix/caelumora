//! The moderators, as components of a channel (issue 174). The built-in one,
//! [`policy_moderator`], routes the way every channel routes - every message
//! to every other participant, approvals, failed turns and commands to whom
//! they concern, and a response request to every agent a user
//! `@`-mentioned - and leaves only whom to ask when nobody was mentioned to
//! its slot, a [`ResponderPolicy`] (issue 44). The policies are components
//! of their own, one file per kind.

pub mod broadcast_policy;
pub mod mentions_only_policy;
pub mod policy_moderator;
pub mod random_policy;
pub mod round_robin_policy;

use crate::component::ComponentRegistry;
use crate::moderator::Moderator;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use std::fmt::Debug;

/// Whom a moderator asks to respond to a message that mentions no agent:
/// the slot of the `policy_moderator`. A channel has exactly one.
#[async_trait]
pub trait ResponderPolicy: Send + Sync + Debug {
    /// The agents of `candidates` to ask to respond to what `sender` wrote -
    /// the agents that are awake and did not write it, in no particular
    /// order.
    async fn choose(&self, sender: ActorId, candidates: Vec<ActorId>) -> Vec<ActorId>;
}

/// Registers the moderator and policy kinds with `registry`, and the
/// moderator and its policy as facets of which a channel has exactly one,
/// with their default kinds.
pub fn register_all(registry: &mut ComponentRegistry) {
    registry.single::<dyn Moderator>(policy_moderator::POLICY_MODERATOR_KIND);
    registry.single::<dyn ResponderPolicy>(round_robin_policy::ROUND_ROBIN_POLICY_KIND);

    broadcast_policy::register(registry);
    mentions_only_policy::register(registry);
    policy_moderator::register(registry);
    random_policy::register(registry);
    round_robin_policy::register(registry);
}
