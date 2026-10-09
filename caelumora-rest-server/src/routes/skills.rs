//! Skills (issue 22): whoever may read them lists and reads them, their
//! owner makes, changes, deletes and restores them. One of a session's scope
//! is written by an agent in that session, not here. Using a skill is no
//! request of its own: an agent loads it in a turn, or a member gives it to
//! an agent with `/skill`.
use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Json, Path, Subject};
use crate::models::{CreateSkillRequest, DeletedResponse, ListOptions, RenameSkillRequest, SkillBodyRequest, SkillDescriptionRequest, SkillResponse};
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use caelumora_model::confidentiality::Classification;
use caelumora_model::skill::SkillId;
use std::sync::Arc;
use uuid::Uuid;

/// The skills the user may read and is cleared for - or, with
/// `?deleted=true`, those they deleted, the trash. Without their bodies.
pub async fn list_skills(State(state): State<Arc<AppState>>, Query(options): Query<ListOptions>, user: AuthenticatedUser) -> Result<Response, ApiError> {
    let views = crate::routes::views(&state, &user);
    if options.deleted {
        let deleted = views.deleted_skills().await
            .map_err(|e| ApiError::from_runtime(e.into(), Subject::Skill, "listing the deleted skills"))?;
        return Ok(Json(deleted.into_iter().map(DeletedResponse::from).collect::<Vec<_>>()).into_response());
    }
    let skills = views.skills().await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Skill, "listing the skills"))?;
    let mut listed = Vec::with_capacity(skills.len());
    for skill in skills {
        listed.push(SkillResponse::from(skill.snapshot().await));
    }
    Ok(Json(listed).into_response())
}

/// Makes a skill of the user's, in their scope or the global one.
pub async fn create_skill(State(state): State<Arc<AppState>>, user: AuthenticatedUser, Json(request): Json<CreateSkillRequest>) -> Result<(StatusCode, Json<SkillResponse>), ApiError> {
    let scope = request.scope.of_user(user.id)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "invalid_scope", "A skill of a session's scope is written by an agent in it"))?;
    let classification = request.classification.unwrap_or(Classification::CHANNEL_DEFAULT);
    let made = state.runtime_manager.skill_manager()
        .create(&user.security_context(), &request.name, scope, &request.description, &request.body, classification).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Skill, "creating the skill"))?;
    Ok((StatusCode::CREATED, Json(readable(&state, &user, made.id).await?)))
}

/// Skill `id`, with its body.
pub async fn get_skill(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<SkillResponse>, ApiError> {
    Ok(Json(readable(&state, &user, SkillId(id)).await?))
}

pub async fn rename_skill(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<RenameSkillRequest>) -> Result<Json<SkillResponse>, ApiError> {
    let id = SkillId(id);
    state.runtime_manager.skill_manager().rename(&user.security_context(), id, &request.name).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Skill, &format!("renaming skill {id}")))?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Says anew what the skill is for.
pub async fn describe_skill(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<SkillDescriptionRequest>) -> Result<Json<SkillResponse>, ApiError> {
    let id = SkillId(id);
    state.runtime_manager.skill_manager().describe(&user.security_context(), id, &request.description).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Skill, &format!("describing skill {id}")))?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Writes anew what the skill tells the model.
pub async fn rewrite_skill(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<SkillBodyRequest>) -> Result<Json<SkillResponse>, ApiError> {
    let id = SkillId(id);
    state.runtime_manager.skill_manager().rewrite(&user.security_context(), id, &request.body).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Skill, &format!("rewriting skill {id}")))?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Reclassifies the skill as a whole.
pub async fn reclassify_skill(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(classification): Json<Classification>) -> Result<Json<SkillResponse>, ApiError> {
    let id = SkillId(id);
    state.runtime_manager.skill_manager().reclassify(&user.security_context(), id, classification).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Skill, &format!("reclassifying skill {id}")))?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Deletes the skill: kept until it is purged.
pub async fn delete_skill(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let id = SkillId(id);
    let deleted = state.runtime_manager.skill_manager().delete(&user.security_context(), id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Skill, &format!("deleting skill {id}")))?;
    match deleted {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("skill_not_found", format!("There is no skill {id}"))),
    }
}

/// Restores the deleted skill; `409` if another took its name meanwhile.
pub async fn restore_skill(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<SkillResponse>, ApiError> {
    let id = SkillId(id);
    state.runtime_manager.skill_manager().restore(&user.security_context(), id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Skill, &format!("restoring skill {id}")))?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Removes the deleted skill for good - as the policy allows, by default
/// the system's alone.
pub async fn purge_skill(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let id = SkillId(id);
    let purged = state.runtime_manager.skill_manager().purge(&user.security_context(), id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Skill, &format!("purging skill {id}")))?;
    match purged {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("skill_not_found", format!("There is no deleted skill {id}"))),
    }
}

/// Skill `id` as the user sees it, with its body.
async fn readable(state: &Arc<AppState>, user: &AuthenticatedUser, id: SkillId) -> Result<SkillResponse, ApiError> {
    let skill = crate::routes::views(state, user).skill(id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Skill, &format!("reading skill {id}")))?;
    let body = skill.body().clone();
    Ok(SkillResponse { body: Some(body), ..skill.snapshot().await.into() })
}
