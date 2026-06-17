use std::sync::Arc;
use axum::{extract::State, Json};
use caelumora_model::template::SessionTemplate;
use crate::state::AppState;

pub async fn list_templates(State(state): State<Arc<AppState>>) -> Json<Vec<SessionTemplate>> {
    Json(state.store.list_session_templates().unwrap_or_default())
}
