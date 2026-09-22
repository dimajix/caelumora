use std::sync::Arc;
use axum::{extract::State, Json};
use caelumora_model::template::SessionTemplate;
use crate::auth::AuthenticatedUser;
use crate::error::ApiError;
use crate::state::AppState;

pub async fn list_templates(State(state): State<Arc<AppState>>, user: AuthenticatedUser) -> Result<Json<Vec<SessionTemplate>>, ApiError> {
    let ctx = user.security_context();
    let templates = state.entity_store.list_session_templates(&ctx).await
        .map_err(|e| ApiError::internal("templates_unavailable", "listing the session templates", &e.into()))?;
    Ok(Json(templates))
}
