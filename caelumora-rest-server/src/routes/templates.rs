use std::sync::Arc;
use axum::{extract::State, Json};
use caelumora_model::template::SessionTemplate;
use crate::auth::AuthenticatedUser;
use crate::state::AppState;

pub async fn list_templates(State(state): State<Arc<AppState>>, user: AuthenticatedUser) -> Json<Vec<SessionTemplate>> {
    let ctx = user.security_context();
    Json(state.entity_store.list_session_templates(&ctx).await.expect("failed to list templates"))
}
