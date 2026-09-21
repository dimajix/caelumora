use crate::models::{ChannelMemberResponse, ChannelResponse, SendApprovalRequest, SendMessageRequest};
use crate::state::AppState;
use axum::response::sse::Event;
use axum::response::Sse;
use axum::{extract::{Path, State}, http::StatusCode, Json};
use caelumora_model::agent::AgentId;
use caelumora_runtime::channel::ChannelLoop;
use caelumora_model::approval::{ApprovalId, ApprovalResponse};
use caelumora_model::message::Message;
use caelumora_model::session::{ChannelId, SessionId};
use futures::Stream;
use futures::StreamExt;
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

pub async fn list_channels(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ChannelResponse>>, StatusCode> {
    let session_id = SessionId(id);
    let session = crate::routes::ensure_session(&state, session_id).await?;

    Ok(Json(
        session.channels()
            .into_iter()
            .map(|c| channel_response(&c))
            .collect(),
    ))
}

pub async fn get_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<ChannelResponse>, StatusCode> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session = crate::routes::ensure_session(&state, session_id).await?;
    
    let channel = session.channel_by_id(channel_id)
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(channel_response(&channel)))
}

fn channel_response(channel: &ChannelLoop) -> ChannelResponse {
    ChannelResponse {
        id: channel.id(),
        name: channel.name(),
        members: channel.members()
            .into_iter()
            .map(|m| ChannelMemberResponse { id: m.id, name: m.name, handle: m.handle, kind: m.kind })
            .collect(),
    }
}

pub async fn get_channel_messages(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<Message>>, StatusCode> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session = crate::routes::ensure_session(&state, session_id).await?;
    let channel = session.channel_by_id(channel_id)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;

    let messages = channel.history().await;

    Ok(Json(messages))
}

pub async fn join_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);

    let session_loop = crate::routes::ensure_session(&state, session_id).await?;
    let channel = session_loop.channel_by_id(channel_id)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;

    let receiver = channel.subscribe();
    let stream = BroadcastStream::new(receiver);

    let sse_stream = stream.map(|res| {
        match res {
            Ok(envelope) => {
                let event_json = serde_json::to_string(&envelope).unwrap_or_default();
                Ok(Event::default().data(event_json))
            }
            Err(e) => {
                tracing::error!("SSE stream error: {}", e);
                // In SSE, we can just skip or send a comment. Here we just continue or the stream might close.
                // For now, we return a default event or just map it.
                Ok(Event::default().comment("stream error"))
            }
        }
    });

    Ok(Sse::new(sse_stream))
}

pub async fn send_message(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    Json(payload): Json<SendMessageRequest>,
) -> Result<StatusCode, StatusCode> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session = crate::routes::ensure_session(&state, session_id).await?;
    
    if let Some(channel) = session.channel_by_id(channel_id) {
        if let Some(user_actor) = channel.user_actor() {
            if !channel.wakeup().await.is_ok() {
                return Ok(StatusCode::INTERNAL_SERVER_ERROR);
            }
            if !user_actor.send_message(payload.content).await.is_ok() {
                return Ok(StatusCode::INTERNAL_SERVER_ERROR);
            }
            return Ok(StatusCode::OK);
        }
    }

    Ok(StatusCode::NOT_FOUND)
}

pub async fn cancel_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session = crate::routes::ensure_session(&state, session_id).await?;

    if session.cancel_channel(channel_id).await.is_ok() {
        return Ok(StatusCode::OK);
    }

    Ok(StatusCode::NOT_FOUND)
}

pub async fn send_approval(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    Json(payload): Json<SendApprovalRequest>,
) -> Result<StatusCode, StatusCode> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session = crate::routes::ensure_session(&state, session_id).await?;

    if let Some(channel) = session.channel_by_id(channel_id) {
        if let Some(user_actor) = channel.user_actor() {
            let response = ApprovalResponse {
                id: ApprovalId(payload.approval_id),
                agent_id: AgentId(payload.agent_id),
            };
            if !user_actor.send_approval(response, payload.approved).await.is_ok() {
                return Ok(StatusCode::INTERNAL_SERVER_ERROR);
            }
            return Ok(StatusCode::OK);
        }
    }

    Ok(StatusCode::NOT_FOUND)
}
