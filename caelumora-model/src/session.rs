pub use crate::channel::{Channel, ChannelId, ChannelState, Participant};
use crate::component::ComponentConfig;
use crate::event::{EventPublisher, GlobalEvent};
use crate::meta::Metadata;
use crate::security::{Secured, SecurityAttributes, SecurityContext};
use crate::store::{EntityStore, StoreResult};
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

    /// Renames the session on behalf of `ctx`: stored first - which is where
    /// the permission is checked (issue 11) - and only then changed live and
    /// announced.
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::PermissionDenied` included;
    /// the session is then unchanged.
    pub async fn update_name(&self, ctx: &SecurityContext, new_name: String) -> StoreResult<()> {
        let event = SessionUpdateEvent::NameChanged(new_name);
        let session = self.apply(&event);
        self.entity_store.update_session(ctx, &session).await?;
        let attributes = session.security_attributes.clone();
        self.session.store(Arc::new(session));
        self.event_publisher.publish(GlobalEvent::SessionUpdated(self.id, event), attributes);
        Ok(())
    }

    /// The session with `event` applied, not yet stored anywhere.
    fn apply(&self, event: &SessionUpdateEvent) -> Session {
        let mut session = (**self.session.load()).clone();
        match event {
            SessionUpdateEvent::NameChanged(name) => session.metadata.name = name.clone(),
        }
        session
    }

    /// Takes over an update made elsewhere - already stored there.
    fn process_session_update(&self, event: &SessionUpdateEvent) {
        self.session.store(Arc::new(self.apply(event)));
    }

    pub async fn process_global_event(&self, event: &GlobalEvent) {
        match event {
            GlobalEvent::SessionUpdated(sid, update) if *sid == self.id => {
                self.process_session_update(update);
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

impl Secured for Session {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionState {
    pub channels: std::collections::HashMap<ChannelId, ChannelState>,
}
