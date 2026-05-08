use std::collections::HashMap;
use std::sync::Mutex;
use chrono::{DateTime, Utc};
use tokio::sync::mpsc;
use uuid::Uuid;


#[derive(Debug, Clone)]
pub struct AgentContextChangePayload {}
#[derive(Debug, Clone)]
pub struct AgentMemoryChangePayload {}


#[derive(Debug, Clone)]
pub enum Event {
    AgentContextChanged(AgentContextChangePayload),
    AgentMemoryChanged(AgentMemoryChangePayload),
}

#[derive(Debug, Clone)]
pub struct EventEnvelope {
    pub origin_session_id: Option<Uuid>,
    pub origin_agent_id: Option<Uuid>,
    pub timestamp: DateTime<Utc>,
    pub event: Event
}


#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SubscriptionTopic {
    All,               // Global broadcast
    SpecificAgent(Uuid), // Only events from this agent
}


pub struct EventBus {
    subscribers: Mutex<HashMap<SubscriptionTopic, Vec<mpsc::Sender<EventEnvelope>>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self { subscribers: Mutex::new(HashMap::new()) }
    }

    pub fn subscribe(&self, topic: SubscriptionTopic) -> mpsc::Receiver<EventEnvelope> {
        let (tx, rx) = mpsc::channel(20);
        let mut subs = self.subscribers.lock().unwrap();

        subs.entry(topic).or_insert_with(Vec::new).push(tx);
        rx
    }

    pub fn publish(&self, origin_agent: Uuid, event: Event) {
        let envelope = EventEnvelope {
            timestamp: Utc::now(),
            origin_agent_id: Some(origin_agent),
            origin_session_id: None,
            event
        };
        let mut subs = self.subscribers.lock().unwrap();

        // 1. Identify all relevant topics:
        //    - The "All" topic always gets the message.
        //    - The "SpecificAgent(origin_agent)" topic gets the message.
        let topics_to_notify = vec![
            SubscriptionTopic::All,
            SubscriptionTopic::SpecificAgent(origin_agent)
        ];

        let mut require_cleanup = false;
        for topic in topics_to_notify {
            if let Some(senders) = subs.get_mut(&topic) {
                // Clean up dead subscribers while notifying
                senders.retain(|tx|
                    match tx.try_send(envelope.clone()) {
                        Ok(_) => true, // Success, keep the subscriber
                        Err(e) => {
                            match e {
                                mpsc::error::TrySendError::Full(_) => {
                                    // The channel is full. We keep the subscriber,
                                    // but we accept that this specific message was dropped.
                                    true
                                }
                                mpsc::error::TrySendError::Closed(_) => {
                                    // The receiver is gone! Return false to remove from Vec.
                                    false
                                }
                            }
                        }
                    }
                );
                if senders.is_empty() {
                    require_cleanup = true;
                }
            }
        }

        if require_cleanup {
            // We use HashMap::retain which allows us to remove the entire
            // Topic entry if the Vec of senders has become empty.
            subs.retain(|_topic, senders| !senders.is_empty());
        }
    }
}