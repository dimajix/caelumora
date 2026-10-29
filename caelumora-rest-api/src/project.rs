//! Projects (issue 467): what holds the results of sessions, as whoever may read one sees it, and
//! how one is made, changed, lifted and chosen for a session.

use crate::access::AccessEntry;
use crate::scope::{ScopeKind, ScopeResponse};
use caelumora_model::project::ProjectId;
use caelumora_model::snapshot;
use caelumora_model::user::UserId;
use caelumora_model::value::text::Description;
use serde::{Deserialize, Serialize};

/// A project as whoever may read it sees it: a shared one they own, work on or are shared, or a
/// session's own.
#[derive(Debug, Serialize)]
pub struct ProjectResponse {
    pub id: ProjectId,
    /// Its name where it lies; `own` for a session's own, which the session names.
    pub name: String,
    /// Where it lies: `user` or `global` for a shared project, `session` for the own project of
    /// the session `session_id` names.
    #[serde(flatten)]
    pub scope: ScopeResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Description>,
    /// Who works on it, each with their name where it is known.
    pub members: Vec<ProjectMemberResponse>,
    /// Whether you may change it - rename, describe, name its members, share, lift, delete: its
    /// owner.
    pub can_change: bool,
    /// Whom it is shared with - only for its owner, empty for everybody else.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub access: Vec<AccessEntry>,
}

/// A member of a project.
#[derive(Debug, Serialize)]
pub struct ProjectMemberResponse {
    pub id: UserId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl From<snapshot::ProjectSnapshot> for ProjectResponse {
    fn from(view: snapshot::ProjectSnapshot) -> Self {
        let members =
            view.members.into_iter().map(|(id, name)| ProjectMemberResponse { id, name }).collect();
        let access = view.access.into_iter().filter_map(AccessEntry::of).collect();
        Self {
            id: view.id,
            name: view.name,
            scope: ScopeResponse::of(&view.scope),
            description: view.description,
            members,
            can_change: view.can_change,
            access,
        }
    }
}

/// A shared project to make: in the user's scope unless `global`, with nobody among its members
/// yet.
#[derive(Debug, Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
    #[serde(default)]
    pub scope: ScopeKind,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RenameProjectRequest {
    pub name: String,
}

/// Where to lift a session's own project, under which name: the user's scope unless `global`.
#[derive(Debug, Deserialize)]
pub struct LiftProjectRequest {
    pub name: String,
    #[serde(default)]
    pub scope: ScopeKind,
}

/// The shared project to put a session into.
#[derive(Debug, Deserialize)]
pub struct SessionProjectRequest {
    pub project_id: ProjectId,
}
