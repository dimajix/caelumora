use crate::auth::AuthenticatedUser;
use crate::models::{ChannelMemberResponse, ChannelResponse, SendApprovalRequest, SendMessageRequest};
use crate::state::AppState;
use axum::response::sse::Event;
use axum::response::Sse;
use crate::error::ApiError;
use crate::routes::channel_of;
use axum::{extract::State, http::StatusCode};
use crate::error::{Json, Path};
use caelumora_model::agent::AgentId;
use caelumora_runtime::actor_impl::user::UserActor;
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
    user: AuthenticatedUser,
) -> Result<Json<Vec<ChannelResponse>>, ApiError> {
    let session_id = SessionId(id);
    let session = crate::routes::ensure_session(&state, &user, session_id).await?;

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
    user: AuthenticatedUser,
) -> Result<Json<ChannelResponse>, ApiError> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session = crate::routes::ensure_session(&state, &user, session_id).await?;
    
    let channel = channel_of(&session, channel_id)?;

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
    user: AuthenticatedUser,
) -> Result<Json<Vec<Message>>, ApiError> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session = crate::routes::ensure_session(&state, &user, session_id).await?;
    let channel = channel_of(&session, channel_id)?;

    let messages = channel.history().await;

    Ok(Json(messages))
}

pub async fn join_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);

    let session_loop = crate::routes::ensure_session(&state, &user, session_id).await?;
    let channel = channel_of(&session_loop, channel_id)?;

    let receiver = channel.subscribe();
    let stream = BroadcastStream::new(receiver);

    let sse_stream = stream.map(move |res| {
        match res {
            Ok(envelope) => {
                let event_json = serde_json::to_string(&envelope).unwrap_or_default();
                Ok(Event::default().data(event_json))
            }
            // The only error: this client fell behind and events were dropped.
            Err(e) => {
                tracing::warn!("SSE client of channel {} lost events: {}", channel_id, e);
                Ok(Event::default().comment(format!("events lost: {}", e)))
            }
        }
    });

    Ok(Sse::new(sse_stream))
}

pub async fn send_message(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<SendMessageRequest>,
) -> Result<StatusCode, ApiError> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session = crate::routes::ensure_session(&state, &user, session_id).await?;
    
    let channel = channel_of(&session, channel_id)?;
    let user_actor = user_actor_of(&channel)?;
    channel.wakeup().await
        .map_err(|e| ApiError::internal("channel_wakeup_failed", "waking the channel's agents", &e))?;
    let span_id = user_actor.send_message(payload.content).await
        .map_err(|e| ApiError::internal("message_not_sent", "sending the message", &e))?;
    tracing::Span::current().record("span_id", tracing::field::display(span_id));
    Ok(StatusCode::OK)
}

/// The user actor through which the REST API acts in `channel`.
fn user_actor_of(channel: &ChannelLoop) -> Result<Arc<UserActor>, ApiError> {
    channel.user_actor()
        .ok_or_else(|| ApiError::conflict("no_user_participant", format!("Channel {} has no user participant to act through", channel.id())))
}

pub async fn cancel_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session = crate::routes::ensure_session(&state, &user, session_id).await?;

    channel_of(&session, channel_id)?;
    session.cancel_channel(channel_id).await
        .map_err(|e| ApiError::internal("cancel_failed", "cancelling the channel", &e))?;
    Ok(StatusCode::OK)
}

pub async fn send_approval(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<SendApprovalRequest>,
) -> Result<StatusCode, ApiError> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session = crate::routes::ensure_session(&state, &user, session_id).await?;

    let channel = channel_of(&session, channel_id)?;
    let user_actor = user_actor_of(&channel)?;
    let response = ApprovalResponse {
        id: ApprovalId(payload.approval_id),
        agent_id: AgentId(payload.agent_id),
    };
    let span_id = user_actor.send_approval(response, payload.approved).await
        .map_err(|e| ApiError::internal("approval_not_sent", "sending the approval", &e))?;
    tracing::Span::current().record("span_id", tracing::field::display(span_id));
    Ok(StatusCode::OK)
}
