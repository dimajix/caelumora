//! Workspaces (issue 253): its owner lists, makes, changes, deletes and
//! restores theirs and works on their files; a member of a channel works on
//! the files of the session's workspace there, where it may be used (issue
//! 251, decision 10). The files are reached through the workspace's
//! `Terminal` alike, whichever way it was reached.
//!
//! Downloads are always served as attachments of `application/octet-stream`,
//! never inline: a file of a workspace is anybody's content, and HTML or SVG
//! would otherwise run in the web UI's origin (issue 208).

use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Json, Path, Subject};
use crate::models::{
    AttachWorkspaceRequest, ChannelResponse, CreateWorkspaceRequest, DeletedResponse, FileResponse, FilesQuery, ListOptions, RenameWorkspaceRequest,
    SessionSnapshot, StorageResponse, UsesWorkspaceRequest, WorkspaceDescriptionRequest, WorkspaceResponse, WorkspaceScope,
};
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use caelumora_model::confidentiality::Classification;
use caelumora_model::scope::Scope;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::workspace::WorkspaceId;
use caelumora_runtime::terminal::{Terminal, WorkspacePath};
use caelumora_runtime::view::WorkspaceView;
use futures::TryStreamExt;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use std::sync::Arc;
use uuid::Uuid;

/// The user's own workspaces - or, with `?deleted=true`, those they
/// deleted, the trash.
pub async fn list_workspaces(State(state): State<Arc<AppState>>, Query(options): Query<ListOptions>, user: AuthenticatedUser) -> Result<Response, ApiError> {
    let views = crate::routes::views(&state, &user);
    if options.deleted {
        let deleted = views.deleted_workspaces().await
            .map_err(|e| ApiError::from_runtime(e.into(), Subject::Workspace, "listing the deleted workspaces"))?;
        return Ok(Json(deleted.into_iter().map(DeletedResponse::from).collect::<Vec<_>>()).into_response());
    }
    let workspaces = views.workspaces().await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Workspace, "listing the workspaces"))?;
    let mut listed = Vec::with_capacity(workspaces.len());
    for workspace in workspaces {
        listed.push(WorkspaceResponse::from(workspace.snapshot().await));
    }
    Ok(Json(listed).into_response())
}

/// The storages a workspace can be made in, by name.
pub async fn list_storages(State(state): State<Arc<AppState>>, _user: AuthenticatedUser) -> Json<Vec<StorageResponse>> {
    Json(state.runtime_manager.workspace_manager().storages().map(|name| StorageResponse { name: name.to_string() }).collect())
}

/// Makes a workspace of the user's, in their scope or the global one.
pub async fn create_workspace(State(state): State<Arc<AppState>>, user: AuthenticatedUser, Json(request): Json<CreateWorkspaceRequest>) -> Result<(StatusCode, Json<WorkspaceResponse>), ApiError> {
    let scope = match request.scope {
        WorkspaceScope::User => Scope::UserScope { user_id: user.id },
        WorkspaceScope::Global => Scope::GlobalScope,
        WorkspaceScope::Session => return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid_scope", "A workspace of a session's scope is made with the session, by its template")),
    };
    let classification = request.classification.unwrap_or(Classification::CHANNEL_DEFAULT);
    let made = state.runtime_manager.workspace_manager()
        .create(&user.security_context(), &request.name, scope, &request.storage, request.description.as_deref(), classification).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Workspace, "creating the workspace"))?;
    Ok((StatusCode::CREATED, Json(owned(&state, &user, made.id).await?)))
}

/// The user's workspace `id` as they see it.
pub async fn get_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<WorkspaceResponse>, ApiError> {
    Ok(Json(owned(&state, &user, WorkspaceId(id)).await?))
}

pub async fn rename_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<RenameWorkspaceRequest>) -> Result<Json<WorkspaceResponse>, ApiError> {
    let id = WorkspaceId(id);
    state.runtime_manager.workspace_manager().rename(&user.security_context(), id, &request.name).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Workspace, &format!("renaming workspace {id}")))?;
    Ok(Json(owned(&state, &user, id).await?))
}

/// Says what the workspace holds; `null` or empty takes it away.
pub async fn describe_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<WorkspaceDescriptionRequest>) -> Result<Json<WorkspaceResponse>, ApiError> {
    let id = WorkspaceId(id);
    state.runtime_manager.workspace_manager().describe(&user.security_context(), id, request.description.as_deref()).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Workspace, &format!("describing workspace {id}")))?;
    Ok(Json(owned(&state, &user, id).await?))
}

/// Reclassifies the workspace as a whole: lowering releases every file.
pub async fn reclassify_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(classification): Json<Classification>) -> Result<Json<WorkspaceResponse>, ApiError> {
    let id = WorkspaceId(id);
    state.runtime_manager.workspace_manager().reclassify(&user.security_context(), id, classification).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Workspace, &format!("reclassifying workspace {id}")))?;
    Ok(Json(owned(&state, &user, id).await?))
}

/// Deletes the workspace: kept, with its files, until it is purged.
pub async fn delete_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let id = WorkspaceId(id);
    let deleted = state.runtime_manager.workspace_manager().delete(&user.security_context(), id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Workspace, &format!("deleting workspace {id}")))?;
    match deleted {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("workspace_not_found", format!("There is no workspace {id}"))),
    }
}

/// Restores the deleted workspace; `409` if another took its name meanwhile.
pub async fn restore_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<WorkspaceResponse>, ApiError> {
    let id = WorkspaceId(id);
    state.runtime_manager.workspace_manager().restore(&user.security_context(), id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Workspace, &format!("restoring workspace {id}")))?;
    Ok(Json(owned(&state, &user, id).await?))
}

/// Removes the deleted workspace for good, its files first - as the policy
/// allows, by default the system's alone.
pub async fn purge_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let id = WorkspaceId(id);
    let purged = state.runtime_manager.workspace_manager().purge(&user.security_context(), id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Workspace, &format!("purging workspace {id}")))?;
    match purged {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("workspace_not_found", format!("There is no deleted workspace {id}"))),
    }
}

/// Lists a directory of the user's workspace `id`.
pub async fn owned_files(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, Query(query): Query<FilesQuery>, user: AuthenticatedUser) -> Result<Json<Vec<FileResponse>>, ApiError> {
    list(owned_terminal(&state, &user, WorkspaceId(id)).await?.as_ref(), &query.path).await
}

pub async fn download_owned_file(State(state): State<Arc<AppState>>, Path((id, path)): Path<(Uuid, String)>, user: AuthenticatedUser) -> Result<Response, ApiError> {
    download(owned_terminal(&state, &user, WorkspaceId(id)).await?.as_ref(), &path).await
}

pub async fn upload_owned_file(State(state): State<Arc<AppState>>, Path((id, path)): Path<(Uuid, String)>, user: AuthenticatedUser, body: Body) -> Result<StatusCode, ApiError> {
    let limit = state.runtime_manager.workspace_manager().max_file_bytes();
    upload(owned_terminal(&state, &user, WorkspaceId(id)).await?.as_ref(), &path, body, limit).await
}

pub async fn delete_owned_file(State(state): State<Arc<AppState>>, Path((id, path)): Path<(Uuid, String)>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    remove(owned_terminal(&state, &user, WorkspaceId(id)).await?.as_ref(), &path).await
}

/// The session's workspace as the user may use it in this channel; `null`
/// where they may not - which says nothing of why.
pub async fn channel_workspace(State(state): State<Arc<AppState>>, Path((id, cid)): Path<(Uuid, Uuid)>, user: AuthenticatedUser) -> Result<Json<Option<WorkspaceResponse>>, ApiError> {
    Ok(Json(match in_channel(&state, &user, SessionId(id), ChannelId(cid)).await? {
        Some(workspace) => Some(workspace.snapshot().await.into()),
        None => None,
    }))
}

pub async fn channel_files(State(state): State<Arc<AppState>>, Path((id, cid)): Path<(Uuid, Uuid)>, Query(query): Query<FilesQuery>, user: AuthenticatedUser) -> Result<Json<Vec<FileResponse>>, ApiError> {
    list(channel_terminal(&state, &user, SessionId(id), ChannelId(cid)).await?.as_ref(), &query.path).await
}

pub async fn download_channel_file(State(state): State<Arc<AppState>>, Path((id, cid, path)): Path<(Uuid, Uuid, String)>, user: AuthenticatedUser) -> Result<Response, ApiError> {
    download(channel_terminal(&state, &user, SessionId(id), ChannelId(cid)).await?.as_ref(), &path).await
}

pub async fn upload_channel_file(State(state): State<Arc<AppState>>, Path((id, cid, path)): Path<(Uuid, Uuid, String)>, user: AuthenticatedUser, body: Body) -> Result<StatusCode, ApiError> {
    let limit = state.runtime_manager.workspace_manager().max_file_bytes();
    upload(channel_terminal(&state, &user, SessionId(id), ChannelId(cid)).await?.as_ref(), &path, body, limit).await
}

pub async fn delete_channel_file(State(state): State<Arc<AppState>>, Path((id, cid, path)): Path<(Uuid, Uuid, String)>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    remove(channel_terminal(&state, &user, SessionId(id), ChannelId(cid)).await?.as_ref(), &path).await
}

/// Attaches the user's workspace to the session: its owner's act, into a
/// session they may change.
pub async fn attach_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<AttachWorkspaceRequest>) -> Result<Json<SessionSnapshot>, ApiError> {
    set_workspace(&state, &user, SessionId(id), Some(request.workspace_id)).await
}

/// Takes the attached workspace away from the session; it shares nothing.
pub async fn detach_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<SessionSnapshot>, ApiError> {
    set_workspace(&state, &user, SessionId(id), None).await
}

/// Sets whether the channel uses the session's workspace: whoever may
/// change the channel.
pub async fn set_uses_workspace(State(state): State<Arc<AppState>>, Path((id, cid)): Path<(Uuid, Uuid)>, user: AuthenticatedUser, Json(request): Json<UsesWorkspaceRequest>) -> Result<Json<ChannelResponse>, ApiError> {
    let (session_id, channel_id) = (SessionId(id), ChannelId(cid));
    let (_, channel) = crate::routes::open_channel(&state, &user, session_id, channel_id).await?;
    channel.update_uses_workspace(&user.security_context(), request.uses).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, "changing the channel's use of the workspace"))?;
    Ok(Json(crate::routes::channel_view(&state, &user, session_id, channel_id).await?))
}

async fn set_workspace(state: &Arc<AppState>, user: &AuthenticatedUser, session_id: SessionId, workspace: Option<WorkspaceId>) -> Result<Json<SessionSnapshot>, ApiError> {
    let session = crate::routes::ensure_session(state, user, session_id).await?;
    session.set_workspace(&user.security_context(), workspace).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, &format!("changing the workspace of session {session_id}")))?;
    Ok(Json(crate::routes::session_view(state, user, session_id).await?))
}

/// The user's workspace `id`, as they see it.
async fn owned(state: &Arc<AppState>, user: &AuthenticatedUser, id: WorkspaceId) -> Result<WorkspaceResponse, ApiError> {
    let workspace = crate::routes::views(state, user).workspace(id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Workspace, &format!("reading workspace {id}")))?;
    Ok(workspace.snapshot().await.into())
}

async fn owned_terminal(state: &Arc<AppState>, user: &AuthenticatedUser, id: WorkspaceId) -> Result<Arc<dyn Terminal>, ApiError> {
    let workspace = crate::routes::views(state, user).workspace(id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Workspace, &format!("reading workspace {id}")))?;
    terminal_of(&workspace).await
}

async fn in_channel(state: &Arc<AppState>, user: &AuthenticatedUser, session: SessionId, channel: ChannelId) -> Result<Option<WorkspaceView>, ApiError> {
    crate::routes::channel_of(state, user, session, channel).await?.workspace().await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Workspace, "reading the channel's workspace"))
}

async fn channel_terminal(state: &Arc<AppState>, user: &AuthenticatedUser, session: SessionId, channel: ChannelId) -> Result<Arc<dyn Terminal>, ApiError> {
    match in_channel(state, user, session, channel).await? {
        Some(workspace) => terminal_of(&workspace).await,
        None => Err(ApiError::not_found("workspace_not_found", "There is no workspace in this channel")),
    }
}

async fn terminal_of(workspace: &WorkspaceView) -> Result<Arc<dyn Terminal>, ApiError> {
    workspace.terminal().await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Workspace, &format!("reaching workspace {}", workspace.id())))
}

/// `text` as a path inside the workspace.
fn path(text: &str) -> Result<WorkspacePath, ApiError> {
    WorkspacePath::parse(text).map_err(|e| ApiError::from_runtime(e.into(), Subject::Workspace, "reading the path"))
}

fn file_error(error: caelumora_runtime::terminal::TerminalError, action: &str) -> ApiError {
    ApiError::from_runtime(error.into(), Subject::Workspace, action)
}

async fn list(terminal: &dyn Terminal, dir: &str) -> Result<Json<Vec<FileResponse>>, ApiError> {
    let entries = terminal.list(&path(dir)?).await.map_err(|e| file_error(e, "listing the directory"))?;
    Ok(Json(entries.into_iter().map(FileResponse::from).collect()))
}

/// The file, streamed, as an attachment under its name.
async fn download(terminal: &dyn Terminal, file: &str) -> Result<Response, ApiError> {
    let file = path(file)?;
    let (size, content) = terminal.read(&file).await.map_err(|e| file_error(e, "reading the file"))?;
    let mut response = Body::from_stream(content).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(disposition) = HeaderValue::from_str(&attachment(file.file_name().unwrap_or("file"))) {
        headers.insert(header::CONTENT_DISPOSITION, disposition);
    }
    Ok(response)
}

/// The request's body as the file, replacing what was there; `201`.
async fn upload(terminal: &dyn Terminal, file: &str, body: Body, limit: u64) -> Result<StatusCode, ApiError> {
    let content = Box::pin(body.into_data_stream().map_err(std::io::Error::other));
    terminal.write(&path(file)?, content, limit).await.map_err(|e| file_error(e, "writing the file"))?;
    Ok(StatusCode::CREATED)
}

async fn remove(terminal: &dyn Terminal, file: &str) -> Result<StatusCode, ApiError> {
    terminal.delete(&path(file)?).await.map_err(|e| file_error(e, "deleting the file"))?;
    Ok(StatusCode::NO_CONTENT)
}

/// `Content-Disposition` of a download named `name` (RFC 6266): the name
/// as it is, encoded, and plain for the clients that read only that.
fn attachment(name: &str) -> String {
    let plain: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
    format!("attachment; filename=\"{plain}\"; filename*=UTF-8''{}", utf8_percent_encode(name, NON_ALPHANUMERIC))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_is_named_plainly_and_as_it_is() {
        assert_eq!(attachment("report.md"), "attachment; filename=\"report.md\"; filename*=UTF-8''report%2Emd");
        let named = attachment("Bericht \"Q3\" ü.pdf");
        assert!(named.starts_with("attachment; filename=\"Bericht__Q3___.pdf\";"), "{named}");
        assert!(named.ends_with("filename*=UTF-8''Bericht%20%22Q3%22%20%C3%BC%2Epdf"), "{named}");
    }
}
