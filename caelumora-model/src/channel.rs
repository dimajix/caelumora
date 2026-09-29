use crate::actor::ActorId;
use crate::agent::AgentId;
use crate::approval::{ApprovalStrategy, Approver, ApproverId};
use crate::confidentiality::{ChannelClassification, Confidentiality};
use crate::message::{MessageHistory, MessageId};
use crate::security::{Secured, SecurityAttributes};
use crate::session::SessionId;
use crate::user::UserId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The moderator of a channel whose template or request names none.
pub const DEFAULT_MODERATOR: &str = "round_robin";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChannelUpdateEvent {
    NameChanged(String),
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
    /// How far the channel's contents may travel (issue 61); never
    /// `Private`. Set by its owner when creating it.
    pub confidentiality: Confidentiality,
    /// Where the view of the history begins for participants who joined
    /// later and may not see all of it (issue 46); everybody else sees all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history_limits: Vec<HistoryLimit>,
}

/// How much of a channel's history a new participant may see, as asked for
/// when adding them (issue 46). Data only: the `ChannelLoop` turns it into a
/// `HistoryStart` and applies it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryAccess {
    #[default]
    All,
    None,
    LastDays { count: u32 },
    LastMessages { count: u32 },
}

/// Where a participant's view of a channel's history begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryStart {
    /// From this message on.
    From(MessageId),
    /// After this message: what came before the participant joined is not
    /// visible.
    After(MessageId),
}

/// The view of the history granted to one participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryLimit {
    pub participant: ActorId,
    pub start: HistoryStart,
}

impl Secured for Channel {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl Channel {
    /// The channel as the confidentiality rules see it.
    pub fn classification(&self) -> ChannelClassification {
        ChannelClassification { session_id: self.session_id, channel_id: self.id, level: self.confidentiality }
    }

    /// A new channel of `participants`, owned by `owner`, as every channel is
    /// built - from a template or on request (issue 09): with an approver
    /// added if there is none, and with every user and agent in it as a
    /// contributor (see `participant_attributes`).
    pub fn build(session_id: SessionId, owner: ActorId, name: String, moderator: String, confidentiality: Confidentiality, mut participants: Vec<Participant>) -> Self {
        if !participants.iter().any(|p| matches!(p, Participant::Approver(_))) {
            participants.push(Participant::Approver(Approver {
                id: ApproverId(Uuid::new_v4()),
                strategy: ApprovalStrategy::Any,
            }));
        }
        let security_attributes = participant_attributes(owner, &participants);
        Self {
            id: ChannelId::new(),
            session_id,
            name,
            moderator,
            participants,
            security_attributes,
            confidentiality,
            history_limits: Vec::new(),
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

/// Who takes part in a session or channel. Users and agents by reference:
/// their records live in the `EntityStore` and are read from there, so
/// that no session keeps a copy that goes stale or shows more of them than
/// their name (issue 120). An approver has no record of its own; it is
/// kept here whole.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Participant {
    User(UserId),
    Agent(AgentId),
    Approver(Approver),
}

impl Participant {
    pub fn actor_id(&self) -> ActorId {
        match self {
            Participant::User(id) => ActorId::User(*id),
            Participant::Agent(id) => ActorId::Agent(*id),
            Participant::Approver(approver) => ActorId::Approver(approver.id),
        }
    }
}

/// The attributes of a session or channel owned by `owner` with these
/// participants: its users and agents contribute - they read it and act in
/// it - while only the owner may change or delete it (issue 11). Approvers
/// act on behalf of users and need no access of their own.
pub fn participant_attributes(owner: ActorId, participants: &[Participant]) -> SecurityAttributes {
    let mut contributors: Vec<ActorId> = Vec::new();
    for participant in participants {
        let id = participant.actor_id();
        if matches!(participant, Participant::User(_) | Participant::Agent(_)) && id != owner && !contributors.contains(&id) {
            contributors.push(id);
        }
    }
    SecurityAttributes { owner_id: owner, contributors }
}

