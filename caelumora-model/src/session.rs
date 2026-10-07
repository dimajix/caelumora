pub use crate::channel::{Channel, ChannelId, ChannelState, Participant};
use crate::approval::ApprovalRule;
use crate::component::ComponentConfig;
use crate::privacy::PrivacyConfig;
use crate::security::{Secured, SecurityAttributes};
use chrono::{DateTime, Utc};
use crate::value::text::{Briefing, Summary};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionUpdateEvent {
    NameChanged(String),
    /// The session's privacy policy changed (issue 134).
    PrivacyChanged(PrivacyConfig),
    /// Its rules on tool calls changed (issue 13), as they now stand.
    ApprovalRulesChanged(Vec<ApprovalRule>),
    /// What it is for changed (issue 245), as it now stands.
    DescriptionChanged { summary: Option<Summary>, briefing: Option<Briefing> },
}

define_id!(SessionId);


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub name: String,
    /// What it is for, in a line (issue 245); `None` if nobody said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
    /// What an agent should know before it works in any of
    /// the session's channels (issue 245), given to
    /// the agents whose personality wires in the
    /// `channel_briefing_prompt_provider`; `None` if nobody wrote one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub briefing: Option<Briefing>,
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
    /// What the session restricts of the tool calls made in any of its
    /// channels (issue 13): rules that ask or deny, copied from its
    /// template when it was created and set by whoever may change it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_rules: Vec<ApprovalRule>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub security_attributes: SecurityAttributes,
    /// Set when it was deleted (issue 164): it stays stored, hidden from
    /// every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<crate::removal::Removal>,
}

crate::impl_deletable!(Session);

impl Secured for Session {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

