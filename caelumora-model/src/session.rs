pub use crate::channel::{Channel, ChannelId, ChannelState, Participant};
use crate::component::ComponentConfig;
use crate::meta::Metadata;
use crate::security::{Secured, SecurityAttributes, SecurityContext};
use crate::store::EntityStore;
use crate::user::UserId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionUpdateEvent {
    NameChanged(String),
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
    /// The session's channels by id, in order; each is a document of its own
    /// (issue 70).
    pub channels: Vec<ChannelId>,
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
