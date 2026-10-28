//! Workspaces (issue 253): as their owner or a user sees them, their files and the commands
//! of their shell (issue 294).

use crate::access::AccessEntry;
use crate::scope::{ScopeKind, ScopeResponse};
use caelumora_model::snapshot;
use caelumora_model::confidentiality::Classification;
use caelumora_model::security::Grant;
use caelumora_model::value::text::Description;
use caelumora_model::workspace::{CommandStatus, WorkspaceId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A workspace (issue 253) as its owner, or a participant of a channel using
/// it, sees it.
#[derive(Debug, Serialize)]
pub struct WorkspaceResponse {
    pub id: WorkspaceId,
    pub name: String,
    /// Where it lies: `session` - made for one session and used in it
    /// alone, which `session_id` names -, `user` or `global`.
    #[serde(flatten)]
    pub scope: ScopeResponse,
    /// The storage its files lie in, by its configured name.
    pub storage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Description>,
    pub classification: Classification,
    /// Whether its storage gives it a shell: agents can run commands in it.
    pub has_shell: bool,
    /// Whether you may change it - rename, describe, reclassify, share -
    /// and attach it to a session: its owner.
    pub can_change: bool,
    /// Whether you may change its files (issue 23); who may not, reads them.
    pub can_write: bool,
    /// Whom it is shared with (issue 23) - only for its owner, empty for
    /// everybody else.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub access: Vec<AccessEntry>,
}

impl From<snapshot::WorkspaceSnapshot> for WorkspaceResponse {
    fn from(view: snapshot::WorkspaceSnapshot) -> Self {
        let access = view.access.into_iter().filter_map(AccessEntry::of).collect();
        Self {
            id: view.id, name: view.name, scope: ScopeResponse::of(&view.scope), storage: view.storage, description: view.description, classification: view.classification,
            has_shell: view.has_shell, can_change: view.can_change, can_write: view.can_write, access,
        }
    }
}

/// A workspace to make (issue 253), in a configured storage; in its
/// owner's scope and `internal` without compartments unless named.
#[derive(Debug, Deserialize)]
pub struct CreateWorkspaceRequest {
    pub name: String,
    #[serde(default)]
    pub scope: ScopeKind,
    pub storage: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub classification: Option<Classification>,
}

#[derive(Debug, Deserialize)]
pub struct RenameWorkspaceRequest {
    pub name: String,
}

/// A configured storage workspaces can be made in.
#[derive(Debug, Serialize)]
pub struct StorageResponse {
    pub name: String,
    /// Whether its workspaces have a shell: agents can run commands in them
    /// (issue 266).
    pub has_shell: bool,
}

/// The workspace to attach to a session (issue 253), shared with the
/// session's participants as far as `share` says - `read` or `write` -, with
/// nobody if absent (issue 23).
#[derive(Debug, Deserialize)]
pub struct AttachWorkspaceRequest {
    pub workspace_id: WorkspaceId,
    #[serde(default)]
    pub share: Option<Grant>,
}

/// Whether a channel uses the session's workspace (issue 251).
#[derive(Debug, Deserialize)]
pub struct UsesWorkspaceRequest {
    pub uses: bool,
}

/// Where in a workspace: a directory to list; the workspace itself if
/// absent.
#[derive(Debug, Deserialize)]
pub struct FilesQuery {
    #[serde(default)]
    pub path: String,
}

/// An entry of a directory of a workspace.
#[derive(Debug, Serialize)]
pub struct FileResponse {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<DateTime<Utc>>,
}


/// How a command of a workspace's shell stands (issue 294): as a result of
/// `run_command` shows it, without what the model is told to do next.
#[derive(Debug, Serialize)]
pub struct CommandResponse {
    pub id: String,
    pub status: CommandStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub output: String,
    pub next_offset: u64,
    pub truncated: bool,
}
