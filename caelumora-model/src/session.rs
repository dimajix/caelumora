pub use crate::channel::{Channel, ChannelId, ChannelState, Participant};
use crate::meta::Metadata;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::store::EntityStore;
use crate::user::UserId;
use std::sync::Arc;
use arc_swap::ArcSwap;
use crate::event::EventSender;
use crate::security::SecurityContext;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionUpdateEvent {
    NameChanged(String),
}

#[derive(Debug)]
pub struct SessionController {
    session: ArcSwap<Session>,
    event_sender: Arc<dyn EventSender<SessionUpdateEvent>>,
    entity_store: Arc<dyn EntityStore>,
}

impl SessionController {
    pub async fn new(
        session: Session,
        event_sender: Arc<dyn EventSender<SessionUpdateEvent>>,
        entity_store: Arc<dyn EntityStore>
    ) -> anyhow::Result<Self> {
        Ok(Self {
            session: ArcSwap::from_pointee(session),
            event_sender,
            entity_store,
        })
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

    pub async fn update_name(&self, new_name: String) {
        self.apply_update(SessionUpdateEvent::NameChanged(new_name)).await;
    }

    async fn apply_update(&self, event: SessionUpdateEvent) {
        self.process_update(&event).await;
        self.event_sender.send(event);
    }

    pub async fn process_update(&self, event: &SessionUpdateEvent) {
        match event {
            SessionUpdateEvent::NameChanged(name) => {
                let mut session = (**self.session.load()).clone();
                session.metadata.name = name.clone();
                let session = Arc::new(session);
                self.session.store(session.clone());
                let ctx = SecurityContext::default();
                self.entity_store.update_session(&ctx, &session).await.expect("failed to update session");
            }
        }
    }
}

define_id!(SessionId);

pub struct SessionContext {
    pub id: SessionId,
    pub owner: UserId,
    pub security: SecurityContext,
    pub entity_store: Arc<dyn EntityStore>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub metadata: Metadata,
    pub channels: Vec<Channel>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionState {
    pub channels: std::collections::HashMap<ChannelId, ChannelState>,
}
