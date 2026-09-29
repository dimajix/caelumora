use crate::auth::AuthenticatedUser;
use crate::models::{AddParticipantRequest, ChannelMemberResponse, ChannelResponse, ChannelSnapshot, CreateChannelRequest, KnowledgeRefResponse, CommandReplyResponse, ExecuteCommandRequest, MessageContextResponse, PublishedNotesResponse, SendApprovalRequest, SendMessageRequest, StreamOptions};
use crate::state::AppState;
use axum::response::sse::Event;
use axum::response::Sse;
use crate::error::ApiError;
use crate::routes::channel_of;
use axum::{extract::{Query, State}, http::StatusCode};
use crate::error::{Json, Path};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentId;
use caelumora_model::event::GlobalEvent;
use caelumora_runtime::actor_impl::user::UserActor;
use caelumora_runtime::channel::ChannelLoop;
use caelumora_runtime::session::{ChannelSpec, SessionError, SessionLoop};
use caelumora_model::message::{KnowledgeRef, KnowledgeSource};
use caelumora_model::approval::{ApprovalId, ApprovalResponse};
use caelumora_model::command::{Command, CommandVisibility};
use caelumora_runtime::command::CommandError;
use caelumora_model::message::{Message, MessageId};
use caelumora_model::security::{AccessQuestions, SecurityContext};
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
        // Only the channels the user may read (issue 70).
        if crate::routes::may_read_channel(&state, &channel, &context).await {
            channels.push(channel_response(&state, &channel, &context).await);
        }
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

    let spec = ChannelSpec { name: payload.name, moderator: payload.moderator, participants: payload.participants, confidentiality: payload.confidentiality };
    let channel = session.create_channel(&context, spec).await
        .map_err(|e| channel_error(e, session_id, "create a channel in", "creating a channel in"))?;
    Ok(Json(channel_response(&state, &channel, &context).await))
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
        Some(SessionError::EmptyChannelName | SessionError::UnknownModerator(_) | SessionError::NotAParticipant { .. } | SessionError::InvalidConfidentiality(_)) => {
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
    
    let channel = channel_of(&state, &session, channel_id, &user.security_context()).await?;

    Ok(Json(channel_response(&state, &channel, &user.security_context()).await))
}

/// How many messages a snapshot carries: the end of the conversation, which
/// is what a client needs to show it. Everything older is fetched over REST
/// (`GET .../messages`), which `has_more` tells the client to do.
const SNAPSHOT_MESSAGES: usize = 20;

/// A running channel as `viewer` sees it: its members, with what the viewer
/// may do with each (see `ChannelMemberResponse::can_read_context`).
pub async fn channel_response(state: &AppState, channel: &ChannelLoop, viewer: &SecurityContext) -> ChannelResponse {
    let security = state.runtime_manager.security_manager();
    let mut members = Vec::new();
    for member in channel.members() {
        let can_read_context = match channel.actor_by_id(member.id) {
            Some(actor) => actor.may_read_context(viewer).await,
            None => false,
        };
        members.push(ChannelMemberResponse { id: member.id, name: member.name, handle: member.handle, kind: member.kind, can_read_context });
    }
    ChannelResponse { id: channel.id(), name: channel.name(), members, moderator: channel.moderator(), confidentiality: channel.classification().level, can_change: security.may_change_channel(viewer, &channel.security_attributes()).await }
}

pub async fn get_channel_messages(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<Message>>, ApiError> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session = crate::routes::ensure_session(&state, &user, session_id).await?;
    let channel = channel_of(&state, &session, channel_id, &user.security_context()).await?;

    // As much of it as the user may see (issue 46).
    let messages = channel.visible_history(user.security_context().initiator_id).await.messages;

    Ok(Json(messages))
}

/// The notes every agent of the channel published (issue 17) - what a
/// client reloads on `GlobalEvent::AgentChannelStateChanged` (issue 65).
pub async fn get_notes(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<PublishedNotesResponse>>, ApiError> {
    let session_id = SessionId(id);
    let session = crate::routes::ensure_session(&state, &user, session_id).await?;
    let channel = channel_of(&state, &session, ChannelId(cid), &user.security_context()).await?;
    let notes = channel.published_notes().await
        .map_err(|e| ApiError::internal("notes_failed", &format!("reading the notes of channel {}", cid), &e))?;
    Ok(Json(notes.into_iter().map(Into::into).collect()))
}

/// The state of `channel` right now, for a stream that starts with a
/// snapshot (issue 34).
async fn channel_snapshot(state: &AppState, channel: &ChannelLoop, viewer: &SecurityContext) -> ChannelSnapshot {
    let (messages, has_more) = recent_messages(channel.visible_history(viewer.initiator_id).await.messages);
    let notes = channel.published_notes().await.unwrap_or_else(|e| {
        // The conversation can be shown without them.
        tracing::warn!("Reading the notes of channel {} failed: {:#}", channel.id(), e);
        Vec::new()
    });
    ChannelSnapshot {
        channel: channel_response(state, channel, viewer).await,
        messages,
        has_more,
        notes: notes.into_iter().map(Into::into).collect(),
    }
}

/// The end of a history, and whether it was cut.
fn recent_messages(history: Vec<Message>) -> (Vec<Message>, bool) {
    let has_more = history.len() > SNAPSHOT_MESSAGES;
    let start = history.len().saturating_sub(SNAPSHOT_MESSAGES);
    (history.into_iter().skip(start).collect(), has_more)
}

/// The snapshot as an SSE event. Named, so that clients that do not know it
/// (and the events that follow, which stay unnamed) are unaffected.
async fn snapshot_event(state: &AppState, channel: &ChannelLoop, viewer: &SecurityContext) -> Event {
    let snapshot = channel_snapshot(state, channel, viewer).await;
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
    let channel = channel_of(&state, &session_loop, channel_id, &user.security_context()).await?;

    // Subscribe *before* reading the state: nothing that happens while the
    // snapshot is assembled is lost. The price is that an event can arrive
    // twice - once in the snapshot, once as an event - which clients drop by
    // message id.
    let receiver = channel.subscribe();
    // Membership is announced on the event bus (issue 73).
    let mut memberships = state.runtime_manager.event_bus().subscribe_channel(channel_id).into_receiver();
    let membership_of = Arc::downgrade(&channel);
    let viewer = user.security_context();
    let snapshot = if options.snapshot { Some(snapshot_event(&state, &channel, &viewer).await) } else { None };

    // A client that falls behind gets a fresh snapshot, if it asked for them.
    // Held weakly: a deleted channel must be dropped, which closes its event
    // sender and so ends this stream (issue 09).
    let channel = Arc::downgrade(&channel);
    let resync_state = state.clone();
    let resync = options.snapshot.then(|| {
        move || {
            let channel = channel.upgrade();
            let viewer = viewer.clone();
            let state = resync_state.clone();
            async move {
                match channel {
                    Some(channel) => snapshot_event(&state, &channel, &viewer).await,
                    None => Event::default().comment("the channel was deleted"),
                }
            }
        }
    });
    let updates = crate::sse::resync_on_lag(receiver, format!("channel {}", channel_id), resync);
    // A user removed from the channel stops receiving it (issue 46). Should
    // the announcement be lost to a lag, whether they still take part is
    // asked instead; on reconnecting, `channel_of` answers for them.
    let me = viewer_id(&user);
    let updates = updates.take_until(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match memberships.recv().await {
                Ok(GlobalEvent::UserChannelLeft(_, cid, uid)) if cid == channel_id && ActorId::User(uid) == me => break,
                Ok(_) => continue,
                Err(RecvError::Lagged(_)) => {
                    if membership_of.upgrade().is_none_or(|c| c.actor_by_id(me).is_none()) {
                        break;
                    }
                }
                Err(RecvError::Closed) => break,
            }
        }
    });

    let sse_stream = futures::stream::iter(snapshot.into_iter().map(Ok)).chain(updates);
    // The stream ends with the session token that authenticated it (issue 33).
    Ok(Sse::new(crate::sse::bounded_by_token(&user, sse_stream)))
}

/// The actor the authenticated user is.
fn viewer_id(user: &AuthenticatedUser) -> ActorId {
    user.security_context().initiator_id
}

/// Adds a participant of the session to the channel (issue 46), with as
/// much of its history as the request grants. Only who may change the
/// channel may do it.
pub async fn add_participant(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<AddParticipantRequest>,
) -> Result<Json<ChannelResponse>, ApiError> {
    let session_id = SessionId(id);
    let session = crate::routes::ensure_session(&state, &user, session_id).await?;
    let context = user.security_context();
    let channel = channel_of(&state, &session, ChannelId(cid), &context).await?;
    channel.add_participant(&context, payload.participant, payload.history).await
        .map_err(|e| participant_error(e, session_id, "add participants to", "adding a participant to"))?;
    Ok(Json(channel_response(&state, &channel, &context).await))
}

/// Removes a participant from the channel (issue 46); `aid` is the id of the
/// user or agent. Only who may change the channel may do it.
pub async fn remove_participant(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let session_id = SessionId(id);
    let session = crate::routes::ensure_session(&state, &user, session_id).await?;
    let context = user.security_context();
    let channel = channel_of(&state, &session, ChannelId(cid), &context).await?;
    let participant = channel.members().into_iter().map(|m| m.id).find(|m| m.id() == aid)
        .ok_or_else(|| ApiError::not_found("participant_not_found", format!("{} takes no part in channel {}", aid, cid)))?;
    channel.remove_participant(&context, participant).await
        .map_err(|e| participant_error(e, session_id, "remove participants from", "removing a participant from"))?;
    Ok(StatusCode::NO_CONTENT)
}

/// The answer for a failure to add or remove a participant: a request that
/// cannot be met is the client's; refusals and everything else are answered
/// as for the session.
fn participant_error(error: anyhow::Error, session_id: SessionId, act: &str, doing: &str) -> ApiError {
    use caelumora_runtime::channel::ChannelError;
    let detail = error.to_string();
    match error.downcast_ref::<ChannelError>() {
        Some(ChannelError::AlreadyParticipant { .. }) => ApiError::conflict("already_participant", detail),
        Some(ChannelError::NotAParticipant { .. } | ChannelError::ParticipantGone { .. }) => ApiError::not_found("participant_not_found", detail),
        Some(ChannelError::NotInSession { .. } | ChannelError::ApproverStays { .. }) => {
            ApiError::new(StatusCode::BAD_REQUEST, "invalid_participant", detail)
        }
        Some(ChannelError::Forbidden { .. }) => ApiError::forbidden("participant_forbidden", detail),
        None => crate::routes::session_error(error, session_id, act, doing),
    }
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
    
    let channel = channel_of(&state, &session, channel_id, &user.security_context()).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    channel.wake_for_delivery();
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
    let channel = channel_of(&state, &session, channel_id, &user.security_context()).await?;
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
    let channel = channel_of(&state, &session, ChannelId(cid), &user.security_context()).await?;
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
    let channel = channel_of(&state, &session, ChannelId(cid), &user.security_context()).await?;
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

/// What the actor `aid` saw when it produced message `mid` (issues 06, 47):
/// its context with the prompt parts where they were placed. The actor answers: only the agent that
/// produced a message has a context for it.
pub async fn message_context(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid, mid)): Path<(Uuid, Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<MessageContextResponse>, ApiError> {
    let session = crate::routes::ensure_session(&state, &user, SessionId(id)).await?;
    let channel = channel_of(&state, &session, ChannelId(cid), &user.security_context()).await?;
    let actor = channel.actor_ids().into_iter()
        .find(|actor| actor.id() == aid)
        .and_then(|actor| channel.actor_by_id(actor))
        .ok_or_else(|| ApiError::not_found("actor_not_found", format!("Actor {} is not in channel {}", aid, cid)))?;
    let seen = actor.message_context(&user.security_context(), MessageId(mid)).await
        .map_err(message_context_error)?;
    let knowledge = seen.reference.knowledge.iter().map(|k| knowledge_response(&session, k)).collect();
    Ok(Json(MessageContextResponse {
        message_id: MessageId(mid),
        actor_id: actor.id(),
        transcript_digest: seen.reference.transcript,
        items: seen.items.into_iter().map(Into::into).collect(),
        knowledge,
    }))
}

/// `knowledge` with the names of its channel and agent, as far as the
/// session still has them.
fn knowledge_response(session: &SessionLoop, knowledge: &KnowledgeRef) -> KnowledgeRefResponse {
    let channel = session.channel_by_id(knowledge.channel);
    let (source, agent_id, digest, history_start) = match knowledge.source {
        KnowledgeSource::Notes { agent, digest } => ("notes", Some(caelumora_model::actor::ActorId::Agent(agent)), digest, None),
        KnowledgeSource::PrivateNotes { agent, digest } => ("private_notes", Some(caelumora_model::actor::ActorId::Agent(agent)), digest, None),
        KnowledgeSource::History { digest, start } => ("history", None, digest, start),
        KnowledgeSource::Memory { agent, digest } => ("memory", Some(caelumora_model::actor::ActorId::Agent(agent)), digest, None),
    };
    let agent_name = agent_id.and_then(|id| channel.as_ref()?.members().into_iter().find(|m| m.id == id).map(|m| m.name));
    KnowledgeRefResponse {
        channel_id: knowledge.channel,
        channel_name: channel.map(|c| c.name()),
        source,
        agent_id,
        agent_name,
        digest,
        history_start,
    }
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

    let channel = channel_of(&state, &session, channel_id, &user.security_context()).await?;
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

        let refused = anyhow::Error::new(caelumora_runtime::channel::ChannelError::Forbidden { action: "waking the agents of", channel: ChannelId::default() });
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
