pub use crate::channel::{Channel, ChannelId, ChannelState, Participant};
use crate::component::ComponentConfig;
use crate::meta::Metadata;
use crate::privacy::PrivacyConfig;
use crate::security::{Secured, SecurityAttributes};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionUpdateEvent {
    NameChanged(String),
    /// The session's privacy policy changed (issue 134).
    PrivacyChanged(PrivacyConfig),
}

define_id!(SessionId);


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
    pub commands: ComponentConfig,
    /// Whom the session lets read what was said in it (issue 134).
    #[serde(default)]
    pub privacy: PrivacyConfig,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub security_attributes: SecurityAttributes,
}

impl Secured for Session {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

