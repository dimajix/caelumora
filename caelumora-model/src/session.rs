use crate::agent::Agent;
use crate::message::{MessageHistory};
use crate::meta::Metadata;
use chrono::{DateTime, Utc};
use derive_more::Display;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::store::EntityResolver;
use crate::user::UserId;
use std::sync::Arc;
use arc_swap::ArcSwap;
use crate::event::EventSender;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChannelUpdateEvent {
    NameChanged(String),
}

#[derive(Debug)]
pub struct ChannelController {
    channel: ArcSwap<Channel>,
    event_sender: Arc<dyn EventSender<ChannelUpdateEvent>>,
}

impl ChannelController {
    pub fn new(channel: Channel, event_sender: Arc<dyn EventSender<ChannelUpdateEvent>>) -> Self {
        Self {
            channel: ArcSwap::from_pointee(channel),
            event_sender,
        }
    }

    pub fn load(&self) -> arc_swap::Guard<Arc<Channel>> {
        self.channel.load()
    }

    pub fn id(&self) -> ChannelId {
        self.channel.load().id
    }

    pub fn name(&self) -> String {
        self.channel.load().name.clone()
    }

    pub fn update_name(&self, new_name: String) {
        self.apply_update(ChannelUpdateEvent::NameChanged(new_name));
    }

    pub fn apply_update(&self, event: ChannelUpdateEvent) {
        self.process_update(&event);
        self.event_sender.send(event);
    }

    pub fn process_update(&self, event: &ChannelUpdateEvent) {
        match event {
            ChannelUpdateEvent::NameChanged(name) => {
                let mut channel = (**self.channel.load()).clone();
                channel.name = name.clone();
                self.channel.store(Arc::new(channel));
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionUpdateEvent {
    NameChanged(String),
}

#[derive(Debug)]
pub struct SessionController {
    session: ArcSwap<Session>,
    event_sender: Arc<dyn EventSender<SessionUpdateEvent>>,
}

impl SessionController {
    pub fn new(session: Session, event_sender: Arc<dyn EventSender<SessionUpdateEvent>>) -> Self {
        Self {
            session: ArcSwap::from_pointee(session),
            event_sender,
        }
    }

    pub fn load(&self) -> arc_swap::Guard<Arc<Session>> {
        self.session.load()
    }

    pub fn id(&self) -> SessionId {
        self.session.load().id
    }

    pub fn name(&self) -> String {
        self.session.load().metadata.name.clone()
    }

    pub fn update_name(&self, new_name: String) {
        self.apply_update(SessionUpdateEvent::NameChanged(new_name));
    }

    fn apply_update(&self, event: SessionUpdateEvent) {
        self.process_update(&event);
        self.event_sender.send(event);
    }

    pub fn process_update(&self, event: &SessionUpdateEvent) {
        match event {
            SessionUpdateEvent::NameChanged(name) => {
                let mut session = (**self.session.load()).clone();
                session.metadata.name = name.clone();
                self.session.store(Arc::new(session));
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Display, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub Uuid);

pub struct SessionContext {
    pub id: SessionId,
    pub owner: UserId,
    pub resolver: Box<dyn EntityResolver>
}

#[derive(Debug, Clone, Copy, Default, Display, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChannelId(pub Uuid);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    pub name: String,
    pub moderator: String,
    pub participants: Vec<Participant>,
}

impl Channel {
    pub fn new(name: String, moderator: String, participants: Vec<Participant>) -> Self {
        let id = ChannelId(Uuid::new_v4());
        Self {
            id,
            name,
            moderator,
            participants
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub metadata: Metadata,
    pub channels: Vec<Channel>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionState {
    pub channels: std::collections::HashMap<ChannelId, ChannelState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Participant {
    User,
    Agent(Agent)
}

