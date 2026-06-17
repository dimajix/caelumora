use std::sync::Arc;
use axum::{extract::{Path, State}, http::StatusCode, Json};
use uuid::Uuid;
use caelumora_model::message::{Message, MessageRole};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_runtime::actor::ActorEvent;
use crate::state::AppState;
use crate::models::{ChannelResponse, SendMessageRequest};

pub async fn list_channels(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ChannelResponse>>, StatusCode> {
    let session_id = SessionId(id);
    let session = state.session_manager.get_session(session_id)
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(
        session.channels()
            .into_iter()
            .map(|c| {
                ChannelResponse {
                    id: c.id(),
                    name: c.name(),
                    participants: c.actor_names()
                }
            })
            .collect(),
    ))
}

pub async fn get_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<ChannelResponse>, StatusCode> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session = state.session_manager.get_session(session_id)
        .ok_or(StatusCode::NOT_FOUND)?;
    
    let channel = session.channel_by_id(channel_id)
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(ChannelResponse {
        id: channel.id(),
        name: channel.name(),
        participants: channel.actor_names()
    }))
}

pub async fn get_channel_messages(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<Message>>, StatusCode> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session = state.session_manager.get_session(session_id)
        .ok_or(StatusCode::NOT_FOUND)?;
    let channel = session.channel_by_id(channel_id)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;

    let messages = channel.history().await;

    Ok(Json(messages))
}

pub async fn send_message(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    Json(payload): Json<SendMessageRequest>,
) -> StatusCode {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session = state.session_manager.get_session(session_id);
    
    if let Some(session) = session {
        if let Some(channel) = session.channel_by_id(channel_id) {
            if let Some(user_actor) = channel.user_actor() {
                let wakeup = ActorEvent::Wakeup;
                let message = ActorEvent::FullResponseGenerated(vec![Message {
                    role: MessageRole::User,
                    content: Some(payload.content),
                    ..Message::default()
                }]);
                if !user_actor.send_message(wakeup).await.is_ok() {
                    return StatusCode::INTERNAL_SERVER_ERROR;
                }
                if !user_actor.send_message(message).await.is_ok() {
                    return StatusCode::INTERNAL_SERVER_ERROR;
                }
            }
        }
    }

    StatusCode::NOT_FOUND
}

pub async fn cancel_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> StatusCode {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session = state.session_manager.get_session(session_id);

    if let Some(session) = session {
        if session.cancel_channel(channel_id).await.is_ok() {
            return StatusCode::OK;
        }
    }

    StatusCode::NOT_FOUND
}
