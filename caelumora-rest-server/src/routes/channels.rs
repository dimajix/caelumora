use crate::auth::AuthenticatedUser;
use crate::models::{ChannelMemberResponse, ChannelResponse, ChannelSnapshot, SendApprovalRequest, SendMessageRequest, StreamOptions};
use crate::state::AppState;
use axum::response::sse::Event;
use axum::response::Sse;
use crate::error::ApiError;
use crate::routes::channel_of;
use axum::{extract::{Query, State}, http::StatusCode};
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

/// How many messages a snapshot carries: the end of the conversation, which
/// is what a client needs to show it. Everything older is fetched over REST
/// (`GET .../messages`), which `has_more` tells the client to do.
const SNAPSHOT_MESSAGES: usize = 20;

pub fn channel_response(channel: &ChannelLoop) -> ChannelResponse {
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

/// The state of `channel` right now, for a stream that starts with a
/// snapshot (issue 34).
async fn channel_snapshot(channel: &ChannelLoop) -> ChannelSnapshot {
    let (messages, has_more) = recent_messages(channel.history().await);
    ChannelSnapshot { channel: channel_response(channel), messages, has_more }
}

/// The end of a history, and whether it was cut.
fn recent_messages(history: Vec<Message>) -> (Vec<Message>, bool) {
    let has_more = history.len() > SNAPSHOT_MESSAGES;
    let start = history.len().saturating_sub(SNAPSHOT_MESSAGES);
    (history.into_iter().skip(start).collect(), has_more)
}

/// The snapshot as an SSE event. Named, so that clients that do not know it
/// (and the events that follow, which stay unnamed) are unaffected.
async fn snapshot_event(channel: &ChannelLoop) -> Event {
    let snapshot = channel_snapshot(channel).await;
    Event::default().event("snapshot").data(serde_json::to_string(&snapshot).unwrap_or_default())
}

pub async fn join_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    Query(options): Query<StreamOptions>,
    user: AuthenticatedUser,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);

    let session_loop = crate::routes::ensure_session(&state, &user, session_id).await?;
    let channel = channel_of(&session_loop, channel_id)?;

    // Subscribe *before* reading the state: nothing that happens while the
    // snapshot is assembled is lost. The price is that an event can arrive
    // twice - once in the snapshot, once as an event - which clients drop by
    // message id.
    let receiver = channel.subscribe();
    let snapshot = if options.snapshot { Some(snapshot_event(&channel).await) } else { None };

    let with_snapshot = options.snapshot;
    let updates = BroadcastStream::new(receiver).then(move |res| {
        let channel = channel.clone();
        async move {
            match res {
                Ok(envelope) => {
                    let event_json = serde_json::to_string(&envelope).unwrap_or_default();
                    Ok(Event::default().data(event_json))
                }
                // The only error: this client fell behind and events were
                // dropped. A client that asked for snapshots gets a fresh one
                // - "here is the state" instead of "something is missing".
                Err(e) => {
                    tracing::warn!("SSE client of channel {} lost events: {}", channel_id, e);
                    if with_snapshot {
                        Ok(snapshot_event(&channel).await)
                    } else {
                        Ok(Event::default().comment(format!("events lost: {}", e)))
                    }
                }
            }
        }
    });

    let sse_stream = futures::stream::iter(snapshot.into_iter().map(Ok)).chain(updates);
    // The stream ends with the session token that authenticated it (issue 33).
    Ok(Sse::new(crate::sse::bounded_by_token(&user, sse_stream)))
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


#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::message::MessageId;

    fn history(count: usize) -> Vec<Message> {
        (0..count).map(|i| Message {
            id: MessageId::new(),
            content: Some(format!("message {i}")),
            ..Message::default()
        }).collect()
    }

    #[test]
    fn a_snapshot_carries_the_end_of_the_history() {
        let (messages, has_more) = recent_messages(history(SNAPSHOT_MESSAGES + 5));

        assert_eq!(messages.len(), SNAPSHOT_MESSAGES);
        assert!(has_more, "the client has to fetch the older ones over REST");
        assert_eq!(messages.first().unwrap().content.as_deref(), Some("message 5"));
        assert_eq!(messages.last().unwrap().content.as_deref(), Some(&format!("message {}", SNAPSHOT_MESSAGES + 4)[..]));
    }

    #[test]
    fn a_short_history_is_complete() {
        let (messages, has_more) = recent_messages(history(3));
        assert_eq!(messages.len(), 3);
        assert!(!has_more);

        let (messages, has_more) = recent_messages(Vec::new());
        assert!(messages.is_empty());
        assert!(!has_more);
    }
}
