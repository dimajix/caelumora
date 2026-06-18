use std::sync::Arc;
use axum::{extract::State, Json};
use caelumora_model::template::SessionTemplate;
use crate::state::AppState;

use caelumora_model::security::SecurityContext;

pub async fn list_templates(State(state): State<Arc<AppState>>) -> Json<Vec<SessionTemplate>> {
    let ctx = SecurityContext::default();
    Json(state.entity_store.list_session_templates(&ctx).await.expect("failed to list templates"))
}
