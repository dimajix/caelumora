use caelumora_model::agent::AgentId;
use caelumora_model::channel::ChannelId;
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::session::SessionId;
use caelumora_model::user::UserId;
use dashmap::DashMap;
use log::warn;
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SubscriptionTopic {
    All,
    Session(SessionId),
    Channel(ChannelId),
    Agent(AgentId),
    User(UserId),
}

#[derive(Debug)]
pub struct EventBus {
    subscribers: DashMap<SubscriptionTopic, Vec<mpsc::Sender<GlobalEvent>>>,
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

    pub fn subscribe(&self, topics: Vec<SubscriptionTopic>) -> mpsc::Receiver<GlobalEvent> {
        let (tx, rx) = mpsc::channel(100);

        for topic in topics {
            self.subscribers.entry(topic).or_default().push(tx.clone());
        }
        rx
    }

    pub fn subscribe_all(&self) -> mpsc::Receiver<GlobalEvent> {
        self.subscribe(vec![SubscriptionTopic::All])
    }

    pub fn subscribe_session(&self, id: SessionId) -> mpsc::Receiver<GlobalEvent> {
        self.subscribe(vec![SubscriptionTopic::Session(id)])
    }

    pub fn subscribe_channel(&self, id: ChannelId) -> mpsc::Receiver<GlobalEvent> {
        self.subscribe(vec![SubscriptionTopic::Channel(id)])
    }

    pub fn subscribe_agent(&self, id: AgentId) -> mpsc::Receiver<GlobalEvent> {
        self.subscribe(vec![SubscriptionTopic::Agent(id)])
    }

    pub fn subscribe_user(&self, id: UserId) -> mpsc::Receiver<GlobalEvent> {
        self.subscribe(vec![SubscriptionTopic::User(id)])
    }

    pub fn publish(&self, event: GlobalEvent) {
        let mut topics_to_notify = vec![SubscriptionTopic::All];

        match &event {
            GlobalEvent::SessionCreated(sid) | GlobalEvent::SessionUpdated(sid, _) | GlobalEvent::SessionDeleted(sid) => {
                topics_to_notify.push(SubscriptionTopic::Session(*sid));
            }
            GlobalEvent::ChannelCreated(sid, cid) | GlobalEvent::ChannelUpdated(sid, cid, _) | GlobalEvent::ChannelDeleted(sid, cid) => {
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
                senders.retain(|tx: &mpsc::Sender<GlobalEvent>| {
                    match tx.try_send(event.clone()) {
                        Ok(_) => true,
                        Err(e) => {
                            match e {
                                mpsc::error::TrySendError::Full(_) => {
                                    // The channel is full. We keep the subscriber,
                                    // but we accept that this specific message was dropped.
                                    warn!("Event dropped due to full buffer for topic {:?}", topic);
                                    true
                                }
                                mpsc::error::TrySendError::Closed(_) => {
                                    // The receiver is gone! Return false to remove from Vec.
                                    false
                                }
                            }
                        }
                    }
                });
            }
        }

        // Periodic cleanup of empty topics could be added here, but DashMap doesn't make it as easy as retain on Mutex<HashMap>.
        // For now, let's keep it simple. Empty Vecs will remain in the DashMap.
    }
}