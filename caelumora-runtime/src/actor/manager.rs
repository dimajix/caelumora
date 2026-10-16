//! What of an actor outlives its run (issue 386): the outbox it shows its
//! user what happens in a channel through, which whoever follows the
//! channel keeps while the channel is stopped, and which the actor of the
//! next run shows to again.

use caelumora_model::actor::{ActorId, Envelope};
use caelumora_model::channel::ChannelId;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use tokio::sync::broadcast;
use tokio::sync::broadcast::WeakSender;

/// How many items a follower of an actor may fall behind before it loses
/// the oldest. A power of two, since a broadcast rounds up to one (as the
/// `EventBus`, issue 38).
pub(crate) const OUTBOX_CAPACITY: usize = 128;

/// What an actor shows whoever follows it (issue 386): what it is routed
/// and says, and where its runs begin and end - in the order it happened,
/// so that what a run showed comes after its start and before its end.
#[derive(Debug, Clone)]
#[expect(clippy::large_enum_variant, reason = "nearly every item is an envelope; boxing it would cost every one an allocation")]
pub enum Shown {
    /// A run of the actor began: the channel runs.
    Started,
    /// What the actor was routed or said.
    Envelope(Envelope),
    /// The run ended, after everything it showed: the channel stopped, or
    /// the actor left it.
    Ended,
}

/// The one owner of what of an actor outlives its run (issue 386). Kept
/// by the `RuntimeManager`, since a session - and with #381 a channel - is
/// unloaded while someone follows it.
#[derive(Debug, Default)]
pub struct ActorManager {
    /// The outbox of each actor of each channel, held by its actor and by
    /// whoever follows it: gone once none of them holds it.
    outboxes: Mutex<HashMap<(ChannelId, ActorId), WeakSender<Shown>>>,
}

impl ActorManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// The outbox of `actor` in `channel`: the same for every run of it and
    /// everyone following it, as long as one of them holds it.
    pub(crate) fn outbox(&self, channel: ChannelId, actor: ActorId) -> broadcast::Sender<Shown> {
        let mut outboxes = self.outboxes.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(outbox) = outboxes.get(&(channel, actor)).and_then(WeakSender::upgrade) {
            return outbox;
        }
        outboxes.retain(|_, outbox| outbox.strong_count() > 0);
        let outbox = broadcast::channel(OUTBOX_CAPACITY).0;
        outboxes.insert((channel, actor), outbox.downgrade());
        outbox
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::user::UserId;

    #[test]
    fn an_outbox_is_shared_while_held_and_gone_after() {
        let manager = ActorManager::new();
        let (channel, actor) = (ChannelId::new(), ActorId::User(UserId::new()));
        let held = manager.outbox(channel, actor);
        let mut following = held.subscribe();
        manager.outbox(channel, actor).send(Shown::Started).unwrap();
        assert!(matches!(following.try_recv(), Ok(Shown::Started)), "one outbox for the actor and whoever follows it");
        assert_eq!(manager.outbox(ChannelId::new(), actor).receiver_count(), 0, "one per channel");

        drop((held, following));
        assert_eq!(manager.outbox(channel, actor).receiver_count(), 0, "once nobody holds it, a new one");
        assert_eq!(manager.outboxes.lock().unwrap().len(), 1, "and the others are forgotten");
    }
}
