pub use crate::channel::{Channel, ChannelId, ChannelState, Participant};
use crate::component::ComponentConfig;
use crate::event::{EventPublisher, GlobalEvent};
use crate::meta::Metadata;
use crate::security::{SecurityAttributes, SecurityContext};
use crate::store::EntityStore;
use crate::user::UserId;
use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionUpdateEvent {
    NameChanged(String),
}

#[derive(Debug)]
pub struct SessionController {
    id: SessionId,
    session: ArcSwap<Session>,
    event_publisher: Arc<dyn EventPublisher>,
    entity_store: Arc<dyn EntityStore>,
}

impl SessionController {
    pub async fn new(
        session: Session,
        event_publisher: Arc<dyn EventPublisher>,
        entity_store: Arc<dyn EntityStore>
    ) -> anyhow::Result<Self> {
        let id = session.id;
        Ok(Self {
            id,
            session: ArcSwap::from_pointee(session),
            event_publisher,
            entity_store,
        })
    }

    pub fn load(&self) -> arc_swap::Guard<Arc<Session>> {
        self.session.load()
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn name(&self) -> String {
        self.session.load().metadata.name.clone()
    }

    pub async fn update_name(&self, new_name: String) {
        let event = SessionUpdateEvent::NameChanged(new_name);
        self.process_session_update(&event).await;
        self.event_publisher.publish(GlobalEvent::SessionUpdated(self.id, event));
    }

    async fn process_session_update(&self, event: &SessionUpdateEvent) {
        match event {
            SessionUpdateEvent::NameChanged(name) => {
                let mut session = (**self.session.load()).clone();
                session.metadata.name = name.clone();
                let session = Arc::new(session);
                self.session.store(session.clone());
                let ctx = SecurityContext::default();
                if let Err(e) = self.entity_store.update_session(&ctx, &session).await {
                    // The live name is already updated; only persisting it failed.
                    tracing::error!("Cannot persist new name of session {}: {}", session.id, e);
                }
            }
        }
    }

    pub async fn process_global_event(&self, event: &GlobalEvent) {
        match event {
            GlobalEvent::SessionUpdated(sid, update) if *sid == self.id => {
                self.process_session_update(update).await;
            }
            GlobalEvent::ChannelCreated(sid, cid) if *sid == self.id => {
                // TODO: Update internal session state to include the new channel
            }
            GlobalEvent::ChannelUpdated(sid, cid, update) if *sid == self.id => {
                // TODO: Update internal session state
            }
            GlobalEvent::ChannelDeleted(sid, cid) if *sid == self.id => {
                // TODO: Update internal session state
            }
            GlobalEvent::AgentCreated(sid,  aid) if *sid == self.id => {
                // TODO: Update internal session state
            }
            GlobalEvent::AgentUpdated(sid,  aid, update) if *sid == self.id => {
                // TODO: React to agent updates if necessary
            }
            GlobalEvent::AgentDeleted(sid,  aid) if *sid == self.id => {
                // TODO: Update internal session state
            }
            _ => {}
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
    pub participants: Vec<Participant>,
    pub channels: Vec<Channel>,
    /// The session's own user commands, copied from its template when it was
    /// created, so that a running session keeps its set (issue 28).
    #[serde(default)]
    pub commands: ComponentConfig,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub security_attributes: SecurityAttributes,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionState {
    pub channels: std::collections::HashMap<ChannelId, ChannelState>,
}
