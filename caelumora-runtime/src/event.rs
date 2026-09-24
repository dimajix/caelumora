use caelumora_model::agent::AgentId;
use caelumora_model::channel::ChannelId;
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::session::SessionId;
use caelumora_model::user::UserId;
use dashmap::DashMap;
use tracing::warn;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SubscriptionTopic {
    All,
    Session(SessionId),
    Channel(ChannelId),
    Agent(AgentId),
    User(UserId),
}

/// How many events a subscriber may fall behind before it loses the oldest.
/// A power of two, because a broadcast channel rounds its capacity up to one.
const SUBSCRIPTION_CAPACITY: usize = 128;

#[derive(Debug)]
pub struct EventBus {
    // One sender per subscription, registered under each of its topics. A
    // broadcast channel rather than an mpsc one: when a subscriber falls
    // behind, it is its receiver that learns so (`Lagged`), not only the log
    // (issue 38).
    subscribers: DashMap<SubscriptionTopic, Vec<broadcast::Sender<GlobalEvent>>>,
}

/// The events of an `EventBus` subscription.
///
/// A subscriber that falls more than `SUBSCRIPTION_CAPACITY` events behind
/// loses the oldest ones. `recv` notes that in the log and goes on, for
/// subscribers that have no way to catch up; one that can - a stream that
/// sends its client a fresh snapshot - takes the receiver with
/// `into_receiver` and handles `RecvError::Lagged` itself.
#[derive(Debug)]
pub struct EventSubscription {
    rx: broadcast::Receiver<GlobalEvent>,
}

impl EventSubscription {
    /// The next event, or `None` once the bus is gone. Lost events are
    /// logged and skipped.
    pub async fn recv(&mut self) -> Option<GlobalEvent> {
        loop {
            match self.rx.recv().await {
                Ok(event) => return Some(event),
                Err(RecvError::Lagged(lost)) => warn!("An event bus subscriber fell behind and lost {} events", lost),
                Err(RecvError::Closed) => return None,
            }
        }
    }

    /// The next event if one is waiting, like `recv`.
    pub fn try_recv(&mut self) -> Option<GlobalEvent> {
        loop {
            match self.rx.try_recv() {
                Ok(event) => return Some(event),
                Err(TryRecvError::Lagged(lost)) => warn!("An event bus subscriber fell behind and lost {} events", lost),
                Err(TryRecvError::Empty | TryRecvError::Closed) => return None,
            }
        }
    }

    /// The underlying receiver, for a subscriber that handles lost events
    /// itself.
    pub fn into_receiver(self) -> broadcast::Receiver<GlobalEvent> {
        self.rx
    }
}

impl EventPublisher for EventBus {
    fn publish(&self, event: GlobalEvent) {
        self.publish(event);
    }
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            subscribers: DashMap::new(),
        }
    }

    pub fn subscribe(&self, topics: Vec<SubscriptionTopic>) -> EventSubscription {
        let (tx, rx) = broadcast::channel(SUBSCRIPTION_CAPACITY);

        for topic in topics {
            self.subscribers.entry(topic).or_default().push(tx.clone());
        }
        EventSubscription { rx }
    }

    pub fn subscribe_all(&self) -> EventSubscription {
        self.subscribe(vec![SubscriptionTopic::All])
    }

    pub fn subscribe_session(&self, id: SessionId) -> EventSubscription {
        self.subscribe(vec![SubscriptionTopic::Session(id)])
    }

    pub fn subscribe_channel(&self, id: ChannelId) -> EventSubscription {
        self.subscribe(vec![SubscriptionTopic::Channel(id)])
    }

    pub fn subscribe_agent(&self, id: AgentId) -> EventSubscription {
        self.subscribe(vec![SubscriptionTopic::Agent(id)])
    }

    pub fn subscribe_user(&self, id: UserId) -> EventSubscription {
        self.subscribe(vec![SubscriptionTopic::User(id)])
    }

    pub fn publish(&self, event: GlobalEvent) {
        let mut topics_to_notify = vec![SubscriptionTopic::All];

        match &event {
            GlobalEvent::SessionCreated(sid) | GlobalEvent::SessionUpdated(sid, _) | GlobalEvent::SessionDeleted(sid) |
            GlobalEvent::SessionStarted(sid) | GlobalEvent::SessionStopped(sid) => {
                topics_to_notify.push(SubscriptionTopic::Session(*sid));
            }
            GlobalEvent::ChannelCreated(sid, cid) | GlobalEvent::ChannelUpdated(sid, cid, _) | GlobalEvent::ChannelDeleted(sid, cid) |
            GlobalEvent::ChannelStarted(sid, cid) | GlobalEvent::ChannelStopped(sid, cid) => {
                topics_to_notify.push(SubscriptionTopic::Session(*sid));
                topics_to_notify.push(SubscriptionTopic::Channel(*cid));
            }
            GlobalEvent::AgentCreated(sid, aid) | GlobalEvent::AgentUpdated(sid, aid, _) | GlobalEvent::AgentDeleted(sid, aid) => {
                topics_to_notify.push(SubscriptionTopic::Session(*sid));
                //topics_to_notify.push(SubscriptionTopic::Channel(*cid));
                topics_to_notify.push(SubscriptionTopic::Agent(*aid));
            }
            GlobalEvent::UserCreated(uid) | GlobalEvent::UserUpdated(uid, _) | GlobalEvent::UserDeleted(uid) => {
                topics_to_notify.push(SubscriptionTopic::User(*uid));
            }
            GlobalEvent::AgentChannelJoined(sid, cid, _) | GlobalEvent::AgentChannelLeft(sid, cid, _) |
            GlobalEvent::UserChannelJoined(sid, cid, _) | GlobalEvent::UserChannelLeft(sid, cid, _) => {
                topics_to_notify.push(SubscriptionTopic::Session(*sid));
                topics_to_notify.push(SubscriptionTopic::Channel(*cid));
            }
        }

        for topic in topics_to_notify {
            if let Some(mut senders) = self.subscribers.get_mut(&topic) {
                // A full channel does not fail the send: it drops the
                // subscriber's oldest event and tells its receiver. A send
                // fails only when the receiver is gone, and so does its
                // subscription.
                senders.retain(|tx| tx.send(event.clone()).is_ok());
            }
        }

        // Periodic cleanup of empty topics could be added here, but DashMap doesn't make it as easy as retain on Mutex<HashMap>.
        // For now, let's keep it simple. Empty Vecs will remain in the DashMap.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn created() -> GlobalEvent {
        GlobalEvent::SessionCreated(SessionId::new())
    }

    #[tokio::test]
    async fn a_subscriber_that_falls_behind_is_told_and_keeps_receiving() {
        let bus = EventBus::new();
        let mut receiver = bus.subscribe_all().into_receiver();
        for _ in 0..SUBSCRIPTION_CAPACITY + 5 {
            bus.publish(created());
        }

        assert!(matches!(receiver.recv().await, Err(RecvError::Lagged(5))), "the loss reaches the subscriber");
        assert!(receiver.recv().await.is_ok(), "and the newest events are still there");
    }

    #[tokio::test]
    async fn a_subscription_skips_what_it_lost_and_goes_on() {
        let bus = EventBus::new();
        let mut subscription = bus.subscribe_all();
        for _ in 0..SUBSCRIPTION_CAPACITY + 5 {
            bus.publish(created());
        }
        let deleted = SessionId::new();
        bus.publish(GlobalEvent::SessionDeleted(deleted));

        let mut received = 0;
        while let Some(event) = subscription.try_recv() {
            received += 1;
            if matches!(event, GlobalEvent::SessionDeleted(id) if id == deleted) {
                break;
            }
        }
        assert_eq!(received, SUBSCRIPTION_CAPACITY, "the oldest events are lost, the rest arrives in order");
    }

    #[tokio::test]
    async fn a_dropped_subscription_is_removed() {
        let bus = EventBus::new();
        drop(bus.subscribe_all());
        bus.publish(created());
        assert!(bus.subscribers.get(&SubscriptionTopic::All).unwrap().is_empty());
    }
}
