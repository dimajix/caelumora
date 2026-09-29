use async_trait::async_trait;
use std::fmt::Debug;
use std::sync::Arc;

use crate::actor::Actor;
use caelumora_model::actor::{ActorId, Envelope};

#[async_trait]
pub trait Moderator : Send + Sync + Debug {
    async fn route(&mut self, event: &Envelope) -> RouteDecision;
    fn add_actor(&mut self, actor: Arc<dyn Actor>) -> ();
    fn remove_actor(&mut self, actor: ActorId) -> ();
}

pub struct RouteDecision {
    pub forwards: Vec<(ActorId, Envelope)>,
    /// Whether this event should also be broadcast to external subscribers (e.g. the UI).
    /// Only approval events set this explicitly, since they're the only ones relayed through
    /// multiple internal hops (Agent -> Approver -> User) where the intermediate hop must stay
    /// internal; everything else defaults to `true`.
    pub broadcast: bool,
}

impl From<Vec<(ActorId, Envelope)>> for RouteDecision {
    fn from(forwards: Vec<(ActorId, Envelope)>) -> Self {
        Self { forwards, broadcast: true }
    }
}

