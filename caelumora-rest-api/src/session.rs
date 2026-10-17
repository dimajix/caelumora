//! Sessions: making, describing and listing them, their participants and the notes kept in
//! them.

use crate::channel::ChannelResponse;
use crate::workspace::WorkspaceResponse;
use caelumora_model::snapshot;
use caelumora_model::actor::ActorId;
use caelumora_model::approval::ApprovalRule;
use caelumora_model::channel::HistoryAccess;
use caelumora_model::confidentiality::Classification;
use caelumora_model::digest::Digest;
use caelumora_model::note::Note;
use caelumora_model::privacy::PrivacyConfig;
use caelumora_model::security::Grant;
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::value::text::{Briefing, Summary};
use caelumora_model::workspace::{WorkspaceChoice, WorkspaceId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub template_id: Uuid,
    pub name: Option<String>,
    /// Its workspace in place of the template's (issue 266); the
    /// template's if absent.
    #[serde(default)]
    pub workspace: Option<SessionWorkspaceRequest>,
}

/// The workspace a new session is given in place of its template's (issue
/// 266).
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionWorkspaceRequest {
    /// None, whatever the template names.
    None,
    /// One of the user's own, shared with the session's participants as far as
    /// `share` says - `read` or `write` -, with nobody if absent (issue 23).
    Existing {
        workspace_id: WorkspaceId,
        #[serde(default)]
        share: Option<Grant>,
    },
    /// One made for the session alone, in a configured storage; `internal`
    /// without compartments unless named, as a workspace made on its own.
    New {
        storage: String,
        #[serde(default)]
        classification: Option<Classification>,
    },
}

impl From<SessionWorkspaceRequest> for WorkspaceChoice {
    fn from(request: SessionWorkspaceRequest) -> Self {
        match request {
            SessionWorkspaceRequest::None => Self::None,
            SessionWorkspaceRequest::Existing { workspace_id, share } => Self::Existing { id: workspace_id, share },
            SessionWorkspaceRequest::New { storage, classification } =>
                Self::New { storage, classification: classification.unwrap_or(Classification::CHANNEL_DEFAULT) },
        }
    }
}

/// A session in a list, or as creating or renaming it answers.
#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub id: SessionId,
    pub name: String,
    /// What it is for, in a line (issue 245); absent if nobody said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
    /// Whom the session lets read what was said in it (issue 134).
    pub privacy: PrivacyConfig,
}

impl From<&Session> for SessionResponse {
    fn from(session: &Session) -> Self {
        Self { id: session.id, name: session.name.to_string(), summary: session.summary.clone(), privacy: session.privacy }
    }
}

impl From<snapshot::SessionSnapshot> for SessionResponse {
    fn from(view: snapshot::SessionSnapshot) -> Self {
        Self { id: view.id, name: view.name, summary: view.summary, privacy: view.privacy }
    }
}

/// What a participant is called whose user or agent no longer exists.
pub(crate) const UNKNOWN: &str = "(unknown)";

/// What kind of participant an actor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipantKind {
    User,
    Agent,
    Approver,
}

impl ParticipantKind {
    /// The kind of participant `id` is; `None` for an id that is none - the
    /// system, nobody.
    pub fn of(id: ActorId) -> Option<Self> {
        match id {
            ActorId::User(_) => Some(Self::User),
            ActorId::Agent(_) => Some(Self::Agent),
            ActorId::Approver(_) => Some(Self::Approver),
            _ => None,
        }
    }
}

/// Someone to take into a session (issue 221), from a search by name: a
/// user or an agent, whether they take part already - in the session, in
/// the channel asked about -, and why they cannot be taken in, if they
/// cannot.
#[derive(Debug, Serialize)]
pub struct CandidateResponse {
    pub id: ActorId,
    pub name: String,
    pub kind: ParticipantKind,
    pub in_session: bool,
    pub in_channel: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<snapshot::CandidateRefusal>,
}

impl CandidateResponse {
    /// `view` as the API shows it; `None` for an id that is no participant.
    pub fn of(view: snapshot::CandidateSnapshot) -> Option<Self> {
        Some(Self { kind: ParticipantKind::of(view.id)?, id: view.id, name: view.name, in_session: view.in_session, in_channel: view.in_channel, refusal: view.refusal })
    }
}

/// A search of whom to take into a session (issue 221): by the beginning
/// of their name, with whether they could be taken into `channel` too.
#[derive(Debug, Deserialize)]
pub struct CandidatesQuery {
    pub prefix: String,
    #[serde(default)]
    pub channel: Option<ChannelId>,
}

/// Someone to take into a session (issue 221) - and with `channel` into
/// that channel at once, seeing as much of its history as `history` grants.
#[derive(Debug, Deserialize)]
pub struct AddSessionParticipantRequest {
    pub participant: ActorId,
    #[serde(default)]
    pub channel: Option<ChannelId>,
    #[serde(default)]
    pub history: HistoryAccess,
}

/// Joining a session by one's own act (issue 221): oneself, or with
/// `participant` one's own agent, and with `channel` that channel at once.
#[derive(Debug, Deserialize)]
pub struct JoinSessionRequest {
    #[serde(default)]
    pub participant: Option<ActorId>,
    #[serde(default)]
    pub channel: Option<ChannelId>,
}

/// The private notes one agent wrote in one channel of a session (issue
/// 233), with the digest of their chain.
#[derive(Debug, Serialize)]
pub struct PrivateNotesResponse {
    pub channel_id: ChannelId,
    pub channel_name: String,
    pub notes: Vec<Note>,
    pub digest: Option<Digest>,
}

/// A session with its channels: an entry of the global snapshot, and what
/// `GET /sessions/{id}` answers.
#[derive(Debug, Serialize)]
pub struct SessionSnapshot {
    pub id: SessionId,
    pub name: String,
    /// What it is for, in a line (issue 245); absent if nobody said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
    /// What an agent should know before it works here (issue 245): given
    /// where one session or channel is asked for, not where they are
    /// listed; absent if nobody wrote one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub briefing: Option<Briefing>,
    /// Whom the session lets read what was said in it (issue 134).
    pub privacy: PrivacyConfig,
    /// What it restricts of the tool calls made in any of its channels
    /// (issue 13): rules that ask or deny.
    pub approval_rules: Vec<ApprovalRule>,
    /// Whether you may change it - rename it, change its privacy, set its
    /// approval rules.
    pub can_change: bool,
    pub channels: Vec<ChannelResponse>,
    /// Everyone who takes part in the session - whom a new channel can be
    /// made of (issue 09).
    pub participants: Vec<ParticipantResponse>,
    /// The workspace attached to it (issue 253), if you may read it - its
    /// owner; absent otherwise. Who uses it learns of it in a channel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceResponse>,
}

impl From<snapshot::SessionSnapshot> for SessionSnapshot {
    fn from(view: snapshot::SessionSnapshot) -> Self {
        Self {
            id: view.id,
            name: view.name,
            summary: view.summary,
            briefing: view.briefing,
            privacy: view.privacy,
            approval_rules: view.approval_rules,
            can_change: view.can_change,
            channels: view.channels.into_iter().map(Into::into).collect(),
            // Every participant is a user, an agent or an approver.
            participants: view.participants.into_iter().filter_map(ParticipantResponse::of).collect(),
            workspace: view.workspace.map(Into::into),
        }
    }
}

/// A participant of a session.
#[derive(Debug, Serialize)]
pub struct ParticipantResponse {
    pub id: ActorId,
    pub name: String,
    pub kind: ParticipantKind,
}

impl ParticipantResponse {
    /// `view` as the API shows it; `None` for an id that is no participant.
    fn of(view: snapshot::ParticipantSnapshot) -> Option<Self> {
        Some(Self { id: view.id, kind: ParticipantKind::of(view.id)?, name: view.name.unwrap_or_else(|| UNKNOWN.to_string()) })
    }
}

#[derive(Debug, Deserialize)]
pub struct RenameSessionRequest {
    pub name: String,
}

/// What a session or channel is for (issue 245), replacing what it said:
/// `null` or empty removes one.
#[derive(Debug, Deserialize)]
pub struct DescriptionRequest {
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub briefing: Option<String>,
}

/// A session's new privacy policy (issue 134).
#[derive(Debug, Deserialize)]
pub struct SessionPrivacyRequest {
    pub privacy: PrivacyConfig,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    
    
    

    #[test]
    fn a_new_session_s_workspace_is_chosen_by_its_kind_and_otherwise_the_template_s() {
        let parse = |json: Value| serde_json::from_value::<CreateSessionRequest>(json).unwrap().workspace.map(WorkspaceChoice::from);
        let template = Uuid::new_v4();
        assert_eq!(parse(serde_json::json!({ "template_id": template })), None, "the template's");
        assert_eq!(parse(serde_json::json!({ "template_id": template, "workspace": { "kind": "none" } })), Some(WorkspaceChoice::None));
        let id = WorkspaceId::new();
        assert_eq!(parse(serde_json::json!({ "template_id": template, "workspace": { "kind": "existing", "workspace_id": id } })), Some(WorkspaceChoice::Existing { id, share: None }));
        assert_eq!(parse(serde_json::json!({ "template_id": template, "workspace": { "kind": "existing", "workspace_id": id, "share": "write" } })), Some(WorkspaceChoice::Existing { id, share: Some(Grant::Write) }), "shared with its members");
        assert_eq!(
            parse(serde_json::json!({ "template_id": template, "workspace": { "kind": "new", "storage": "sandbox" } })),
            Some(WorkspaceChoice::New { storage: "sandbox".to_string(), classification: Classification::CHANNEL_DEFAULT }),
            "internal unless classified",
        );
    }

}
