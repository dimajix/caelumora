use async_trait::async_trait;
use std::fmt::Debug;
use std::sync::Arc;

use crate::actor::Actor;
use caelumora_model::actor::{ActorId, Envelope};

/// Decides how the events of a channel are routed and who is asked to
/// respond. A component of the channel, of which it has exactly one (issue
/// 174); shared like every facet, so it keeps what it changes behind its own
/// lock.
#[async_trait]
pub trait Moderator : Send + Sync + Debug {
    async fn route(&self, event: &Envelope) -> RouteDecision;
    fn add_actor(&self, actor: Arc<dyn Actor>);
    fn remove_actor(&self, actor: ActorId);
}

pub struct RouteDecision {
    pub forwards: Vec<(ActorId, Envelope)>,
    /// Whether this event should also be broadcast to external subscribers (e.g. the UI).
    /// `false` only for the internal hops of an approval (agent -> approver, approver ->
    /// agent), which must not show; everything else - messages, the approver's request to
    /// the users, `TurnFailed`, `CommandExecuted`, `ParticipantRemoved` - is
    /// broadcast.
    pub broadcast: bool,
}

impl From<Vec<(ActorId, Envelope)>> for RouteDecision {
    fn from(forwards: Vec<(ActorId, Envelope)>) -> Self {
        Self { forwards, broadcast: true }
    }
}

