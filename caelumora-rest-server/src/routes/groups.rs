//! Groups (issue 286): a user makes teams, names their members and shares
//! workspaces and artifacts with them by naming a team in an access list.
//! Whoever may read a group - its owner and its members - lists and reads
//! it; its owner renames it, sets its members, deletes and restores it. The
//! participants of a session or a channel are a group too, read here but
//! changed only by taking part.
use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Json, Path, Subject};
use crate::models::{CreateGroupRequest, DeletedResponse, GroupResponse, ListOptions, RenameGroupRequest};
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use caelumora_model::group::GroupId;
use caelumora_model::user::UserId;
use std::sync::Arc;
use uuid::Uuid;

/// The teams the user owns or belongs to - or, with `?deleted=true`, those
/// they deleted, the trash.
pub async fn list_groups(State(state): State<Arc<AppState>>, Query(options): Query<ListOptions>, user: AuthenticatedUser) -> Result<Response, ApiError> {
    let views = crate::routes::views(&state, &user);
    if options.deleted {
        let deleted = views.deleted_groups().await
            .map_err(|e| ApiError::from_runtime(e.into(), Subject::Group, "listing the deleted groups"))?;
        return Ok(Json(deleted.into_iter().map(DeletedResponse::from).collect::<Vec<_>>()).into_response());
    }
    let groups = views.groups().await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Group, "listing the groups"))?;
    let mut listed = Vec::with_capacity(groups.len());
    for group in groups {
        listed.push(GroupResponse::from(group.snapshot().await));
    }
    Ok(Json(listed).into_response())
}

/// Makes a team of the user's, in their scope or the global one.
pub async fn create_group(State(state): State<Arc<AppState>>, user: AuthenticatedUser, Json(request): Json<CreateGroupRequest>) -> Result<(StatusCode, Json<GroupResponse>), ApiError> {
    let scope = request.scope.of_user(user.id)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "invalid_scope", "A group of a session's scope is its participants, made with it"))?;
    let made = state.runtime_manager.group_manager()
        .create(&user.security_context(), &request.name, scope).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Group, "creating the group"))?;
    Ok((StatusCode::CREATED, Json(readable(&state, &user, made.id).await?)))
}

/// Group `id`, with its members.
pub async fn get_group(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<GroupResponse>, ApiError> {
    Ok(Json(readable(&state, &user, GroupId(id)).await?))
}

pub async fn rename_group(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<RenameGroupRequest>) -> Result<Json<GroupResponse>, ApiError> {
    let id = GroupId(id);
    state.runtime_manager.group_manager().rename(&user.security_context(), id, &request.name).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Group, &format!("renaming group {id}")))?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Makes the users the body names the members of the team, replacing who
/// was in it.
pub async fn set_members(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(members): Json<Vec<Uuid>>) -> Result<Json<GroupResponse>, ApiError> {
    let id = GroupId(id);
    let members = members.into_iter().map(UserId).collect();
    state.runtime_manager.group_manager().set_members(&user.security_context(), id, members).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Group, &format!("changing the members of group {id}")))?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Deletes the team: kept until it is purged; who it was shared with
/// through it reads it no longer.
pub async fn delete_group(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let id = GroupId(id);
    let deleted = state.runtime_manager.group_manager().delete(&user.security_context(), id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Group, &format!("deleting group {id}")))?;
    match deleted {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("group_not_found", format!("There is no group {id}"))),
    }
}

/// Restores the deleted team; `409` if another took its name meanwhile.
pub async fn restore_group(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<GroupResponse>, ApiError> {
    let id = GroupId(id);
    state.runtime_manager.group_manager().restore(&user.security_context(), id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Group, &format!("restoring group {id}")))?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Removes the deleted team for good - as the policy allows, by default the
/// system's alone.
pub async fn purge_group(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let id = GroupId(id);
    let purged = state.runtime_manager.group_manager().purge(&user.security_context(), id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Group, &format!("purging group {id}")))?;
    match purged {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("group_not_found", format!("There is no deleted group {id}"))),
    }
}

/// Group `id` as the user sees it.
async fn readable(state: &Arc<AppState>, user: &AuthenticatedUser, id: GroupId) -> Result<GroupResponse, ApiError> {
    let group = crate::routes::views(state, user).group(id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Group, &format!("reading group {id}")))?;
    Ok(group.snapshot().await.into())
}
