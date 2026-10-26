//! Artifacts (issue 271) in the API: their owner keeps, describes,
//! shares, deletes and restores theirs; whoever the artifact's list lets
//! read it reads the versions whose own list lets them as well (issues 23,
//! 316), by the artifact, never through a channel - a channel lists what
//! was published in it. A version is downloaded as an attachment
//! (`routes::attachment_response`), named as its artifact. Served behind
//! the core's login (`RestPlugin`, issue 328), each handler with the
//! `ArtifactManager` in its state.

use crate::api::{ArtifactResponse, KeepArtifactRequest, ShareOptions};
use crate::artifact::ArtifactId;
use crate::manager::{ArtifactError, ArtifactManager};
use crate::plugin::ArtifactsPlugin;
use crate::view::{ArtifactChannelViews, ArtifactViews};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::store::StoreError;
use caelumora_rest_api::access::AccessEntry;
use caelumora_rest_api::scope::ItemDescriptionRequest;
use caelumora_rest_api::trash::{DeletedResponse, ListOptions};
use caelumora_rest_server::auth::AuthenticatedUser;
use caelumora_rest_server::error::{ApiError, Json, Path, Query};
use caelumora_rest_server::plugin::RestPlugin;
use caelumora_rest_server::routes::{attachment_response, channel_of, views};
use caelumora_rest_server::state::AppState;
use std::sync::Arc;
use uuid::Uuid;

/// What the handlers of artifacts share: the state of every handler, and
/// the one owner of artifacts, taken from the runtime when the routes are
/// built.
#[derive(Clone)]
struct Artifacts {
    app: Arc<AppState>,
    manager: Arc<ArtifactManager>,
}

impl RestPlugin for ArtifactsPlugin {
    fn routes(&self, app: Arc<AppState>) -> Router {
        // Without the plugin registered with the runtime there are no
        // artifacts to serve.
        let Some(manager) = app.runtime_manager.services().get::<ArtifactManager>().map(Arc::clone) else {
            tracing::error!("The artifacts plugin serves no routes: the runtime was built without it");
            return Router::new();
        };
        Router::new()
            .route("/artifacts", get(list_artifacts))
            .route("/artifacts/{id}", get(get_artifact).delete(delete_artifact))
            .route("/artifacts/{id}/scope", put(keep_artifact))
            .route("/artifacts/{id}/description", put(describe_artifact))
            .route("/artifacts/{id}/restore", post(restore_artifact))
            .route("/artifacts/{id}/purge", post(purge_artifact))
            .route("/artifacts/{id}/access", put(set_access))
            .route("/artifacts/{id}/versions/{n}/content", get(download_version))
            .route("/sessions/{id}/channels/{cid}/artifacts", get(channel_artifacts))
            .with_state(Artifacts { app, manager })
    }
}

/// How the API answers what went wrong with an artifact.
impl From<ArtifactError> for ApiError {
    fn from(error: ArtifactError) -> Self {
        let detail = error.to_string();
        match error {
            ArtifactError::NotFound(_) | ArtifactError::Store(StoreError::NotFound { .. }) => Self::not_found("artifact_not_found", detail),
            ArtifactError::VersionNotFound { .. } => Self::not_found("version_not_found", detail),
            ArtifactError::NotConfigured => Self::bad_request("artifacts_not_configured", detail),
            ArtifactError::StorageNotCleared(_) => Self::bad_request("storage_not_cleared", detail),
            ArtifactError::ClassifiedOtherwise(_) => Self::conflict("classified_otherwise", detail),
            ArtifactError::InvalidScope => Self::bad_request("invalid_scope", detail),
            ArtifactError::Invalid(e) => e.into(),
            ArtifactError::Content(e) => e.into(),
            ArtifactError::StorageGone(e) => e.into(),
            ArtifactError::Store(e) => e.into(),
            // A caller that is no user, a runtime without artifacts: not the client's request.
            ArtifactError::OwnerNotAUser | ArtifactError::NotJoined => Self::internal("internal_error", "the request", &error.into()),
        }
    }
}

/// The artifacts the user may read anything of - their own and those
/// shared with them - or, with `?deleted=true`, those they deleted, the
/// trash.
async fn list_artifacts(State(state): State<Artifacts>, Query(options): Query<ListOptions>, user: AuthenticatedUser) -> Result<Response, ApiError> {
    let views = views(&state.app, &user);
    if options.deleted {
        let deleted = views.deleted_artifacts().await?;
        return Ok(Json(deleted.into_iter().map(DeletedResponse::from).collect::<Vec<_>>()).into_response());
    }
    let artifacts = views.artifacts().await?;
    let mut listed = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        listed.push(ArtifactResponse::from(artifact.snapshot().await));
    }
    Ok(Json(listed).into_response())
}

/// Artifact `id`, with the versions the user may read.
async fn get_artifact(State(state): State<Artifacts>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<ArtifactResponse>, ApiError> {
    Ok(Json(owned(&state, &user, ArtifactId(id)).await?))
}

/// Version `number` of artifact `id`: whoever its list or the version's
/// lets read it (issue 23).
async fn download_version(State(state): State<Artifacts>, Path((id, number)): Path<(Uuid, u32)>, user: AuthenticatedUser) -> Result<Response, ApiError> {
    let id = ArtifactId(id);
    let artifact = views(&state.app, &user).artifact(id).await?;
    let (size, content) = artifact.content(number).await?;
    Ok(attachment_response(size, content, artifact.artifact().name.as_str()))
}

/// Keeps the artifact beyond its session, in the user's scope or the
/// global one.
async fn keep_artifact(State(state): State<Artifacts>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<KeepArtifactRequest>) -> Result<Json<ArtifactResponse>, ApiError> {
    let id = ArtifactId(id);
    let scope = request.scope.of_user(user.id)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "invalid_scope", "An artifact is kept in its owner's scope or the global one"))?;
    state.manager.keep(&user.security_context(), id, scope).await?;
    Ok(Json(owned(&state, &user, id).await?))
}

/// Says what the artifact is; `null` or empty takes it away.
async fn describe_artifact(State(state): State<Artifacts>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<ItemDescriptionRequest>) -> Result<Json<ArtifactResponse>, ApiError> {
    let id = ArtifactId(id);
    state.manager.describe(&user.security_context(), id, request.description.as_deref()).await?;
    Ok(Json(owned(&state, &user, id).await?))
}

/// Deletes the artifact: kept, with its content, until it is purged.
async fn delete_artifact(State(state): State<Artifacts>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let id = ArtifactId(id);
    let deleted = state.manager.delete(&user.security_context(), id).await?;
    match deleted {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("artifact_not_found", format!("There is no artifact {id}"))),
    }
}

/// Restores the deleted artifact; `409` if another took its name meanwhile.
async fn restore_artifact(State(state): State<Artifacts>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<ArtifactResponse>, ApiError> {
    let id = ArtifactId(id);
    state.manager.restore(&user.security_context(), id).await?;
    Ok(Json(owned(&state, &user, id).await?))
}

/// Removes the deleted artifact for good, its content first - as the
/// policy allows, by default the system's alone.
async fn purge_artifact(State(state): State<Artifacts>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let id = ArtifactId(id);
    let purged = state.manager.purge(&user.security_context(), id).await?;
    match purged {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("artifact_not_found", format!("There is no deleted artifact {id}"))),
    }
}

/// The artifacts with a version published in the channel, each with those
/// versions, as a reader of the channel sees them.
async fn channel_artifacts(State(state): State<Artifacts>, Path((id, cid)): Path<(Uuid, Uuid)>, user: AuthenticatedUser) -> Result<Json<Vec<ArtifactResponse>>, ApiError> {
    let channel = channel_of(&state.app, &user, SessionId(id), ChannelId(cid)).await?;
    let artifacts = channel.artifacts().await?;
    let mut listed = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        listed.push(ArtifactResponse::from(artifact.snapshot().await));
    }
    Ok(Json(listed))
}

/// Gives the artifact the access list in the body (issues 23, 316),
/// replacing the one it had: its owner's act. Whom it newly names reads
/// every version there is as well, unless `?versions=false`.
async fn set_access(State(state): State<Artifacts>, Path(id): Path<Uuid>, Query(options): Query<ShareOptions>, user: AuthenticatedUser, Json(entries): Json<Vec<AccessEntry>>) -> Result<Json<ArtifactResponse>, ApiError> {
    let id = ArtifactId(id);
    let acl = AccessEntry::acl_of(entries).map_err(|e| ApiError::bad_request("invalid_access", e))?;
    state.manager.set_access(&user.security_context(), id, acl, options.versions).await?;
    Ok(Json(owned(&state, &user, id).await?))
}

/// Artifact `id`, as the user sees it, with where its versions were
/// published.
async fn owned(state: &Artifacts, user: &AuthenticatedUser, id: ArtifactId) -> Result<ArtifactResponse, ApiError> {
    let artifact = views(&state.app, user).artifact(id).await?;
    Ok(ArtifactResponse::from(artifact.snapshot_with_publications().await))
}
