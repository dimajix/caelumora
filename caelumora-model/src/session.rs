pub use crate::channel::{Channel, ChannelId, ChannelState, Participant};
use crate::approval::ApprovalRule;
use crate::component::ComponentConfig;
use crate::privacy::PrivacyConfig;
use crate::actor::ActorId;
use crate::group::GroupId;
use crate::security::{Grant, Secured, SecurityAttributes, Subject};
use crate::user::UserId;
use crate::store::query::Field;
use chrono::{DateTime, Utc};
use crate::value::name::Name;
use crate::value::text::{Briefing, Summary};
use crate::value::ValueError;
use crate::workspace::WorkspaceId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionUpdateEvent {
    /// Renamed, as it was asked: the name is made from it as the change is
    /// applied (issue 248), as a description is.
    NameChanged(String),
    /// The session's privacy policy changed (issue 134).
    PrivacyChanged(PrivacyConfig),
    /// Its rules on tool calls changed (issue 13), as they now stand.
    ApprovalRulesChanged(Vec<ApprovalRule>),
    /// What it is for changed (issue 245), as it now stands.
    DescriptionChanged { summary: Option<String>, briefing: Option<String> },
    /// A workspace was attached, or the one attached taken away (issue
    /// 253), as it now stands.
    WorkspaceChanged(Option<WorkspaceId>),
}

define_id!(SessionId);


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    /// What it is called (issue 248): a name it could be looked up by.
    pub name: Name,
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
    /// The workspace its people and agents share in all its channels (issue
    /// 253), at most one; attached by whoever may change the session. Which
    /// of its channels may use it, the workspace's classification decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub security_attributes: SecurityAttributes,
    /// Set when it was deleted (issue 164): it stays stored, hidden from
    /// every read but those for what was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<crate::removal::Removal>,
}

crate::impl_deletable!(Session);

impl Session {
    /// The attributes of a new session `id` of `owner`: read by its
    /// members, whoever they are at the time (issue 23) - the group of its
    /// participants, stored with it (`Group::of_session`).
    pub fn attributes_of(id: SessionId, owner: UserId) -> SecurityAttributes {
        SecurityAttributes::owned_by(ActorId::User(owner)).granting(Subject::Group(GroupId::of_session(id)), Grant::Read)
    }

    /// Says what the session is for (issue 245): `summary` and `briefing`
    /// replace what it said - a missing or empty one removes it.
    ///
    /// # Errors
    /// `ValueError` for the first that does not fit; the session is then
    /// unchanged.
    pub fn describe(&mut self, summary: Option<&str>, briefing: Option<&str>) -> Result<(), ValueError> {
        (self.summary, self.briefing) = (Summary::optional(summary)?, Briefing::optional(briefing)?);
        Ok(())
    }
}

/// What a query of sessions may name (issue 293). `Participant` is an
/// element of `Participants`, the whole of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionField {
    /// Its participants.
    Participants,
    /// A participant, within `Participants`.
    Participant,
}

impl Field for SessionField {
    fn path(self) -> &'static [&'static str] {
        match self {
            SessionField::Participants => &["participants"],
            SessionField::Participant => &[],
        }
    }
}

impl Secured for Session {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

