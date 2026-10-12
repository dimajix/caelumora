//! Workspaces (issue 253): its owner lists, makes, changes, shares, deletes
//! and restores theirs; whoever its access list lets reads its files, or
//! changes them (issue 23) - by the workspace, never through a channel. A
//! command of its shell is ended through the channel it was started in
//! (issue 294), by whoever may change the files there.
//!
//! Downloads are always served as attachments (`routes::attachment_response`),
//! never inline: a file of a workspace is anybody's content, and HTML or SVG
//! would otherwise run in the web UI's origin (issue 208).

use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Json, Path};
use caelumora_rest_api::access::AccessEntry;
use caelumora_rest_api::channel::ChannelResponse;
use caelumora_rest_api::scope::ItemDescriptionRequest;
use caelumora_rest_api::session::SessionSnapshot;
use caelumora_rest_api::trash::{DeletedResponse, ListOptions};
use caelumora_rest_api::workspace::{AttachWorkspaceRequest, CommandResponse, CreateWorkspaceRequest, FileResponse, FilesQuery, RenameWorkspaceRequest, StorageResponse, UsesWorkspaceRequest, WorkspaceResponse};
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use caelumora_model::confidentiality::Classification;
use caelumora_model::security::Grant;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::workspace::WorkspaceId;
use caelumora_runtime::terminal::{CommandState, FileEntry, Terminal, WorkspacePath};
use futures::TryStreamExt;
use std::sync::Arc;
use uuid::Uuid;

/// The user's own workspaces - or, with `?deleted=true`, those they
/// deleted, the trash.
pub async fn list_workspaces(State(state): State<Arc<AppState>>, Query(options): Query<ListOptions>, user: AuthenticatedUser) -> Result<Response, ApiError> {
    let views = crate::routes::views(&state, &user);
    if options.deleted {
        let deleted = views.deleted_workspaces().await?;
        return Ok(Json(deleted.into_iter().map(DeletedResponse::from).collect::<Vec<_>>()).into_response());
    }
    let workspaces = views.workspaces().await?;
    let mut listed = Vec::with_capacity(workspaces.len());
    for workspace in workspaces {
        listed.push(WorkspaceResponse::from(workspace.snapshot().await));
    }
    Ok(Json(listed).into_response())
}

/// The storages a workspace can be made in, by name.
pub async fn list_storages(State(state): State<Arc<AppState>>, _user: AuthenticatedUser) -> Json<Vec<StorageResponse>> {
    Json(state.runtime_manager.workspace_manager().storages().map(|(name, has_shell)| StorageResponse { name: name.to_string(), has_shell }).collect())
}

/// Makes a workspace of the user's, in their scope or the global one.
pub async fn create_workspace(State(state): State<Arc<AppState>>, user: AuthenticatedUser, Json(request): Json<CreateWorkspaceRequest>) -> Result<(StatusCode, Json<WorkspaceResponse>), ApiError> {
    let scope = request.scope.of_user(user.id)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "invalid_scope", "A workspace of a session's scope is made with the session"))?;
    let classification = request.classification.unwrap_or(Classification::CHANNEL_DEFAULT);
    let made = state.runtime_manager.workspace_manager()
        .create(&user.security_context(), &request.name, scope, &request.storage, request.description.as_deref(), classification).await?;
    Ok((StatusCode::CREATED, Json(owned(&state, &user, made.id).await?)))
}

/// The user's workspace `id` as they see it.
pub async fn get_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<WorkspaceResponse>, ApiError> {
    Ok(Json(owned(&state, &user, WorkspaceId(id)).await?))
}

pub async fn rename_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<RenameWorkspaceRequest>) -> Result<Json<WorkspaceResponse>, ApiError> {
    let id = WorkspaceId(id);
    state.runtime_manager.workspace_manager().rename(&user.security_context(), id, &request.name).await?;
    Ok(Json(owned(&state, &user, id).await?))
}

/// Says what the workspace holds; `null` or empty takes it away.
pub async fn describe_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<ItemDescriptionRequest>) -> Result<Json<WorkspaceResponse>, ApiError> {
    let id = WorkspaceId(id);
    state.runtime_manager.workspace_manager().describe(&user.security_context(), id, request.description.as_deref()).await?;
    Ok(Json(owned(&state, &user, id).await?))
}

/// Reclassifies the workspace as a whole: lowering releases every file.
pub async fn reclassify_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(classification): Json<Classification>) -> Result<Json<WorkspaceResponse>, ApiError> {
    let id = WorkspaceId(id);
    state.runtime_manager.workspace_manager().reclassify(&user.security_context(), id, classification).await?;
    Ok(Json(owned(&state, &user, id).await?))
}

/// Deletes the workspace: kept, with its files, until it is purged.
pub async fn delete_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let id = WorkspaceId(id);
    let deleted = state.runtime_manager.workspace_manager().delete(&user.security_context(), id).await?;
    match deleted {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("workspace_not_found", format!("There is no workspace {id}"))),
    }
}

/// Restores the deleted workspace; `409` if another took its name meanwhile.
pub async fn restore_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<WorkspaceResponse>, ApiError> {
    let id = WorkspaceId(id);
    state.runtime_manager.workspace_manager().restore(&user.security_context(), id).await?;
    Ok(Json(owned(&state, &user, id).await?))
}

/// Removes the deleted workspace for good, its files first - as the policy
/// allows, by default the system's alone.
pub async fn purge_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let id = WorkspaceId(id);
    let purged = state.runtime_manager.workspace_manager().purge(&user.security_context(), id).await?;
    match purged {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("workspace_not_found", format!("There is no deleted workspace {id}"))),
    }
}

/// Lists a directory of workspace `id`: whoever may read it (issue 23).
pub async fn files(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, Query(query): Query<FilesQuery>, user: AuthenticatedUser) -> Result<Json<Vec<FileResponse>>, ApiError> {
    list(terminal(&state, &user, WorkspaceId(id), Grant::Read).await?.as_ref(), &query.path).await
}

pub async fn download_file(State(state): State<Arc<AppState>>, Path((id, path)): Path<(Uuid, String)>, user: AuthenticatedUser) -> Result<Response, ApiError> {
    download(terminal(&state, &user, WorkspaceId(id), Grant::Read).await?.as_ref(), &path).await
}

/// Writes a file of workspace `id`: whoever may change its files.
pub async fn upload_file(State(state): State<Arc<AppState>>, Path((id, path)): Path<(Uuid, String)>, user: AuthenticatedUser, body: Body) -> Result<StatusCode, ApiError> {
    let limit = state.runtime_manager.workspace_manager().max_file_bytes();
    upload(terminal(&state, &user, WorkspaceId(id), Grant::Write).await?.as_ref(), &path, body, limit).await
}

pub async fn delete_file(State(state): State<Arc<AppState>>, Path((id, path)): Path<(Uuid, String)>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    remove(terminal(&state, &user, WorkspaceId(id), Grant::Write).await?.as_ref(), &path).await
}

/// Gives the workspace the access list in the body (issue 23), replacing
/// the one it had: its owner's act.
pub async fn set_access(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(entries): Json<Vec<AccessEntry>>) -> Result<Json<WorkspaceResponse>, ApiError> {
    let id = WorkspaceId(id);
    let acl = entries.into_iter().map(AccessEntry::into_acl).collect();
    state.runtime_manager.workspace_manager().set_access(&user.security_context(), id, acl).await?;
    Ok(Json(owned(&state, &user, id).await?))
}

/// Attaches the user's workspace to the session: its owner's act, into a
/// session they may change.
pub async fn attach_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<AttachWorkspaceRequest>) -> Result<Json<SessionSnapshot>, ApiError> {
    set_workspace(&state, &user, SessionId(id), Some(request.workspace_id), request.share).await
}

/// Takes the attached workspace away from the session; it shares nothing.
pub async fn detach_workspace(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<SessionSnapshot>, ApiError> {
    set_workspace(&state, &user, SessionId(id), None, None).await
}

/// Sets whether the channel uses the session's workspace: whoever may
/// change the channel.
pub async fn set_uses_workspace(State(state): State<Arc<AppState>>, Path((id, cid)): Path<(Uuid, Uuid)>, user: AuthenticatedUser, Json(request): Json<UsesWorkspaceRequest>) -> Result<Json<ChannelResponse>, ApiError> {
    let (session_id, channel_id) = (SessionId(id), ChannelId(cid));
    let (_, channel) = crate::routes::open_channel(&state, &user, session_id, channel_id).await?;
    channel.update_uses_workspace(&user.security_context(), request.uses).await?;
    Ok(Json(crate::routes::channel_view(&state, &user, session_id, channel_id).await?))
}

/// Ends command `command_id` of the workspace's shell, started in the
/// channel (issue 294): whoever may change the workspace's files there.
/// Answers how the command stands, with what it printed.
pub async fn kill_command(State(state): State<Arc<AppState>>, Path((id, cid, command_id)): Path<(Uuid, Uuid, String)>, user: AuthenticatedUser) -> Result<Json<CommandResponse>, ApiError> {
    let killed = state.runtime_manager.workspace_manager().kill_command(&user.security_context(), SessionId(id), ChannelId(cid), &command_id, 0).await?;
    Ok(Json(command_response(killed)))
}

async fn set_workspace(state: &Arc<AppState>, user: &AuthenticatedUser, session_id: SessionId, workspace: Option<WorkspaceId>, share: Option<Grant>) -> Result<Json<SessionSnapshot>, ApiError> {
    let session = crate::routes::ensure_session(state, user, session_id).await?;
    session.set_workspace(&user.security_context(), workspace, share).await?;
    Ok(Json(crate::routes::session_view(state, user, session_id).await?))
}

/// The user's workspace `id`, as they see it.
async fn owned(state: &Arc<AppState>, user: &AuthenticatedUser, id: WorkspaceId) -> Result<WorkspaceResponse, ApiError> {
    let workspace = crate::routes::views(state, user).workspace(id).await?;
    Ok(workspace.snapshot().await.into())
}

/// The terminal of workspace `id` for what `need` asks: reading its files,
/// or changing them too (issue 23).
async fn terminal(state: &Arc<AppState>, user: &AuthenticatedUser, id: WorkspaceId, need: Grant) -> Result<Arc<dyn Terminal>, ApiError> {
    let (_, terminal) = state.runtime_manager.workspace_manager().terminal(&user.security_context(), id, need).await?;
    Ok(terminal)
}

/// `text` as a path inside the workspace.
fn path(text: &str) -> Result<WorkspacePath, ApiError> {
    Ok(WorkspacePath::parse(text)?)
}

async fn list(terminal: &dyn Terminal, dir: &str) -> Result<Json<Vec<FileResponse>>, ApiError> {
    let entries = terminal.list(&path(dir)?).await?;
    Ok(Json(entries.into_iter().map(file_response).collect()))
}

/// The file, streamed, as an attachment under its name.
async fn download(terminal: &dyn Terminal, file: &str) -> Result<Response, ApiError> {
    let file = path(file)?;
    let (size, content) = terminal.read(&file).await?;
    Ok(crate::routes::attachment_response(size, content, file.file_name().unwrap_or("file")))
}

/// The request's body as the file, replacing what was there; `201`.
async fn upload(terminal: &dyn Terminal, file: &str, body: Body, limit: u64) -> Result<StatusCode, ApiError> {
    let content = Box::pin(body.into_data_stream().map_err(std::io::Error::other));
    terminal.write(&path(file)?, content, limit).await?;
    Ok(StatusCode::CREATED)
}

async fn remove(terminal: &dyn Terminal, file: &str) -> Result<StatusCode, ApiError> {
    terminal.delete(&path(file)?).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `entry` of a directory as the API shows it.
fn file_response(entry: FileEntry) -> FileResponse {
    FileResponse { name: entry.name, is_dir: entry.is_dir, size: entry.size, modified: entry.modified }
}

/// How a command stands, as the API shows it.
fn command_response(state: CommandState) -> CommandResponse {
    CommandResponse { id: state.id, status: state.status, exit_code: state.exit_code, output: state.output, next_offset: state.next_offset, truncated: state.truncated }
}
