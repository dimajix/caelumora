//! Agents' notes in the API (issues 17, 233, 607): the board of a channel, which a client reads as
//! it opens the channel and again on the channel's `NOTES_CHANGED`, and an agent's private notes,
//! for whoever may read what it saw. Served behind the kernel's login (`RestPlugin`, issue 323).

use crate::plugin::NotesPlugin;
use crate::view::{AgentNotesBoard, SessionAgentNotes};
use crate::wire::{PrivateNotesResponse, PublishedNotesResponse};
use axum::extract::State;
use axum::routing::get;
use axum::Router;
use caelumora_domain::channel::ChannelId;
use caelumora_domain::session::SessionId;
use caelumora_rest_api::actor::ActorPath;
use caelumora_rest_server::auth::AuthenticatedUser;
use caelumora_rest_server::error::{ApiError, Json, Path};
use caelumora_rest_server::plugin::RestPlugin;
use caelumora_rest_server::routes::{channel_view, views};
use caelumora_rest_server::state::AppState;
use std::sync::Arc;

impl RestPlugin for NotesPlugin {
    fn routes(&self, app: Arc<AppState>) -> Router {
        Router::new()
            .route("/sessions/{id}/channels/{cid}/notes", get(get_notes))
            .route("/sessions/{id}/actors/{aid}/private_notes", get(agent_private_notes))
            .with_state(app)
    }
}

/// The notes every agent of the channel published (issue 17) - what a client reads as it opens the
/// channel, and again on the channel's `NOTES_CHANGED` (issue 65).
async fn get_notes(
    State(state): State<Arc<AppState>>,
    Path((session_id, channel_id)): Path<(SessionId, ChannelId)>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<PublishedNotesResponse>>, ApiError> {
    let channel = channel_view(&state, &user, session_id, channel_id).await?;
    let notes = channel.notes().await.map_err(|e| {
        ApiError::internal(
            "notes_failed",
            &format!("reading the notes of channel {}", channel_id),
            &e.into(),
        )
    })?;
    Ok(Json(notes.into_iter().map(Into::into).collect()))
}

/// The private notes agent `aid` wrote in the channels of session `id` the user may read (issue
/// 233), sorted by the channel's name, for whoever may read what it saw (`ReadAgentContext`); none
/// for a user.
async fn agent_private_notes(
    State(state): State<Arc<AppState>>,
    Path((session_id, ActorPath(actor_id))): Path<(SessionId, ActorPath)>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<PrivateNotesResponse>>, ApiError> {
    // A user keeps no private notes, as it counts no calls (`get_participant_usage`).
    let Some(agent) = actor_id.as_agent_id() else { return Ok(Json(Vec::new())) };
    let session = views(&state, &user).session(session_id).await?;
    let found = session.private_notes_of(agent).await?;
    Ok(Json(
        found
            .into_iter()
            .map(|found| PrivateNotesResponse {
                channel_id: found.channel.id(),
                channel_name: found.channel.name().to_string(),
                notes: found.notes,
                digest: found.digest,
            })
            .collect(),
    ))
}
