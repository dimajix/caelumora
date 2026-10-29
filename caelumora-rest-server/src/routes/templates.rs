use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Json};
use crate::state::AppState;
use axum::extract::State;
use caelumora_model::store::query::Query;
use caelumora_rest_api::template::TemplateResponse;
use std::sync::Arc;

/// The session templates the user may start a session from - what a client
/// offers, not the templates themselves (issue 106).
pub(super) async fn list_templates(
    State(state): State<Arc<AppState>>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<TemplateResponse>>, ApiError> {
    let ctx = user.security_context();
    let templates = state
        .runtime_manager
        .entity_stores()
        .session_templates
        .list(&ctx, &Query::all())
        .await
        .map(|page| page.items)
        .map_err(|e| {
            ApiError::internal("templates_unavailable", "listing the session templates", &e.into())
        })?;
    let mut templates: Vec<TemplateResponse> =
        templates.iter().map(TemplateResponse::from).collect();
    // Sorted by name: the store lists in no particular order (the JSON store
    // in its directory's), and clients should see the same order every time
    // (issue 40).
    templates.sort_by(|l, r| l.name.cmp(&r.name));
    Ok(Json(templates))
}
