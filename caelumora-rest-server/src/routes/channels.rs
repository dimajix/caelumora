use crate::auth::AuthenticatedUser;
use crate::models::{AddParticipantRequest, DescriptionRequest, AgentToolsResponse, DeletedResponse, ListOptions, ChannelMemberResponse, ChannelResponse, ChannelSnapshot, CreateChannelRequest, KnowledgeRefResponse, CommandReplyResponse, ExecuteCommandRequest, GivenUpTurnResponse, HistoryMessageResponse, LabelCauses, MessageContextResponse, SeenItemResponse, PublishedNotesResponse, MessageRequest, RetryTurnRequest, SendApprovalRequest, StreamOptions};
use crate::state::AppState;
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response, Sse};
use crate::error::{ApiError, Subject};
use axum::{extract::{Query, State}, http::StatusCode};
use crate::error::{Json, Path};
use caelumora_model::snapshot::Detail;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentId;
use caelumora_runtime::actor_impl::user::UserActor;
use caelumora_runtime::channel::ChannelLoop;
use caelumora_runtime::session::ChannelSpec;
use caelumora_runtime::view::{AgentContextView, ChannelView, MemberView, Viewer};
use caelumora_model::approval::{ApprovalId, ApprovalResponse, ApprovalRule};
use caelumora_model::channel::moderation_components;
use caelumora_model::command::{Command, CommandVisibility};
use caelumora_model::confidentiality::Classification;
use caelumora_model::value::language::Language;
use caelumora_model::context::ContextItem;
use caelumora_model::message::{Message, MessageId};
use caelumora_model::security::SecurityContext;
use caelumora_model::session::{ChannelId, SessionId};
use futures::Stream;
use std::convert::Infallible;
use std::sync::Arc;
use uuid::Uuid;

/// The channels of the session the user may read - or, with
/// `?deleted=true`, those deleted from it they may read as such (issue 164).
pub async fn list_channels(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(options): Query<ListOptions>,
    user: AuthenticatedUser,
) -> Result<Response, ApiError> {
    let session_id = SessionId(id);
    // Only the channels the user may read (issue 70), as they see them
    // (issue 80).
    let session = crate::routes::views(&state, &user).session(session_id).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Session, &format!("listing the channels of session {session_id}")))?;
    if options.deleted {
        let deleted = session.deleted_channels().await
            .map_err(|e| ApiError::from_runtime(e.into(), Subject::Session, &format!("listing the deleted channels of session {session_id}")))?;
        return Ok(Json(deleted.into_iter().map(DeletedResponse::from).collect::<Vec<_>>()).into_response());
    }
    let mut channels: Vec<ChannelResponse> = Vec::new();
    for channel in session.channels().await {
        channels.push(channel.snapshot(Detail::MEMBERS).await.into());
    }
    Ok(Json(channels).into_response())
}

/// Restores a channel deleted from the session (issue 164), starts it again
/// and answers it as `GET` does. Only who may change the session and
/// restore the channel may.
pub async fn restore_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<ChannelResponse>, ApiError> {
    let session_id = SessionId(id);
    let session = crate::routes::ensure_session(&state, &user, session_id).await?;
    session.restore_channel(&user.security_context(), ChannelId(cid)).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, &format!("restoring channel {cid} of session {session_id}")))?;
    Ok(Json(crate::routes::channel_view(&state, &user, session_id, ChannelId(cid)).await?))
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

    let components = moderation_components(payload.moderator.as_deref(), payload.responder_policy.as_deref());
    let spec = ChannelSpec { name: payload.name, components, participants: payload.participants, classification: payload.classification, language: payload.language };
    let channel = session.create_channel(&context, spec).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, &format!("creating a channel in session {session_id}")))?;
    Ok(Json(crate::routes::channel_view(&state, &user, session_id, channel.id()).await?))
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
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, &format!("deleting a channel of session {session_id}")))?;
    Ok(StatusCode::NO_CONTENT)
}


pub async fn get_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<ChannelResponse>, ApiError> {
    Ok(Json(crate::routes::channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?))
}

/// How many messages a snapshot carries: the end of the conversation, which
/// is what a client needs to show it. Everything older is fetched over REST
/// (`GET .../messages`), which `has_more` tells the client to do.
const SNAPSHOT_MESSAGES: usize = 20;

pub async fn get_channel_messages(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<Message>>, ApiError> {
    let channel = crate::routes::channel_of(&state, &user, SessionId(id), ChannelId(cid)).await?;
    // As much of it as the user may see (issue 46).
    let history = channel.history().await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Session, &format!("reading the history of channel {cid}")))?;
    Ok(Json(history.messages))
}

/// The notes every agent of the channel published (issue 17) - what a
/// client reloads on `GlobalEvent::AgentChannelStateChanged` (issue 65).
pub async fn get_notes(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<PublishedNotesResponse>>, ApiError> {
    let channel = crate::routes::channel_of(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let notes = channel.notes().await
        .map_err(|e| ApiError::internal("notes_failed", &format!("reading the notes of channel {}", cid), &e.into()))?;
    Ok(Json(notes.into_iter().map(Into::into).collect()))
}

/// The state of `channel` right now, for a stream that starts with a
/// snapshot (issue 34); `None` if the channel cannot be described to
/// `viewer` any more - deleted, or no longer theirs to read - in which case
/// their watch ends.
async fn channel_snapshot(state: &AppState, channel: &ChannelLoop, viewer: &SecurityContext) -> Option<ChannelSnapshot> {
    let views = state.session_manager.views(Viewer::new(viewer.clone()));
    let reached = match views.session(channel.session_id()).await {
        Ok(session) => session.channel(channel.id()).await,
        Err(e) => Err(e),
    };
    let view = match reached {
        Ok(view) => view,
        Err(e) => {
            tracing::warn!("Cannot describe channel {} for its snapshot: {:#}", channel.id(), e);
            return None;
        }
    };
    let history = match view.history().await {
        Ok(history) => history,
        Err(e) => {
            tracing::warn!("Cannot read the history of channel {} for its snapshot: {:#}", channel.id(), e);
            return None;
        }
    };
    let (messages, has_more) = recent_messages(history.messages);
    // The conversation can be shown without what of it cannot be read.
    let channel = view.snapshot(Detail::MEMBERS | Detail::USAGE | Detail::NOTES | Detail::APPROVALS | Detail::BRIEFING).await;
    Some(ChannelSnapshot::of(channel, messages, has_more))
}

/// The end of a history, and whether it was cut.
fn recent_messages(history: Vec<Message>) -> (Vec<Message>, bool) {
    let has_more = history.len() > SNAPSHOT_MESSAGES;
    let start = history.len().saturating_sub(SNAPSHOT_MESSAGES);
    (history.into_iter().skip(start).collect(), has_more)
}

/// Follows a channel live (issue 104): only as one of its participants,
/// through the user's own actor - the stream ends when the user leaves the
/// channel, or it stops or is deleted. Starts with a snapshot if asked.
pub async fn join_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    Query(options): Query<StreamOptions>,
    user: AuthenticatedUser,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let viewer = user.security_context();
    // Watched *before* the state is read: nothing that happens while the
    // snapshot is assembled is lost. The price is that an event can arrive
    // twice - once in the snapshot, once as an event - which clients drop by
    // message id.
    let watch = user_actor_of(&channel, &user)?.watch();
    let snapshot = move || {
        let (state, channel, viewer) = (Arc::clone(&state), Arc::clone(&channel), viewer.clone());
        async move { channel_snapshot(&state, &channel, &viewer).await }
    };
    Ok(Sse::new(crate::sse::stream(&user, options.snapshot, watch.events, format!("channel {}", cid), snapshot, watch.ended.cancelled_owned()).await))
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
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let context = user.security_context();
    channel.add_participant(&context, payload.participant, payload.history).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Participant, &format!("adding a participant to channel {cid}")))?;
    Ok(Json(crate::routes::channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?))
}

/// Removes a participant from the channel (issue 46); `aid` is the id of the
/// user or agent. Only who may change the channel may do it.
/// One participant of the channel, with what its model calls used (issue
/// 209) - what a client fetches again when an agent's turn ends, instead
/// of the whole channel.
pub async fn get_participant(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<ChannelMemberResponse>, ApiError> {
    let channel = crate::routes::channel_of(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let member = member_of(&channel, aid).await?.snapshot(Detail::USAGE | Detail::PROFILE).await;
    ChannelMemberResponse::of(member).map(Json)
        .ok_or_else(|| ApiError::not_found("actor_not_found", format!("Actor {} is not in channel {}", aid, cid)))
}

pub async fn remove_participant(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let context = user.security_context();
    let participant = actor_in(&channel, aid)
        .ok_or_else(|| ApiError::not_found("participant_not_found", format!("{} takes no part in channel {}", aid, cid)))?;
    channel.remove_participant(&context, participant).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Participant, &format!("removing a participant from channel {cid}")))?;
    Ok(StatusCode::NO_CONTENT)
}


pub async fn send_message(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<MessageRequest>,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor.send_message(payload.content, payload.classification).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, "sending the message"))?;
    tracing::Span::current().record("span_id", tracing::field::display(span_id));
    Ok(StatusCode::OK)
}

/// Modifies the authenticated user's message `mid` (issue 234): it reads as
/// `content` from now on, in its place. Answered once it is on its way; the
/// channel's stream shows it as `MessageModified`.
pub async fn modify_message(
    State(state): State<Arc<AppState>>,
    Path((id, cid, mid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<MessageRequest>,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor.modify_message(MessageId(mid), payload.content, payload.classification).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, "modifying the message"))?;
    tracing::Span::current().record("span_id", tracing::field::display(span_id));
    Ok(StatusCode::OK)
}

/// Asks the agent that said message `mid` to answer again instead of it
/// (issue 234). Answered once the request is on its way: whether the agent
/// does is its own to decide, and the channel's stream shows its answer as
/// `RetryAccepted` - followed by the message taken back and the new answer -
/// or `RetryRejected` with the reason.
pub async fn retry_message(
    State(state): State<Arc<AppState>>,
    Path((id, cid, mid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor.request_retry(MessageId(mid)).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, "asking for another answer"))?;
    tracing::Span::current().record("span_id", tracing::field::display(span_id));
    Ok(StatusCode::OK)
}

/// Asks participant `aid`, an agent, to answer again instead of its answer
/// under way, the turn the body names (issue 235). Refused at once if that
/// turn is not under way any more (`409 not_under_way`); otherwise the
/// channel ends the turn as it delivers the request, and the stream shows
/// `RetryAccepted` and the new answer.
pub async fn retry_turn(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<RetryTurnRequest>,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let participant = actor_in(&channel, aid)
        .ok_or_else(|| ApiError::not_found("participant_not_found", format!("{} takes no part in channel {}", aid, cid)))?;
    let span_id = user_actor.request_retry_of_turn(participant, payload.turn_id).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, "asking for another answer"))?;
    tracing::Span::current().record("span_id", tracing::field::display(span_id));
    Ok(StatusCode::OK)
}

/// Takes the authenticated user's message `mid` back (issue 234): it stays
/// in its place without what it said, and the agents are no longer given
/// it - not a deletion. Shown on the channel's stream as `MessageRevoked`.
pub async fn revoke_message(
    State(state): State<Arc<AppState>>,
    Path((id, cid, mid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor.revoke_message(MessageId(mid)).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, "taking back the message"))?;
    tracing::Span::current().record("span_id", tracing::field::display(span_id));
    Ok(StatusCode::NO_CONTENT)
}

/// The actor of `channel` whose id is `id`, whatever kind it is - a path
/// names an actor by its UUID alone.
fn actor_in(channel: &ChannelLoop, id: Uuid) -> Option<ActorId> {
    channel.actor_ids().into_iter().find(|actor| actor.id() == id)
}

/// The member of `channel` whose id is `id`, whatever kind it is - or a
/// former member (issue 164), whose history and context stay readable.
async fn member_of(channel: &ChannelView, id: Uuid) -> Result<MemberView, ApiError> {
    let mut members = channel.members().await;
    members.extend(channel.former_members().await);
    if let Some(member) = members.into_iter().find(|member| member.id().id() == id) {
        return Ok(member);
    }
    // An agent that left before the channel kept who left is known by what
    // it said.
    channel.member(ActorId::Agent(AgentId(id))).await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Participant, "finding the member"))
}

/// The actor the authenticated user acts through in `channel` (issue 30).
///
/// Whoever writes or approves does so in their own name, so someone who is
/// not a participant of the channel has no actor here and is refused.
fn user_actor_of(channel: &ChannelLoop, user: &AuthenticatedUser) -> Result<Arc<UserActor>, ApiError> {
    channel.user_actor(user.id).ok_or_else(|| {
        ApiError::forbidden("not_a_participant", format!("You are not a participant of channel {}", channel.id()))
    })
}

/// Ends the turn of every agent in the channel - an operation on the
/// channel, like renaming it, not an act of taking part: whoever the policy
/// lets cancel it may (`CancelChannel`, checked by the channel).
pub async fn cancel_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    channel.cancel(&user.security_context()).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Cancel, "cancelling the channel"))?;
    Ok(StatusCode::OK)
}

/// Ends the turn of one participant of the channel (issue 235), under the
/// same permission as cancelling the channel (`CancelChannel`, checked by
/// the channel). The others go on, and the channel stays answering.
pub async fn cancel_participant(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let participant = actor_in(&channel, aid)
        .ok_or_else(|| ApiError::not_found("participant_not_found", format!("{} takes no part in channel {}", aid, cid)))?;
    channel.cancel_actor(&user.security_context(), participant).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Cancel, &format!("cancelling a participant of channel {cid}")))?;
    Ok(StatusCode::OK)
}


/// Reclassifies the channel (issue 133): only its owner. Lowering it
/// releases what is in it. Answers with the channel as the user sees it now.
pub async fn reclassify_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(classification): Json<Classification>,
) -> Result<Json<ChannelResponse>, ApiError> {
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    channel.reclassify(&user.security_context(), classification).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, "reclassifying the channel"))?;
    Ok(Json(crate::routes::channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?))
}

/// Has the channel's conversation held in the language the body names - a
/// BCP 47 tag such as `"de"` - or in none in particular (`null`, issue
/// 56): whoever may change the channel. Its agents are told from their next
/// turn on. Answers with the channel as the user sees it now.
pub async fn set_channel_language(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(language): Json<Option<Language>>,
) -> Result<Json<ChannelResponse>, ApiError> {
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    channel.update_language(&user.security_context(), language).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, "changing the language of the channel"))?;
    Ok(Json(crate::routes::channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?))
}

/// Sets what the channel restricts of the tool calls made in it (issue 13):
/// rules that ask or deny, replacing those it had - whoever may change the
/// channel. Answers `400 invalid_rule` for a rule that would allow, is
/// narrower than the channel or cannot be matched, and otherwise the
/// channel as the user sees it now.
pub async fn update_channel_approval_rules(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(rules): Json<Vec<ApprovalRule>>,
) -> Result<Json<ChannelResponse>, ApiError> {
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    channel.update_approval_rules(&user.security_context(), rules).await
        .map_err(|e| crate::routes::change_error(e, "invalid_rule", Subject::Session, "changing the approval rules of the channel"))?;
    Ok(Json(crate::routes::channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?))
}

/// Says what the channel is for (issue 245): its summary and briefing,
/// `null` or empty to remove one - whoever may change the channel. Its
/// agents are told from their next turn on. Answers `400
/// invalid_description` for one that is too long or holds what it may not,
/// and otherwise the channel as the user sees it now.
pub async fn update_channel_description(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<DescriptionRequest>,
) -> Result<Json<ChannelResponse>, ApiError> {
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let (summary, briefing) = payload.description()?;
    channel.update_description(&user.security_context(), summary, briefing).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, "changing the description of the channel"))?;
    Ok(Json(crate::routes::channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?))
}

/// Reclassifies a message of the channel (issues 133, 239): its author or
/// the channel's owner, never above the channel - as a participant, through
/// their actor, as every change to a message. Answered once it is on its
/// way; the channel's stream shows it as `MessageReclassified`.
pub async fn reclassify_message(
    State(state): State<Arc<AppState>>,
    Path((id, cid, mid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(classification): Json<Classification>,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor.reclassify_message(MessageId(mid), classification).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Session, "reclassifying a message"))?;
    tracing::Span::current().record("span_id", tracing::field::display(span_id));
    Ok(StatusCode::OK)
}

/// The commands the authenticated user may see in the channel, for
/// completion (issue 28).
pub async fn list_commands(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<Command>>, ApiError> {
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
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
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let reply = user_actor.execute_command(&payload.name, &payload.args).await
        .map_err(|e| ApiError::from_runtime(e, Subject::Command, &format!("running /{}", payload.name)))?;
    // An answer for the whole channel reaches this client on the channel's
    // stream, like everybody else; returning it here as well would show it twice.
    let output = (reply.visibility == CommandVisibility::Private).then_some(reply.output);
    Ok(Json(CommandReplyResponse { command: reply.command, output }))
}


/// What the actor `aid` saw when it produced message `mid` (issues 06, 47):
/// its context with the prompt parts where they were placed. The actor answers: only the agent that
/// produced a message has a context for it.
pub async fn message_context(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid, mid)): Path<(Uuid, Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<MessageContextResponse>, ApiError> {
    let channel = crate::routes::channel_of(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let member = member_of(&channel, aid).await?;
    let view = member.context().await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Context, "reading the context"))?;
    let seen = view.context_of(MessageId(mid))
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Context, "reading the context"))?;
    // What set the message's label, of the parts classified (issue 170).
    let label = view.classification_of(MessageId(mid)).unwrap_or_default();
    let mut classified: Vec<&Classification> = seen.reference.knowledge.iter().filter_map(|k| k.classification.as_ref()).collect();
    for item in &seen.items {
        match item {
            ContextItem::Prompt { classification, .. } => classified.extend(classification),
            ContextItem::Conversation { message } => {
                classified.push(&message.classification);
                classified.extend(message.tool_results.iter().map(|r| &r.classification));
            }
        }
    }
    let causes = LabelCauses::new(label, classified);
    let mut knowledge = Vec::with_capacity(seen.reference.knowledge.len());
    for read in &seen.reference.knowledge {
        knowledge.push(KnowledgeRefResponse::describe(channel.session(), read, &causes).await);
    }
    Ok(Json(MessageContextResponse {
        message_id: MessageId(mid),
        actor_id: member.id(),
        transcript_digest: seen.reference.transcript,
        items: seen.items.into_iter().map(|item| SeenItemResponse::of(item, &causes)).collect(),
        knowledge,
        tools_digest: seen.reference.tools,
        tool_sets: seen.tool_sets,
    }))
}

/// The turns actor `aid` gave up in the channel (issue 145), oldest first,
/// for whoever may read what it saw - each with where it branched off the
/// agent's history and its messages as that history's are (issue 146).
pub async fn given_up_turns(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<GivenUpTurnResponse>>, ApiError> {
    let channel = crate::routes::channel_of(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let view = member_of(&channel, aid).await?.context().await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Context, "reading the turns given up"))?;
    let turns = view.given_up_turns().into_iter()
        .map(|turn| GivenUpTurnResponse {
            reason: turn.given_up.reason,
            at: turn.given_up.at,
            after: turn.after,
            messages: turn.messages.into_iter().map(|message| history_message(&view, message)).collect(),
        })
        .collect();
    Ok(Json(turns))
}

/// `message` of the agent whose context `view` is, as its history shows it.
fn history_message(view: &AgentContextView, message: Message) -> HistoryMessageResponse {
    HistoryMessageResponse { has_context: view.has_context(message.id), in_transcript: view.in_transcript(message.id), message }
}

/// All that was offered to actor `aid` in the channel and what it said
/// itself (issue 207), oldest first, for whoever may read what it saw.
/// Not paged: like the channel's messages, the history is returned whole.
pub async fn agent_history(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<HistoryMessageResponse>>, ApiError> {
    let channel = crate::routes::channel_of(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let view = member_of(&channel, aid).await?.context().await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Context, "reading the history"))?;
    let history = view.history().into_iter().map(|message| history_message(&view, message)).collect();
    Ok(Json(history))
}

/// The tools actor `aid` has now in the channel (issue 194), for whoever
/// may see them (`ReadAgentTools`).
pub async fn agent_tools(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<AgentToolsResponse>, ApiError> {
    let channel = crate::routes::channel_of(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let tool_sets = member_of(&channel, aid).await?.tool_sets().await
        .map_err(|e| ApiError::from_runtime(e.into(), Subject::Agent, "reading the tools"))?;
    Ok(Json(AgentToolsResponse { tool_sets }))
}

pub async fn send_approval(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<SendApprovalRequest>,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = crate::routes::open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let response = ApprovalResponse::new(ApprovalId(payload.approval_id), AgentId(payload.agent_id));
    let span_id = user_actor.send_approval(response, payload.approved).await
        .map_err(|e| ApiError::internal("approval_not_sent", "sending the approval", &e))?;
    tracing::Span::current().record("span_id", tracing::field::display(span_id));
    Ok(StatusCode::OK)
}


#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_runtime::view::{ContextError, ViewError};
    use caelumora_runtime::command::CommandError;
    use caelumora_model::message::MessageId;

    fn history(count: usize) -> Vec<Message> {
        (0..count).map(|i| Message {
            id: MessageId::new(),
            content: Some(format!("message {i}")),
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
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
        let status = |error: CommandError| ApiError::from_runtime(anyhow::Error::new(error), Subject::Command, "running /x").status();
        assert_eq!(status(CommandError::Unknown("x".to_string())), StatusCode::BAD_REQUEST);
        assert_eq!(status(CommandError::Empty), StatusCode::BAD_REQUEST);
        assert_eq!(status(CommandError::InvalidArguments { command: "x".to_string(), reason: "y".to_string() }), StatusCode::BAD_REQUEST);
        assert_eq!(status(CommandError::Forbidden("x".to_string())), StatusCode::FORBIDDEN);
        assert_eq!(status(CommandError::Failed { command: "x".to_string(), reason: "y".to_string() }), StatusCode::INTERNAL_SERVER_ERROR);

        let refused = anyhow::Error::new(caelumora_runtime::channel::ChannelError::Forbidden { action: "waking the agents of", channel: ChannelId::default() });
        assert_eq!(ApiError::from_runtime(refused, Subject::Command, "running /wakeup").status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn a_context_that_cannot_be_shown_says_why() {
        let actor = caelumora_model::actor::ActorId::None;
        let message = MessageId::new();
        let status = |error: anyhow::Error| ApiError::from_runtime(error, Subject::Context, "reading the context");
        assert_eq!(status(ViewError::Forbidden { actor }.into()).status(), StatusCode::FORBIDDEN);
        assert_eq!(status(ViewError::NoContext { actor }.into()).code(), "context_not_recorded");
        assert_eq!(status(ContextError::NotRecorded { actor, message }.into()).code(), "context_not_recorded");
        assert_eq!(status(ContextError::Unavailable { actor, message }.into()).code(), "context_unavailable");
        assert_eq!(status(ViewError::Failed(anyhow::anyhow!("disk")).into()).status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
