use crate::agent::Agent;
use crate::approval::Approver;
use crate::event::{EventPublisher, GlobalEvent};
use crate::message::MessageHistory;
use crate::security::SecurityAttributes;
use crate::session::SessionId;
use crate::user::User;
use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChannelUpdateEvent {
    NameChanged(String),
}

#[derive(Debug)]
pub struct ChannelController {
    id: ChannelId,
    session_id: SessionId,
    channel: ArcSwap<Channel>,
    event_publisher: Arc<dyn EventPublisher>,
}

impl ChannelController {
    pub fn new(channel: Channel, event_publisher: Arc<dyn EventPublisher>) -> Self {
        let id = channel.id;
        let session_id = channel.session_id;
        Self {
            id,
            session_id,
            channel: ArcSwap::from_pointee(channel),
            event_publisher,
        }
    }

    pub fn load(&self) -> arc_swap::Guard<Arc<Channel>> {
        self.channel.load()
    }

    pub fn id(&self) -> ChannelId {
        self.id
    }

    pub fn name(&self) -> String {
        self.channel.load().name.clone()
    }

    pub async fn update_name(&self, new_name: String) {
        let event = ChannelUpdateEvent::NameChanged(new_name);
        self.process_channel_update(&event).await;
        self.event_publisher.publish(GlobalEvent::ChannelUpdated(self.session_id, self.id, event));
    }

    async fn process_channel_update(&self, event: &ChannelUpdateEvent) {
        match event {
            ChannelUpdateEvent::NameChanged(name) => {
                let mut channel = (**self.channel.load()).clone();
                channel.name = name.clone();
                self.channel.store(Arc::new(channel));
            }
        }
    }

    pub async fn process_global_event(&self, event: &GlobalEvent) {
        match event {
            GlobalEvent::ChannelUpdated(_, cid, update) if *cid == self.id => {
                self.process_channel_update(update).await;
            }
            GlobalEvent::AgentChannelJoined(sid, cid, aid) if *cid == self.id => {
                // TODO: Update internal session state
            }
            GlobalEvent::AgentChannelLeft(sid, cid, aid) if *cid == self.id => {
                // TODO: Update internal session state
            }
            _ => {}
        }
    }
}

define_id!(ChannelId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    pub session_id: SessionId,
    pub name: String,
    pub moderator: String,
    pub participants: Vec<Participant>,
    pub security_attributes: SecurityAttributes,
}

impl Channel {
    pub fn new(session_id: SessionId, name: String, moderator: String, participants: Vec<Participant>, security_attributes: SecurityAttributes) -> Self {
        let id = ChannelId(Uuid::new_v4());
        Self {
            id,
            session_id,
            name,
            moderator,
            participants,
            security_attributes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelState {
    pub channel_id: ChannelId,
    pub history: MessageHistory,
}

impl ChannelState {
    pub fn new(channel_id: ChannelId) -> Self {
        Self {
            channel_id,
            history: MessageHistory::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Participant {
    User(User),
    Agent(Agent),
    Approver(Approver),
}
