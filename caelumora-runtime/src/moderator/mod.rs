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
pub mod user_driven_policy;

use async_trait::async_trait;
use std::fmt::Debug;
use std::sync::Arc;

use crate::actor::Actor;
use crate::component::ComponentRegistry;
use caelumora_model::actor::{ActorDescription, ActorId, Envelope};
use caelumora_model::message::Message;

/// Decides how the events of a channel are routed and who is asked to
/// respond. A component of the channel, of which it has exactly one (issue
/// 174); shared like every facet, so it keeps what it changes behind its own
/// lock.
#[async_trait]
pub(crate) trait Moderator : Send + Sync + Debug {
    /// Whom `event` is delivered to, and as what: the event itself, or one
    /// of the moderator's own, such as a request to respond. What a user
    /// sees is what is routed to their actor (issue 220).
    async fn route(&self, event: &Envelope) -> Vec<(ActorId, Envelope)>;
    fn add_actor(&self, actor: Arc<dyn Actor>);
    fn remove_actor(&self, actor: ActorId);
}

/// Whom a moderator asks to respond to a message that mentions no agent:
/// the slot of the `policy_moderator`. A channel has exactly one.
#[async_trait]
pub trait ResponderPolicy: Send + Sync + Debug {
    /// The agents of `candidates` to ask to respond to `messages`, what
    /// `event` said - a message, or the messages of a turn -, by its sender
    /// and in its context, in which whatever the policy asks is asked. The
    /// candidates are the agents that are awake and did not say them, in no
    /// particular order, as they are described to everyone in the channel
    /// (issue 403).
    async fn choose(&self, event: &Envelope, messages: &[Message], candidates: Vec<ActorDescription>) -> Vec<ActorId>;
}

/// Registers the moderator and policy kinds with `registry`, and the
/// moderator and its policy as facets of which a channel has exactly one,
/// with their default kinds.
pub fn register_all(registry: &mut ComponentRegistry) {
    registry.single::<dyn Moderator>(Some(policy_moderator::POLICY_MODERATOR_KIND));
    registry.single::<dyn ResponderPolicy>(Some(round_robin_policy::ROUND_ROBIN_POLICY_KIND));

    broadcast_policy::register(registry);
    mentions_only_policy::register(registry);
    policy_moderator::register(registry);
    random_policy::register(registry);
    round_robin_policy::register(registry);
    user_driven_policy::register(registry);
}
