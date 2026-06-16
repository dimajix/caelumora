use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::mpsc;
use caelumora_model::session::SessionId;
use caelumora_model::channel::ChannelId;
use caelumora_model::agent::AgentId;
use caelumora_model::user::UserId;
use crate::runtime::GlobalEvent;

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
    subscribers: Mutex<HashMap<SubscriptionTopic, Vec<mpsc::Sender<GlobalEvent>>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            subscribers: Mutex::new(HashMap::new()),
        }
    }

    pub fn subscribe(&self, topics: Vec<SubscriptionTopic>) -> mpsc::Receiver<GlobalEvent> {
        let (tx, rx) = mpsc::channel(100);
        let mut subs = self.subscribers.lock().unwrap();

        for topic in topics {
            subs.entry(topic).or_insert_with(Vec::new).push(tx.clone());
        }
        rx
    }

    pub fn publish(&self, event: GlobalEvent) {
        let mut subs = self.subscribers.lock().unwrap();

        let mut topics_to_notify = vec![SubscriptionTopic::All];

        match &event {
            GlobalEvent::SessionCreated(sid) | GlobalEvent::SessionUpdated(sid, _) | GlobalEvent::SessionDeleted(sid) => {
                topics_to_notify.push(SubscriptionTopic::Session(*sid));
            }
            GlobalEvent::ChannelCreated(sid, cid) | GlobalEvent::ChannelUpdated(sid, cid, _) | GlobalEvent::ChannelDeleted(sid, cid) => {
                topics_to_notify.push(SubscriptionTopic::Session(*sid));
                topics_to_notify.push(SubscriptionTopic::Channel(*cid));
            }
            GlobalEvent::AgentCreated(sid, cid, aid) | GlobalEvent::AgentUpdated(sid, cid, aid, _) | GlobalEvent::AgentDeleted(sid, cid, aid) => {
                topics_to_notify.push(SubscriptionTopic::Session(*sid));
                topics_to_notify.push(SubscriptionTopic::Channel(*cid));
                topics_to_notify.push(SubscriptionTopic::Agent(*aid));
            }
            GlobalEvent::UserCreated(uid) | GlobalEvent::UserUpdated(uid, _) | GlobalEvent::UserDeleted(uid) => {
                topics_to_notify.push(SubscriptionTopic::User(*uid));
            }
        }

        let mut require_cleanup = false;
        for topic in topics_to_notify {
            if let Some(senders) = subs.get_mut(&topic) {
                senders.retain(|tx: &mpsc::Sender<GlobalEvent>| {
                    match tx.try_send(event.clone()) {
                        Ok(_) => true,
                        Err(e) => {
                            match e {
                                mpsc::error::TrySendError::Full(_) => {
                                    // The channel is full. We keep the subscriber,
                                    // but we accept that this specific message was dropped.
                                    // In a production system, we might want more robust handling.
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
                if senders.is_empty() {
                    require_cleanup = true;
                }
            }
        }

        if require_cleanup {
            subs.retain(|_, senders: &mut Vec<mpsc::Sender<GlobalEvent>>| !senders.is_empty());
        }
    }
}