//! Artifacts (issue 271): their owner keeps, describes, shares, deletes and
//! restores theirs; whoever the artifact's list or a version's lets reads
//! those versions (issue 23), by the artifact, never through a channel - a
//! channel lists what was published in it. A version is downloaded as an
//! attachment (`routes::attachment_response`), named as its artifact.

use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Json, Path, Subject};
use crate::models::{AccessEntry, ArtifactResponse, DeletedResponse, ItemDescriptionRequest, KeepArtifactRequest, ListOptions};
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use caelumora_model::artifact::ArtifactId;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::snapshot::Detail;
use caelumora_runtime::artifact::ArtifactError;
use std::sync::Arc;
use uuid::Uuid;

fn failed(error: ArtifactError, action: &str) -> ApiError {
    ApiError::from_runtime(error.into(), Subject::Artifact, action)
}

/// The artifacts the user may read anything of - their own and those
/// shared with them - or, with `?deleted=true`, those they deleted, the
/// trash.
pub async fn list_artifacts(State(state): State<Arc<AppState>>, Query(options): Query<ListOptions>, user: AuthenticatedUser) -> Result<Response, ApiError> {
    let views = crate::routes::views(&state, &user);
    if options.deleted {
        let deleted = views.deleted_artifacts().await
            .map_err(|e| ApiError::from_runtime(e.into(), Subject::Artifact, "listing the deleted artifacts"))?;
        return Ok(Json(deleted.into_iter().map(DeletedResponse::from).collect::<Vec<_>>()).into_response());
    }
    let artifacts = views.artifacts().await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Artifact, "listing the artifacts"))?;
    let mut listed = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        listed.push(ArtifactResponse::from(artifact.snapshot(Detail::empty()).await));
    }
    Ok(Json(listed).into_response())
}

/// Artifact `id`, with the versions the user may read.
pub async fn get_artifact(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<ArtifactResponse>, ApiError> {
    Ok(Json(owned(&state, &user, ArtifactId(id)).await?))
}

/// Version `number` of artifact `id`: whoever its list or the version's
/// lets read it (issue 23).
pub async fn download_version(State(state): State<Arc<AppState>>, Path((id, number)): Path<(Uuid, u32)>, user: AuthenticatedUser) -> Result<Response, ApiError> {
    let id = ArtifactId(id);
    let artifact = crate::routes::views(&state, &user).artifact(id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Artifact, &format!("reading artifact {id}")))?;
    let (size, content) = artifact.content(number).await
        .map_err(|e| failed(e, &format!("reading version {number} of artifact {id}")))?;
    Ok(crate::routes::attachment_response(size, content, artifact.artifact().name.as_str()))
}

/// Keeps the artifact beyond its session, in the user's scope or the
/// global one.
pub async fn keep_artifact(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<KeepArtifactRequest>) -> Result<Json<ArtifactResponse>, ApiError> {
    let id = ArtifactId(id);
    let scope = request.scope.of_user(user.id)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "invalid_scope", "An artifact is kept in its owner's scope or the global one"))?;
    state.runtime_manager.artifact_manager().keep(&user.security_context(), id, scope).await
        .map_err(|e| failed(e, &format!("keeping artifact {id}")))?;
    Ok(Json(owned(&state, &user, id).await?))
}

/// Says what the artifact is; `null` or empty takes it away.
pub async fn describe_artifact(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<ItemDescriptionRequest>) -> Result<Json<ArtifactResponse>, ApiError> {
    let id = ArtifactId(id);
    state.runtime_manager.artifact_manager().describe(&user.security_context(), id, request.description.as_deref()).await
        .map_err(|e| failed(e, &format!("describing artifact {id}")))?;
    Ok(Json(owned(&state, &user, id).await?))
}

/// Deletes the artifact: kept, with its content, until it is purged.
pub async fn delete_artifact(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let id = ArtifactId(id);
    let deleted = state.runtime_manager.artifact_manager().delete(&user.security_context(), id).await
        .map_err(|e| failed(e, &format!("deleting artifact {id}")))?;
    match deleted {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("artifact_not_found", format!("There is no artifact {id}"))),
    }
}

/// Restores the deleted artifact; `409` if another took its name meanwhile.
pub async fn restore_artifact(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<ArtifactResponse>, ApiError> {
    let id = ArtifactId(id);
    state.runtime_manager.artifact_manager().restore(&user.security_context(), id).await
        .map_err(|e| failed(e, &format!("restoring artifact {id}")))?;
    Ok(Json(owned(&state, &user, id).await?))
}

/// Removes the deleted artifact for good, its content first - as the
/// policy allows, by default the system's alone.
pub async fn purge_artifact(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let id = ArtifactId(id);
    let purged = state.runtime_manager.artifact_manager().purge(&user.security_context(), id).await
        .map_err(|e| failed(e, &format!("purging artifact {id}")))?;
    match purged {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("artifact_not_found", format!("There is no deleted artifact {id}"))),
    }
}

/// The artifacts with a version published in the channel, each with those
/// versions, as a reader of the channel sees them.
pub async fn channel_artifacts(State(state): State<Arc<AppState>>, Path((id, cid)): Path<(Uuid, Uuid)>, user: AuthenticatedUser) -> Result<Json<Vec<ArtifactResponse>>, ApiError> {
    let channel = crate::routes::channel_of(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let artifacts = channel.artifacts().await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Artifact, "listing the channel's artifacts"))?;
    let mut listed = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        listed.push(ArtifactResponse::from(artifact.snapshot(Detail::empty()).await));
    }
    Ok(Json(listed))
}

/// Gives the artifact the access list in the body for the whole of it
/// (issue 23), replacing the one it had: its owner's act.
pub async fn set_access(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(entries): Json<Vec<AccessEntry>>) -> Result<Json<ArtifactResponse>, ApiError> {
    let id = ArtifactId(id);
    let acl = entries.into_iter().map(AccessEntry::into_acl).collect();
    state.runtime_manager.artifact_manager().set_access(&user.security_context(), id, acl).await
        .map_err(|e| failed(e, &format!("sharing artifact {id}")))?;
    Ok(Json(owned(&state, &user, id).await?))
}

/// Artifact `id`, as the user sees it, with where its versions were
/// published.
async fn owned(state: &Arc<AppState>, user: &AuthenticatedUser, id: ArtifactId) -> Result<ArtifactResponse, ApiError> {
    let artifact = crate::routes::views(state, user).artifact(id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Artifact, &format!("reading artifact {id}")))?;
    Ok(ArtifactResponse::from(artifact.snapshot(Detail::PUBLICATIONS).await))
}
