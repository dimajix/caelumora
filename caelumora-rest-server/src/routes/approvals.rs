use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use caelumora_model::approval::{ApprovalId, ApprovalResponse};
use std::sync::Arc;
use uuid::Uuid;

pub async fn respond(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(payload): Json<ApprovalResponse>,
) -> Result<StatusCode, StatusCode> {
    let approval_id = ApprovalId(id);
    
    // Safety check: ensure the ID in the path matches the ID in the payload
    if approval_id != payload.id {
        return Err(StatusCode::BAD_REQUEST);
    }

    state.runtime_manager.approval_manager().respond(approval_id, payload)
        .map_err(|e| {
            tracing::error!("Failed to respond to approval {}: {}", approval_id, e);
            StatusCode::NOT_FOUND
        })?;

    Ok(StatusCode::OK)
}
