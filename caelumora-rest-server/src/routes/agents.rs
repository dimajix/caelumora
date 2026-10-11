//! The agents a user may read (issue 164): listing them, and deleting,
//! restoring and purging them. Agents are made from templates; there is no
//! way yet to create one here.

use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Json, Path, Subject};
use crate::models::{AgentResponse, DeletedResponse, ListOptions};
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
        let deleted = views.deleted_agents().await
            .map_err(|e| ApiError::from_runtime(e.into(), Subject::Agent, "listing the deleted agents"))?;
        return Ok(Json(deleted.into_iter().map(DeletedResponse::from).collect::<Vec<_>>()).into_response());
    }
    let agents = views.agents().await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Agent, "listing the agents"))?;
    Ok(Json(agents.into_iter().map(AgentResponse::from).collect::<Vec<_>>()).into_response())
}

/// Deletes agent `id`: it leaves every channel it takes part in, as a
/// former participant, and is kept marked until it is purged.
pub async fn delete_agent(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let agent_id = AgentId(id);
    state.session_manager.delete_agent(&user.security_context(), agent_id).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Agent, &format!("deleting agent {agent_id}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Restores the deleted agent `id`; it takes part in no channel until it is
/// added again. `409` if another agent took its name meanwhile.
pub async fn restore_agent(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<AgentResponse>, ApiError> {
    let agent_id = AgentId(id);
    state.runtime_manager.agent_manager().restore(&user.security_context(), agent_id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Agent, &format!("restoring agent {agent_id}")))?;
    // As the list shows it: what the user may do with it is the views' to say.
    let agents = crate::routes::views(&state, &user).agents().await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Agent, "listing the agents"))?;
    agents.into_iter().find(|agent| agent.id == agent_id).map(|agent| Json(agent.into()))
        .ok_or_else(|| ApiError::not_found("agent_not_found", format!("Agent {agent_id} is not there")))
}

/// Removes the deleted agent `id` for good, with its state - as the policy
/// allows, by default the system's alone.
pub async fn purge_agent(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let agent_id = AgentId(id);
    state.runtime_manager.agent_manager().purge(&user.security_context(), agent_id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Agent, &format!("purging agent {agent_id}")))?;
    Ok(StatusCode::NO_CONTENT)
}
