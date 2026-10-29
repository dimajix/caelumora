//! Skills (issue 22) in the API: whoever may read them lists and reads
//! them, their owner makes, changes, deletes and restores them. One of a
//! session's scope is written by an agent in that session, not here. Using
//! a skill is no request of its own: an agent loads it in a turn, or a
//! participant gives it to an agent with `/skill`. Served behind the core's
//! login (`RestPlugin`, issue 323), each handler with the `SkillManager`
//! in its state.

use crate::api::{
    CreateSkillRequest, RenameSkillRequest, SkillBodyRequest, SkillDescriptionRequest,
    SkillResponse,
};
use crate::manager::{SkillError, SkillManager};
use crate::plugin::SkillsPlugin;
use crate::skill::SkillId;
use crate::view::SkillViews;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use caelumora_model::confidentiality::Classification;
use caelumora_model::store::StoreError;
use caelumora_rest_api::deleted::{DeletedResponse, ListOptions};
use caelumora_rest_server::auth::AuthenticatedUser;
use caelumora_rest_server::error::{ApiError, Json, Path, Query};
use caelumora_rest_server::plugin::RestPlugin;
use caelumora_rest_server::routes::views;
use caelumora_rest_server::state::AppState;
use std::sync::Arc;
use uuid::Uuid;

/// What the handlers of skills share: the state of every handler, and the
/// one writer of skills, taken from the runtime when the routes are built.
#[derive(Clone)]
struct Skills {
    app: Arc<AppState>,
    manager: Arc<SkillManager>,
}

impl RestPlugin for SkillsPlugin {
    fn routes(&self, app: Arc<AppState>) -> Router {
        // Without the plugin registered with the runtime there are no
        // skills to serve.
        let Some(manager) = app.runtime_manager.services().get::<SkillManager>().map(Arc::clone)
        else {
            tracing::error!("The skills plugin serves no routes: the runtime was built without it");
            return Router::new();
        };
        Router::new()
            .route("/skills", get(list_skills).post(create_skill))
            .route("/skills/{id}", get(get_skill).patch(rename_skill).delete(delete_skill))
            .route("/skills/{id}/description", put(describe_skill))
            .route("/skills/{id}/body", put(rewrite_skill))
            .route("/skills/{id}/classification", put(reclassify_skill))
            .route("/skills/{id}/restore", post(restore_skill))
            .route("/skills/{id}/purge", post(purge_skill))
            .with_state(Skills { app, manager })
    }
}

/// How the API answers what went wrong with a skill.
impl From<SkillError> for ApiError {
    fn from(error: SkillError) -> Self {
        let detail = error.to_string();
        match error {
            SkillError::NotFound(_)
            | SkillError::NoSkillNamed(_)
            | SkillError::Store(StoreError::NotFound { .. }) => {
                Self::not_found("skill_not_found", detail)
            }
            SkillError::ClassificationRefused => {
                Self::bad_request("invalid_classification", detail)
            }
            SkillError::NotCleared => Self::forbidden("not_cleared", detail),
            SkillError::Invalid(e) => e.into(),
            SkillError::Store(StoreError::Validation(_)) => {
                Self::bad_request("invalid_scope", detail)
            }
            SkillError::Store(e) => e.into(),
            // A caller that is no user, a runtime without skills: not the
            // client's request.
            SkillError::OwnerNotAUser | SkillError::NotJoined => {
                Self::internal("internal_error", "the request", &error.into())
            }
        }
    }
}

/// The skills the user may read and is cleared for - or, with
/// `?deleted=true`, those they deleted. Without their bodies.
async fn list_skills(
    State(state): State<Skills>,
    Query(options): Query<ListOptions>,
    user: AuthenticatedUser,
) -> Result<Response, ApiError> {
    let views = views(&state.app, &user);
    if options.deleted {
        let deleted = views.deleted_skills().await?;
        return Ok(Json(deleted.into_iter().map(DeletedResponse::from).collect::<Vec<_>>())
            .into_response());
    }
    let skills = views.skills().await?;
    let mut listed = Vec::with_capacity(skills.len());
    for skill in skills {
        listed.push(SkillResponse::from(skill.snapshot().await));
    }
    Ok(Json(listed).into_response())
}

/// Makes a skill of the user's, in their scope or the global one.
async fn create_skill(
    State(state): State<Skills>,
    user: AuthenticatedUser,
    Json(request): Json<CreateSkillRequest>,
) -> Result<(StatusCode, Json<SkillResponse>), ApiError> {
    let scope = request.scope.of_user(user.id).ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_scope",
            "A skill of a session's scope is written by an agent in it",
        )
    })?;
    let classification = request.classification.unwrap_or(Classification::CHANNEL_DEFAULT);
    let made = state
        .manager
        .create(
            &user.security_context(),
            &request.name,
            scope,
            &request.description,
            &request.body,
            classification,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(readable(&state, &user, made.id).await?)))
}

/// Skill `id`, with its body.
async fn get_skill(
    State(state): State<Skills>,
    Path(id): Path<Uuid>,
    user: AuthenticatedUser,
) -> Result<Json<SkillResponse>, ApiError> {
    Ok(Json(readable(&state, &user, SkillId(id)).await?))
}

async fn rename_skill(
    State(state): State<Skills>,
    Path(id): Path<Uuid>,
    user: AuthenticatedUser,
    Json(request): Json<RenameSkillRequest>,
) -> Result<Json<SkillResponse>, ApiError> {
    let id = SkillId(id);
    state.manager.rename(&user.security_context(), id, &request.name).await?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Says anew what the skill is for.
async fn describe_skill(
    State(state): State<Skills>,
    Path(id): Path<Uuid>,
    user: AuthenticatedUser,
    Json(request): Json<SkillDescriptionRequest>,
) -> Result<Json<SkillResponse>, ApiError> {
    let id = SkillId(id);
    state.manager.describe(&user.security_context(), id, &request.description).await?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Writes anew what the skill tells the model.
async fn rewrite_skill(
    State(state): State<Skills>,
    Path(id): Path<Uuid>,
    user: AuthenticatedUser,
    Json(request): Json<SkillBodyRequest>,
) -> Result<Json<SkillResponse>, ApiError> {
    let id = SkillId(id);
    state.manager.rewrite(&user.security_context(), id, &request.body).await?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Reclassifies the skill as a whole.
async fn reclassify_skill(
    State(state): State<Skills>,
    Path(id): Path<Uuid>,
    user: AuthenticatedUser,
    Json(classification): Json<Classification>,
) -> Result<Json<SkillResponse>, ApiError> {
    let id = SkillId(id);
    state.manager.reclassify(&user.security_context(), id, classification).await?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Deletes the skill: kept until it is purged.
async fn delete_skill(
    State(state): State<Skills>,
    Path(id): Path<Uuid>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let id = SkillId(id);
    let deleted = state.manager.delete(&user.security_context(), id).await?;
    match deleted {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("skill_not_found", format!("There is no skill {id}"))),
    }
}

/// Restores the deleted skill; `409` if another took its name meanwhile.
async fn restore_skill(
    State(state): State<Skills>,
    Path(id): Path<Uuid>,
    user: AuthenticatedUser,
) -> Result<Json<SkillResponse>, ApiError> {
    let id = SkillId(id);
    state.manager.restore(&user.security_context(), id).await?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Removes the deleted skill for good - as the policy allows, by default
/// the system's alone.
async fn purge_skill(
    State(state): State<Skills>,
    Path(id): Path<Uuid>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let id = SkillId(id);
    let purged = state.manager.purge(&user.security_context(), id).await?;
    match purged {
        true => Ok(StatusCode::NO_CONTENT),
        false => {
            Err(ApiError::not_found("skill_not_found", format!("There is no deleted skill {id}")))
        }
    }
}

/// Skill `id` as the user sees it, with its body.
async fn readable(
    state: &Skills,
    user: &AuthenticatedUser,
    id: SkillId,
) -> Result<SkillResponse, ApiError> {
    let skill = views(&state.app, user).skill(id).await?;
    let body = skill.body().clone();
    Ok(SkillResponse { body: Some(body), ..skill.snapshot().await.into() })
}
