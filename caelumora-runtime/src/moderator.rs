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
pub(crate) trait Moderator : Send + Sync + Debug {
    /// Whom `event` is delivered to, and as what: the event itself, or one
    /// of the moderator's own, such as a request to respond. What a user
    /// sees is what is routed to their actor (issue 220).
    async fn route(&self, event: &Envelope) -> Vec<(ActorId, Envelope)>;
    fn add_actor(&self, actor: Arc<dyn Actor>);
    fn remove_actor(&self, actor: ActorId);
}
