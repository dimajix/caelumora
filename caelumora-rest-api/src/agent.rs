//! Agents on their own, not as participants of a channel (issues 164, 227), and
//! the personalities they are made of.

use crate::scope::{ScopeKind, ScopeResponse};
use caelumora_model::snapshot;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{AgentId, PersonalityId, PersonalityKind};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::value::text::Description;
use serde::{Deserialize, Serialize};

/// An agent the requesting user may read (issues 164, 227), with what they
/// may do with it.
#[derive(Debug, Serialize)]
pub struct AgentResponse {
    pub id: AgentId,
    pub name: String,
    /// Where it lies: `user` or `global` for an agent of its own, `session`
    /// for one that takes part only in the session `session_id` names - or
    /// in its channel `channel_id`.
    #[serde(flatten)]
    pub scope: ScopeResponse,
    /// Whose it is: whose rights it acts with.
    pub owner: ActorId,
    /// What it is made of.
    pub personality_id: PersonalityId,
    /// Its personality's name; absent if the personality is gone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personality: Option<String>,
    /// Whether you may rename it.
    pub can_change: bool,
    pub can_delete: bool,
    /// The channels it takes part in that you may read - resting in them
    /// included: given where one agent is asked for, not where they are
    /// listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<Vec<AgentChannelResponse>>,
}

/// A channel an agent takes part in, with its session.
#[derive(Debug, Serialize)]
pub struct AgentChannelResponse {
    pub session_id: SessionId,
    pub session_name: String,
    pub channel_id: ChannelId,
    pub channel_name: String,
}

impl From<snapshot::AgentSnapshot> for AgentResponse {
    fn from(view: snapshot::AgentSnapshot) -> Self {
        Self {
            id: view.id, name: view.name, scope: ScopeResponse::of(&view.scope), owner: view.owner, personality_id: view.personality_id,
            personality: view.personality, can_change: view.can_change, can_delete: view.can_delete, channels: None,
        }
    }
}

/// An agent to make of a personality (issue 227): the requesting user's, in
/// their scope unless `global` - or `session`, taking part only in the
/// session `session_id` names, or in its channel `channel_id`, as far as
/// the personality admits it.
#[derive(Debug, Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    pub personality_id: PersonalityId,
    #[serde(default)]
    pub scope: ScopeKind,
    #[serde(default)]
    pub session_id: Option<SessionId>,
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
}

#[derive(Debug, Deserialize)]
pub struct RenameAgentRequest {
    pub name: String,
}

/// A personality the requesting user may read (issue 227): what they may
/// make an agent of.
#[derive(Debug, Serialize)]
pub struct PersonalityResponse {
    pub id: PersonalityId,
    pub name: String,
    /// Where it lies, as an agent's (`AgentResponse::scope`): an agent made
    /// of it lies there or within it.
    #[serde(flatten)]
    pub scope: ScopeResponse,
    pub kind: PersonalityKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Description>,
}

impl From<snapshot::PersonalitySnapshot> for PersonalityResponse {
    fn from(view: snapshot::PersonalitySnapshot) -> Self {
        Self { id: view.id, name: view.name, scope: ScopeResponse::of(&view.scope), kind: view.kind, description: view.description }
    }
}
