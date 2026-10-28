//! Projects (issue 467): a user makes shared projects, names their members, shares and lifts
//! them; whoever may read a project - its owner, its members, whom its list names, the
//! participants of a session that lies in it - lists and reads it; its owner changes, deletes and
//! restores it. A session is put into a project, or taken out into its own, by whoever may change
//! the session.

use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Json, Path, Query};
use crate::routes::{ensure_session, session_view, views};
use crate::routes::sessions::all_listed;
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use caelumora_model::project::ProjectId;
use caelumora_model::session::SessionId;
use caelumora_model::user::UserId;
use caelumora_rest_api::access::AccessEntry;
use caelumora_rest_api::deleted::{DeletedResponse, ListOptions};
use caelumora_rest_api::project::{CreateProjectRequest, LiftProjectRequest, ProjectResponse, RenameProjectRequest, SessionProjectRequest};
use caelumora_rest_api::scope::ItemDescriptionRequest;
use caelumora_rest_api::session::{SessionResponse, SessionSnapshot};
use caelumora_runtime::controller::ProjectController;
use std::sync::Arc;
use uuid::Uuid;

/// The shared projects the user reaches - or, with `?deleted=true`, those deleted they may read
/// as such.
pub(super) async fn list_projects(State(state): State<Arc<AppState>>, Query(options): Query<ListOptions>, user: AuthenticatedUser) -> Result<Response, ApiError> {
    let views = views(&state, &user);
    if options.deleted {
        let deleted = views.deleted_projects().await?;
        return Ok(Json(deleted.into_iter().map(DeletedResponse::from).collect::<Vec<_>>()).into_response());
    }
    let projects = views.projects().await?;
    let mut listed = Vec::with_capacity(projects.len());
    for project in projects {
        listed.push(ProjectResponse::from(project.snapshot().await));
    }
    Ok(Json(listed).into_response())
}

/// Makes a shared project of the user's, in their scope or the global one.
pub(super) async fn create_project(State(state): State<Arc<AppState>>, user: AuthenticatedUser, Json(request): Json<CreateProjectRequest>) -> Result<(StatusCode, Json<ProjectResponse>), ApiError> {
    let scope = request.scope.of_user(user.id)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "invalid_scope", "A project of a session's scope is its own, made with it"))?;
    let made = state.project_manager.create(&user.security_context(), &request.name, scope, request.description.as_deref()).await?;
    Ok((StatusCode::CREATED, Json(readable(&state, &user, made.id).await?)))
}

/// Project `id`, as the user sees it.
pub(super) async fn get_project(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<ProjectResponse>, ApiError> {
    Ok(Json(readable(&state, &user, ProjectId(id)).await?))
}

pub(super) async fn rename_project(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<RenameProjectRequest>) -> Result<Json<ProjectResponse>, ApiError> {
    let id = ProjectId(id);
    controller(&state, id).await?.update_name(&user.security_context(), &request.name).await?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Says what the project is for; `null` or empty takes it away.
pub(super) async fn describe_project(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<ItemDescriptionRequest>) -> Result<Json<ProjectResponse>, ApiError> {
    let id = ProjectId(id);
    controller(&state, id).await?.update_description(&user.security_context(), request.description.as_deref()).await?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Makes the users the body names the members of the project, replacing who was among them.
pub(super) async fn set_members(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(members): Json<Vec<Uuid>>) -> Result<Json<ProjectResponse>, ApiError> {
    let id = ProjectId(id);
    controller(&state, id).await?.update_members(&user.security_context(), members.into_iter().map(UserId).collect()).await?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Gives the project the access list in the body, replacing the one it had: its owner's act.
pub(super) async fn set_access(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(entries): Json<Vec<AccessEntry>>) -> Result<Json<ProjectResponse>, ApiError> {
    let id = ProjectId(id);
    let acl = AccessEntry::acl_of(entries).map_err(|e| ApiError::bad_request("invalid_access", e))?;
    controller(&state, id).await?.update_access(&user.security_context(), acl).await?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Lifts a session's own project into the user's scope or the global one, under a name of its
/// own: a shared project then, with the session in it.
pub(super) async fn lift_project(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<LiftProjectRequest>) -> Result<Json<ProjectResponse>, ApiError> {
    let id = ProjectId(id);
    let scope = request.scope.of_user(user.id)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "invalid_scope", "A project is lifted into its owner's scope or the global one"))?;
    controller(&state, id).await?.lift(&user.security_context(), &request.name, scope).await?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Deletes the project, with the sessions that lie in it and what lies in its scope: kept until
/// it is purged.
pub(super) async fn delete_project(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let id = ProjectId(id);
    match state.project_manager.delete(&user.security_context(), id).await? {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("project_not_found", format!("There is no project {id}"))),
    }
}

/// Restores the deleted project, with what went with it as far as the user may; `409` if another
/// took its name meanwhile.
pub(super) async fn restore_project(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<ProjectResponse>, ApiError> {
    let id = ProjectId(id);
    state.project_manager.restore(&user.security_context(), id).await?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Removes the deleted project for good, with what went with it - as the policy allows, by
/// default the system's alone.
pub(super) async fn purge_project(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let id = ProjectId(id);
    match state.project_manager.purge(&user.security_context(), id).await? {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("project_not_found", format!("There is no deleted project {id}"))),
    }
}

/// The sessions that lie in project `id` and the user may read, as `GET /sessions` lists them.
pub(super) async fn project_sessions(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<Vec<SessionResponse>>, ApiError> {
    let sessions = views(&state, &user).project(ProjectId(id)).await?.sessions().await?;
    Ok(Json(all_listed(sessions).await))
}

/// Puts the session into the shared project the body names: whoever may change the session and
/// read the project.
pub(super) async fn put_into_project(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<SessionProjectRequest>) -> Result<Json<SessionSnapshot>, ApiError> {
    update_project(&state, &user, SessionId(id), Some(request.project_id)).await
}

/// Takes the session out of the project it lies in, into its own.
pub(super) async fn take_out_of_project(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<SessionSnapshot>, ApiError> {
    update_project(&state, &user, SessionId(id), None).await
}

async fn update_project(state: &Arc<AppState>, user: &AuthenticatedUser, session_id: SessionId, project: Option<ProjectId>) -> Result<Json<SessionSnapshot>, ApiError> {
    let session = ensure_session(state, user, session_id).await?;
    session.update_project(&user.security_context(), project).await?;
    Ok(Json(session_view(state, user, session_id).await?))
}

/// The controller of project `id`, through which what it is changes; whoever does is asked as
/// they do.
async fn controller(state: &Arc<AppState>, id: ProjectId) -> Result<Arc<ProjectController>, ApiError> {
    Ok(state.runtime_manager.controller_manager().project_controller(id).await?)
}

/// Project `id` as the user sees it.
async fn readable(state: &Arc<AppState>, user: &AuthenticatedUser, id: ProjectId) -> Result<ProjectResponse, ApiError> {
    let project = views(state, user).project(id).await?;
    Ok(project.snapshot().await.into())
}
