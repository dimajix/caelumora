pub use crate::channel::{Channel, ChannelId, ChannelState, Participant};
use crate::meta::Metadata;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::store::{StateStore, TemplateStore};
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
    state_store: Arc<dyn StateStore>,
}

impl SessionController {
    pub fn new(
        session: Session,
        event_sender: Arc<dyn EventSender<SessionUpdateEvent>>,
        state_store: Arc<dyn StateStore>
    ) -> anyhow::Result<Self> {
        Ok(Self {
            session: ArcSwap::from_pointee(session),
            event_sender,
            state_store,
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
                let session = Arc::new(session);
                self.session.store(session.clone());
                if let Err(e) = self.state_store.update_session(&session) {
                    log::error!("Failed to persist session update: {}", e);
                }
            }
        }
    }
}

define_id!(SessionId);

pub struct SessionContext {
    pub id: SessionId,
    pub owner: UserId,
    pub security: SecurityContext,
    pub state_store: Arc<dyn StateStore>,
    pub template_store: Arc<dyn TemplateStore>,
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
