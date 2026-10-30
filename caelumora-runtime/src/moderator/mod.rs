//! The moderators, as components of a channel (issue 174). A moderator decides whom an event goes
//! to, whom to ask to respond and what to tell of its own - a [`Routing`] - and never what anyone
//! is told (issue 418): the channel delivers by the rule every channel delivers by
//! (`Envelope::concerns`) unless the moderator names whom, and builds every envelope. The built-in
//! one, [`policy_moderator`], asks every agent a message `@`-mentions and leaves whom to ask when
//! nobody was mentioned to its slot, a [`ResponderPolicy`] (issue 44), and whom to ask when the
//! policy could not choose to its own rule (issue 441). The policies are components of their own,
//! one file per kind.

pub mod broadcast_policy;
pub mod mentions_only_policy;
pub mod policy_moderator;
pub mod random_policy;
pub mod round_robin_policy;
pub mod user_driven_policy;

use crate::actor::Actor;
use crate::component::ComponentRegistry;
use async_trait::async_trait;
use caelumora_model::actor::{ActorDescription, ActorEvent, ActorId, Envelope, RequestReason};
use caelumora_model::history::Audience;
use caelumora_model::message::{Message, MessageId};
use std::fmt::Debug;
use std::sync::Arc;

/// Decides whom the events of a channel are delivered to, who is asked to
/// respond, and what it tells of its own - never what anyone is told of an
/// event (issue 418): the channel builds every envelope it delivers. A
/// component of the channel, of which it has exactly one (issue 174);
/// shared like every facet, so it keeps what it changes behind its own lock.
#[async_trait]
pub(crate) trait Moderator: Send + Sync + Debug {
    /// What becomes of `event` in the channel: whom it is delivered to,
    /// whom to ask to respond, and what the moderator tells of its own.
    /// What a user sees is what is delivered to their actor (issue 220).
    async fn route(&self, event: &Envelope) -> Routing;
    fn add_actor(&self, actor: Arc<dyn Actor>);
    fn remove_actor(&self, actor: ActorId);
}

/// What a moderator decided of an event (issue 418). The default delivers
/// it as every channel does and asks nobody.
#[derive(Debug, Default)]
pub(crate) struct Routing {
    /// Whom the event is delivered to, as it entered the channel - never
    /// back to its sender, unless the channel announces it in their name.
    /// What its entry in the history records (issue 417).
    pub audience: Audience,
    /// Whom to ask to respond to what the event said, each by a
    /// `ResponseRequested` in the event's contexts, after it.
    pub ask: Vec<ActorId>,
    /// Why those of `ask` are asked (issue 439), which their requests
    /// carry: all of them for the same reason.
    pub reason: RequestReason,
    /// How many hops since the last message of a user the requests of
    /// `ask` carry (`EventContext::agent_hops`, issue 45): the moderator
    /// decides the chain, so it counts them.
    pub agent_hops: u32,
    /// What the moderator tells of its own, in the system's name, and to
    /// whom.
    pub notices: Vec<(Vec<ActorId>, Notice)>,
}

/// What a moderator may tell of its own (issue 418) - nothing else: what
/// it decides is whom, not what anyone is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Notice {
    /// A chain between agents reached its limit (issue 200): `message` made
    /// nobody respond, not even the agents `not_asked` it mentioned.
    ChainCutOff { message: MessageId, max_agent_hops: u32, not_asked: Vec<ActorId> },
    /// The policy could not choose who responds to `message` (issue 441), and nobody was asked.
    ResponderUndecided { message: MessageId },
}

impl From<Notice> for ActorEvent {
    fn from(notice: Notice) -> Self {
        match notice {
            Notice::ChainCutOff { message, max_agent_hops, not_asked } => {
                ActorEvent::ChainCutOff { message, max_agent_hops, not_asked }
            }
            Notice::ResponderUndecided { message } => ActorEvent::ResponderUndecided { message },
        }
    }
}

/// Whom a moderator asks to respond to a message that mentions no agent:
/// the slot of the `policy_moderator`. A channel has exactly one.
#[async_trait]
pub trait ResponderPolicy: Send + Sync + Debug {
    /// The agents of `candidates` to ask to respond to what `event` said - a message, or the
    /// messages of a turn -, by its sender and in its context, in which whatever the policy asks is
    /// asked. `messages` is the conversation, the oldest first: what was heard before, as far as
    /// the moderator keeps it, as it stands (issue 414), and last what `event` said; each names its
    /// sender. The candidates are the agents that are awake and did not say what `event` said, in
    /// no particular order, as they are described to everyone in the channel (issue 403).
    ///
    /// # Errors
    /// `Undecided` if the policy could not choose (issue 441), saying why without anything that was
    /// said - whom to ask then is the moderator's to decide.
    async fn choose(
        &self,
        event: &Envelope,
        messages: &[Message],
        candidates: Vec<ActorDescription>,
    ) -> Result<Vec<ActorId>, Undecided>;
}

/// Why a `ResponderPolicy` could not choose whom to ask (issue 441): its words for the log, at
/// `warn!`, which no participant is told - so they hold nothing of what was said.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Undecided(pub String);

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
