//! Groups (issue 286): teams, and the participants of a session or a channel.

use crate::scope::ScopeKind;
use caelumora_model::snapshot;
use caelumora_model::actor::ActorId;
use caelumora_model::group::GroupId;
use caelumora_model::session::{ChannelId, SessionId};
use serde::{Deserialize, Serialize};

/// A group (issue 286) as whoever may read it sees it: a team, or the
/// participants of a session or a channel.
#[derive(Debug, Serialize)]
pub struct GroupResponse {
    pub id: GroupId,
    pub name: String,
    /// Where it lies: `user` or `global` for a team, `session` for the
    /// participants of the session `session_id` names - or of its channel
    /// `channel_id`.
    pub scope: ScopeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<ChannelId>,
    /// Whose it is.
    pub owner: ActorId,
    /// Who belongs to it, each with their name where it is known.
    pub members: Vec<GroupMemberResponse>,
    /// Whether you may rename it, change its members and delete it: the
    /// owner of a team.
    pub can_change: bool,
}

/// A member of a group.
#[derive(Debug, Serialize)]
pub struct GroupMemberResponse {
    pub id: ActorId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl From<snapshot::GroupSnapshot> for GroupResponse {
    fn from(view: snapshot::GroupSnapshot) -> Self {
        let (scope, session_id, channel_id) = ScopeKind::placed(&view.scope);
        let members = view.members.into_iter().map(|(id, name)| GroupMemberResponse { id, name }).collect();
        Self { id: view.id, name: view.name, scope, session_id, channel_id, owner: view.owner, members, can_change: view.can_change }
    }
}

/// A team to make (issue 286): in the user's scope unless `global`, with
/// nobody in it yet.
#[derive(Debug, Deserialize)]
pub struct CreateGroupRequest {
    pub name: String,
    #[serde(default)]
    pub scope: ScopeKind,
}

#[derive(Debug, Deserialize)]
pub struct RenameGroupRequest {
    pub name: String,
}
