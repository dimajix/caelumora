//! The agents a user may read (issue 164): listing them, and deleting,
//! restoring and purging them. Agents are made from templates; there is no
//! way yet to create one here.

use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Json, Path};
use caelumora_rest_api::agent::AgentResponse;
use caelumora_rest_api::trash::{DeletedResponse, ListOptions};
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use caelumora_model::agent::AgentId;
use std::sync::Arc;
use uuid::Uuid;

/// The agents the user may read - or, with `?deleted=true`, those they
/// deleted and may read as such, the trash.
pub async fn list_agents(State(state): State<Arc<AppState>>, Query(options): Query<ListOptions>, user: AuthenticatedUser) -> Result<Response, ApiError> {
    let views = crate::routes::views(&state, &user);
    if options.deleted {
        let deleted = views.deleted_agents().await?;
        return Ok(Json(deleted.into_iter().map(DeletedResponse::from).collect::<Vec<_>>()).into_response());
    }
    let agents = views.agents().await?;
    let mut listed = Vec::with_capacity(agents.len());
    for agent in agents {
        listed.push(AgentResponse::from(agent.snapshot().await?));
    }
    Ok(Json(listed).into_response())
}

/// Deletes agent `id`: it rests in the channels it takes part in, as a
/// former member, and is kept marked until it is purged.
pub async fn delete_agent(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let agent_id = AgentId(id);
    let deleted = state.runtime_manager.agent_manager().delete(&user.security_context(), agent_id).await?;
    match deleted {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("agent_not_found", format!("There is no agent {agent_id}"))),
    }
}

/// Restores the deleted agent `id`; it takes part again in the channels it
/// rested in. `409` if another agent took its name meanwhile.
pub async fn restore_agent(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<AgentResponse>, ApiError> {
    let agent_id = AgentId(id);
    state.runtime_manager.agent_manager().restore(&user.security_context(), agent_id).await?;
    // As the list shows it: what the user may do with it is the views' to say.
    let agent = crate::routes::views(&state, &user).agent(agent_id).await?;
    Ok(Json(agent.snapshot().await?.into()))
}

/// Removes the deleted agent `id` for good, with its state - as the policy
/// allows, by default the system's alone.
pub async fn purge_agent(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let agent_id = AgentId(id);
    let purged = state.runtime_manager.agent_manager().purge(&user.security_context(), agent_id).await?;
    match purged {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("agent_not_found", format!("There is no deleted agent {agent_id}"))),
    }
}
