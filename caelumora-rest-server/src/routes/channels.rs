use crate::auth::AuthenticatedUser;
use crate::models::{ChannelMemberResponse, ChannelResponse, ChannelSnapshot, CreateChannelRequest, CommandReplyResponse, ExecuteCommandRequest, MessageContextResponse, SendApprovalRequest, SendMessageRequest, StreamOptions};
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
use caelumora_runtime::session::{ChannelSpec, SessionError};
use caelumora_model::approval::{ApprovalId, ApprovalResponse};
use caelumora_model::command::{Command, CommandVisibility};
use caelumora_runtime::command::CommandError;
use caelumora_model::message::{Message, MessageId};
use caelumora_model::security::SecurityContext;
use caelumora_runtime::actor::MessageContextError;
use caelumora_model::session::{ChannelId, SessionId};
use futures::Stream;
use futures::StreamExt;
use std::convert::Infallible;
use std::sync::Arc;
use uuid::Uuid;

pub async fn list_channels(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<ChannelResponse>>, ApiError> {
    let session_id = SessionId(id);
    let session = crate::routes::ensure_session(&state, &user, session_id).await?;

    let context = user.security_context();
    let mut channels = Vec::new();
    for channel in session.channels() {
        channels.push(channel_response(&channel, &context).await);
    }
    Ok(Json(channels))
}

/// Creates a channel in the session and starts it (issue 09). Only who may
/// change the session may do it.
pub async fn create_channel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    user: AuthenticatedUser,
    Json(payload): Json<CreateChannelRequest>,
) -> Result<Json<ChannelResponse>, ApiError> {
    let session_id = SessionId(id);
    let session = crate::routes::ensure_session(&state, &user, session_id).await?;
    let context = user.security_context();

    let spec = ChannelSpec { name: payload.name, moderator: payload.moderator, participants: payload.participants };
    let channel = session.create_channel(&context, spec).await
        .map_err(|e| channel_error(e, session_id, "create a channel in", "creating a channel in"))?;
    Ok(Json(channel_response(&channel, &context).await))
}

/// Stops and deletes a channel of the session, with its history (issue 09).
pub async fn delete_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let session_id = SessionId(id);
    let session = crate::routes::ensure_session(&state, &user, session_id).await?;

    session.delete_channel(&user.security_context(), ChannelId(cid)).await
        .map_err(|e| channel_error(e, session_id, "delete a channel of", "deleting a channel of"))?;
    Ok(StatusCode::NO_CONTENT)
}

/// The answer for a failure to create or delete a channel: a request that
/// cannot be met is the client's (`400`, `404`, `409`); refusals and
/// everything else are answered as for the session.
fn channel_error(error: anyhow::Error, session_id: SessionId, act: &str, doing: &str) -> ApiError {
    let detail = error.to_string();
    match error.downcast_ref::<SessionError>() {
        Some(SessionError::ChannelNotFound { .. }) => ApiError::not_found("channel_not_found", detail),
        Some(SessionError::DuplicateChannelName { .. }) => ApiError::conflict("channel_exists", detail),
        Some(SessionError::EmptyChannelName | SessionError::UnknownModerator(_) | SessionError::NotAParticipant { .. }) => {
            ApiError::new(StatusCode::BAD_REQUEST, "invalid_channel", detail)
        }
        None => crate::routes::session_error(error, session_id, act, doing),
    }
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

    Ok(Json(channel_response(&channel, &user.security_context()).await))
}

/// How many messages a snapshot carries: the end of the conversation, which
/// is what a client needs to show it. Everything older is fetched over REST
/// (`GET .../messages`), which `has_more` tells the client to do.
const SNAPSHOT_MESSAGES: usize = 20;

/// A running channel as `viewer` sees it: its members, with what the viewer
/// may do with each (see `ChannelMemberResponse::can_read_context`).
pub async fn channel_response(channel: &ChannelLoop, viewer: &SecurityContext) -> ChannelResponse {
    let mut members = Vec::new();
    for member in channel.members() {
        let can_read_context = match channel.actor_by_id(member.id) {
            Some(actor) => actor.may_read_context(viewer).await,
            None => false,
        };
        members.push(ChannelMemberResponse { id: member.id, name: member.name, handle: member.handle, kind: member.kind, can_read_context });
    }
    ChannelResponse { id: channel.id(), name: channel.name(), members, moderator: channel.moderator() }
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
async fn channel_snapshot(channel: &ChannelLoop, viewer: &SecurityContext) -> ChannelSnapshot {
    let (messages, has_more) = recent_messages(channel.history().await);
    ChannelSnapshot { channel: channel_response(channel, viewer).await, messages, has_more }
}

/// The end of a history, and whether it was cut.
fn recent_messages(history: Vec<Message>) -> (Vec<Message>, bool) {
    let has_more = history.len() > SNAPSHOT_MESSAGES;
    let start = history.len().saturating_sub(SNAPSHOT_MESSAGES);
    (history.into_iter().skip(start).collect(), has_more)
}

/// The snapshot as an SSE event. Named, so that clients that do not know it
/// (and the events that follow, which stay unnamed) are unaffected.
async fn snapshot_event(channel: &ChannelLoop, viewer: &SecurityContext) -> Event {
    let snapshot = channel_snapshot(channel, viewer).await;
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
    let viewer = user.security_context();
    let snapshot = if options.snapshot { Some(snapshot_event(&channel, &viewer).await) } else { None };

    // A client that falls behind gets a fresh snapshot, if it asked for them.
    // Held weakly: a deleted channel must be dropped, which closes its event
    // sender and so ends this stream (issue 09).
    let channel = Arc::downgrade(&channel);
    let resync = options.snapshot.then(|| {
        move || {
            let channel = channel.upgrade();
            let viewer = viewer.clone();
            async move {
                match channel {
                    Some(channel) => snapshot_event(&channel, &viewer).await,
                    None => Event::default().comment("the channel was deleted"),
                }
            }
        }
    });
    let updates = crate::sse::resync_on_lag(receiver, format!("channel {}", channel_id), resync);

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
    let user_actor = user_actor_of(&channel, &user)?;
    channel.wake_for_delivery().await
        .map_err(|e| ApiError::internal("channel_wakeup_failed", "waking the channel's agents", &e))?;
    let span_id = user_actor.send_message(payload.content).await
        .map_err(|e| ApiError::internal("message_not_sent", "sending the message", &e))?;
    tracing::Span::current().record("span_id", tracing::field::display(span_id));
    Ok(StatusCode::OK)
}

/// The actor the authenticated user acts through in `channel` (issue 30).
///
/// Whoever writes or approves does so in their own name, so someone who is
/// not a participant of the channel has no actor here and is refused. Until
/// participants can be added to a running channel ([46](issues/46-channel-participants.md)),
/// that means the participants a channel was created with.
fn user_actor_of(channel: &ChannelLoop, user: &AuthenticatedUser) -> Result<Arc<UserActor>, ApiError> {
    channel.user_actor(user.id).ok_or_else(|| {
        ApiError::forbidden("not_a_participant", format!("You are not a participant of channel {}", channel.id()))
    })
}

pub async fn cancel_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session = crate::routes::ensure_session(&state, &user, session_id).await?;

    // Cancelling ends the turn of every agent in the channel, so it is an
    // action in the channel like writing into it - participants only.
    let channel = channel_of(&session, channel_id)?;
    user_actor_of(&channel, &user)?;
    session.cancel_channel(channel_id, &user.security_context()).await
        .map_err(cancel_error)?;
    Ok(StatusCode::OK)
}

/// A refused cancel is an answer, not a failure: `403` with what `user_message`
/// discloses, which for a `ChannelError` is the refusal itself.
fn cancel_error(error: anyhow::Error) -> ApiError {
    if caelumora_runtime::error::is_forbidden(&error) {
        let detail = caelumora_runtime::error::user_message(&error)
            .unwrap_or_else(|| "Cancelling this channel is not permitted".to_string());
        return ApiError::forbidden("cancel_forbidden", detail);
    }
    ApiError::internal("cancel_failed", "cancelling the channel", &error)
}

/// The commands the authenticated user may see in the channel, for
/// completion (issue 28).
pub async fn list_commands(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<Command>>, ApiError> {
    let session = crate::routes::ensure_session(&state, &user, SessionId(id)).await?;
    let channel = channel_of(&session, ChannelId(cid))?;
    let user_actor = user_actor_of(&channel, &user)?;
    Ok(Json(user_actor.visible_commands().await))
}

/// Runs a user command in the channel (issue 28). Commands are recognised by
/// the client - a message that starts with a slash is sent here instead of to
/// `messages` - and never enter the conversation.
pub async fn execute_command(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<ExecuteCommandRequest>,
) -> Result<Json<CommandReplyResponse>, ApiError> {
    let session = crate::routes::ensure_session(&state, &user, SessionId(id)).await?;
    let channel = channel_of(&session, ChannelId(cid))?;
    let user_actor = user_actor_of(&channel, &user)?;
    let reply = user_actor.execute_command(&payload.name, &payload.args).await
        .map_err(command_error)?;
    // An answer for the whole channel reaches this client on the channel's
    // stream, like everybody else; returning it here as well would show it twice.
    let output = (reply.visibility == CommandVisibility::Private).then_some(reply.output);
    Ok(Json(CommandReplyResponse { command: reply.command, output }))
}

/// A command that did not run is the user's answer, not a server failure -
/// except when it failed, which is `500` as for anything else.
fn command_error(error: anyhow::Error) -> ApiError {
    let detail = error.to_string();
    match error.downcast_ref::<CommandError>() {
        Some(CommandError::Empty | CommandError::Unknown(_)) => ApiError::new(StatusCode::BAD_REQUEST, "unknown_command", detail),
        Some(CommandError::InvalidArguments { .. }) => ApiError::new(StatusCode::BAD_REQUEST, "invalid_command_arguments", detail),
        Some(CommandError::Forbidden(_)) => ApiError::forbidden("command_forbidden", detail),
        _ if caelumora_runtime::error::is_forbidden(&error) => {
            let detail = caelumora_runtime::error::user_message(&error)
                .unwrap_or_else(|| "Running this command is not permitted".to_string());
            ApiError::forbidden("command_forbidden", detail)
        }
        _ => ApiError::internal("command_failed", "running the command", &error),
    }
}

/// What the actor `aid` saw when it produced message `mid` (issue 06): its
/// context and system prompt. The actor answers: only the agent that
/// produced a message has a context for it.
pub async fn message_context(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid, mid)): Path<(Uuid, Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<MessageContextResponse>, ApiError> {
    let session = crate::routes::ensure_session(&state, &user, SessionId(id)).await?;
    let channel = channel_of(&session, ChannelId(cid))?;
    let actor = channel.actor_ids().into_iter()
        .find(|actor| actor.id() == aid)
        .and_then(|actor| channel.actor_by_id(actor))
        .ok_or_else(|| ApiError::not_found("actor_not_found", format!("Actor {} is not in channel {}", aid, cid)))?;
    let seen = actor.message_context(&user.security_context(), MessageId(mid)).await
        .map_err(message_context_error)?;
    Ok(Json(MessageContextResponse {
        message_id: MessageId(mid),
        actor_id: actor.id(),
        context_digest: seen.reference.context,
        system_prompt_digest: seen.reference.system_prompt,
        system_prompt: seen.system_prompt,
        messages: seen.messages,
    }))
}

fn message_context_error(error: MessageContextError) -> ApiError {
    let detail = error.to_string();
    match error {
        MessageContextError::Forbidden { .. } => ApiError::forbidden("context_forbidden", detail),
        MessageContextError::NotRecorded { .. } => ApiError::not_found("context_not_recorded", detail),
        MessageContextError::Unavailable { .. } => ApiError::not_found("context_unavailable", detail),
        MessageContextError::Failed { .. } => ApiError::internal("context_failed", "reading the context", &anyhow::Error::new(error)),
    }
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
    let user_actor = user_actor_of(&channel, &user)?;
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

    #[test]
    fn a_command_that_does_not_run_is_answered_with_what_the_user_did() {
        let status = |error: CommandError| command_error(anyhow::Error::new(error)).status();
        assert_eq!(status(CommandError::Unknown("x".to_string())), StatusCode::BAD_REQUEST);
        assert_eq!(status(CommandError::Empty), StatusCode::BAD_REQUEST);
        assert_eq!(status(CommandError::InvalidArguments { command: "x".to_string(), reason: "y".to_string() }), StatusCode::BAD_REQUEST);
        assert_eq!(status(CommandError::Forbidden("x".to_string())), StatusCode::FORBIDDEN);
        assert_eq!(status(CommandError::Failed { command: "x".to_string(), reason: "y".to_string() }), StatusCode::INTERNAL_SERVER_ERROR);

        let refused = anyhow::Error::new(caelumora_model::channel::ChannelError::Forbidden { action: "waking the agents of", channel: ChannelId::default() });
        assert_eq!(command_error(refused).status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn a_context_that_cannot_be_shown_says_why() {
        let actor = caelumora_model::actor::ActorId::None;
        let message = MessageId::new();
        let status = |error| message_context_error(error).status();
        assert_eq!(status(MessageContextError::Forbidden { actor }), StatusCode::FORBIDDEN);
        assert_eq!(status(MessageContextError::NotRecorded { actor, message }), StatusCode::NOT_FOUND);
        assert_eq!(status(MessageContextError::Unavailable { actor, message }), StatusCode::NOT_FOUND);
        assert_eq!(status(MessageContextError::Failed { actor, message }), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
