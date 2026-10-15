use crate::auth::AuthenticatedUser;
use crate::error::ApiError;
use crate::error::{Json, Path};
use crate::state::AppState;
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response, Sse};
use axum::{extract::{Query, State}, http::StatusCode};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentId;
use caelumora_model::approval::{ApprovalId, ApprovalResponse, ApprovalRule};
use caelumora_model::channel::moderation_components;
use caelumora_model::command::{Command, CommandVisibility};
use caelumora_model::confidentiality::{Classification, LabelCauses};
use caelumora_model::context::{ContextItem, KnowledgeRef, KnowledgeSource};
use caelumora_model::history::HistoryEntry;
use caelumora_model::message::{Message, MessageId};
use caelumora_model::question::QuestionId;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::snapshot::Detail;
use caelumora_rest_api::channel::{AddParticipantRequest, AnswerQuestionsRequest, ChannelMemberResponse, ChannelResponse, ChannelSnapshot, CommandReplyResponse, CreateChannelRequest, ExecuteCommandRequest, MessageRequest, PublishedNotesResponse, SendApprovalRequest, TurnRequest};
use caelumora_rest_api::context::{AgentComponentsResponse, AgentToolsResponse, KnowledgeRefResponse, MessageContextResponse, SeenItemResponse};
use caelumora_rest_api::history::{HistoryItemResponse, HistoryMessageResponse};
use caelumora_rest_api::session::DescriptionRequest;
use caelumora_rest_api::stream::StreamOptions;
use caelumora_rest_api::trash::{DeletedResponse, ListOptions};
use caelumora_runtime::actor::user::UserActor;
use caelumora_runtime::channel::ChannelLoop;
use caelumora_runtime::session::ChannelSpec;
use caelumora_runtime::view::{AgentContextView, ChannelView, MemberView, Reached, SessionView, Viewer};
use futures::Stream;
use std::convert::Infallible;
use std::sync::Arc;
use uuid::Uuid;
use crate::routes::{change_error, channel_of, channel_view, ensure_session, open_channel, views};
use crate::sse::stream;
use tracing::Span;

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
    let session = views(&state, &user).session(session_id).await?;
    if options.deleted {
        let deleted = session.deleted_channels().await?;
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
    let session = ensure_session(&state, &user, session_id).await?;
    session.restore_channel(&user.security_context(), ChannelId(cid)).await?;
    Ok(Json(channel_view(&state, &user, session_id, ChannelId(cid)).await?))
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
    let session = ensure_session(&state, &user, session_id).await?;
    let context = user.security_context();

    let components = moderation_components(payload.moderator.as_deref(), payload.responder_policy.as_deref());
    let spec = ChannelSpec { name: payload.name, components, participants: payload.participants, classification: payload.classification, language: payload.language, uses_workspace: payload.uses_workspace };
    let channel = session.create_channel(&context, spec).await?;
    Ok(Json(channel_view(&state, &user, session_id, channel.id()).await?))
}

/// Stops and deletes a channel of the session, with its history (issue 09).
pub async fn delete_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let session_id = SessionId(id);
    let session = ensure_session(&state, &user, session_id).await?;

    session.delete_channel(&user.security_context(), ChannelId(cid)).await?;
    Ok(StatusCode::NO_CONTENT)
}


pub async fn get_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<ChannelResponse>, ApiError> {
    Ok(Json(channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?))
}

/// How many items of its history a snapshot carries: the end of the
/// conversation, which is what a client needs to show it. Everything older
/// is fetched over REST (`GET .../history`), which `has_more` tells the
/// client to do.
const SNAPSHOT_ITEMS: usize = 20;

/// The history of the channel, as the user may see it (issue 46): its
/// messages and how turns ended (issue 258), oldest first.
pub async fn get_channel_history(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<HistoryItemResponse<Message>>>, ApiError> {
    let channel = channel_of(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let history = channel.history().await?;
    Ok(Json(items(history.items)))
}

/// The items of a history as the API gives them.
fn items(history: Vec<HistoryEntry>) -> Vec<HistoryItemResponse<Message>> {
    history.into_iter().filter_map(|entry| HistoryItemResponse::of(entry, |message| message)).collect()
}

/// The notes every agent of the channel published (issue 17) - what a
/// client reloads on the channel's `channel::AGENT_STATE_CHANGED` (issue
/// 65).
pub async fn get_notes(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<PublishedNotesResponse>>, ApiError> {
    let channel = channel_of(&state, &user, SessionId(id), ChannelId(cid)).await?;
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
    let (recent, has_more) = recent_items(history.items);
    // The conversation can be shown without what of it cannot be read.
    let channel = view.snapshot(Detail::MEMBERS | Detail::USAGE | Detail::NOTES | Detail::APPROVALS | Detail::BRIEFING).await;
    Some(ChannelSnapshot::of(channel, items(recent), has_more))
}

/// The end of a history, and whether it was cut.
fn recent_items(history: Vec<HistoryEntry>) -> (Vec<HistoryEntry>, bool) {
    let has_more = history.len() > SNAPSHOT_ITEMS;
    let start = history.len().saturating_sub(SNAPSHOT_ITEMS);
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
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
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
    Ok(Sse::new(stream(&user, options.snapshot, watch.events, format!("channel {}", cid), snapshot, watch.ended.cancelled_owned()).await))
}

/// Adds a user or agent to the channel (issue 46), with as much of its
/// history as the request grants - one not in the session yet to the
/// session, too (issue 221). Only who may change the session and the
/// channel may do it.
pub async fn add_participant(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<AddParticipantRequest>,
) -> Result<Json<ChannelResponse>, ApiError> {
    // Through the session: one not in it yet is taken into it, too (issue 221).
    let session = ensure_session(&state, &user, SessionId(id)).await?;
    session.add_participant(&user.security_context(), payload.participant, Some((ChannelId(cid), payload.history))).await?;
    Ok(Json(channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?))
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
    let channel = channel_of(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let member = member_of(&channel, aid).await?.snapshot(Detail::USAGE | Detail::PROFILE).await;
    ChannelMemberResponse::of(member).map(Json)
        .ok_or_else(|| ApiError::not_found("actor_not_found", format!("Actor {} is not in channel {}", aid, cid)))
}

pub async fn remove_participant(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let context = user.security_context();
    let participant = actor_in(&channel, aid)
        .ok_or_else(|| ApiError::not_found("participant_not_found", format!("{} takes no part in channel {}", aid, cid)))?;
    channel.remove_participant(&context, participant).await?;
    Ok(StatusCode::NO_CONTENT)
}


pub async fn send_message(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<MessageRequest>,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor.send_message(payload.content, payload.classification).await?;
    Span::current().record("span_id", tracing::field::display(span_id));
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
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor.modify_message(MessageId(mid), payload.content, payload.classification).await?;
    Span::current().record("span_id", tracing::field::display(span_id));
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
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor.request_retry(MessageId(mid)).await?;
    Span::current().record("span_id", tracing::field::display(span_id));
    Ok(StatusCode::OK)
}

/// Asks participant `aid`, an agent, to answer again instead of its answer
/// under way, the turn the body names (issue 235), or anew after its last
/// turn given up (issue 259). Refused at once if the turn is neither
/// (`409 turn_not_retryable`); otherwise the channel ends a turn under way
/// as it delivers the request, and the stream shows `RetryAccepted` and the
/// new answer.
pub async fn retry_turn(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<TurnRequest>,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let participant = actor_in(&channel, aid)
        .ok_or_else(|| ApiError::not_found("participant_not_found", format!("{} takes no part in channel {}", aid, cid)))?;
    let span_id = user_actor.request_retry_of_turn(participant, payload.turn_id).await?;
    Span::current().record("span_id", tracing::field::display(span_id));
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
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor.revoke_message(MessageId(mid)).await?;
    Span::current().record("span_id", tracing::field::display(span_id));
    Ok(StatusCode::NO_CONTENT)
}

/// The actor of `channel` whose id is `id`, whatever kind it is - a path
/// names an actor by its UUID alone.
fn actor_in(channel: &ChannelLoop, id: Uuid) -> Option<ActorId> {
    channel.member_ids().into_iter().find(|actor| actor.id() == id)
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
    Ok(channel.member(ActorId::Agent(AgentId(id))).await?)
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
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    channel.cancel(&user.security_context()).await?;
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
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let participant = actor_in(&channel, aid)
        .ok_or_else(|| ApiError::not_found("participant_not_found", format!("{} takes no part in channel {}", aid, cid)))?;
    channel.cancel_actor(&user.security_context(), participant).await?;
    Ok(StatusCode::OK)
}

/// Interrupts the tool call `call_id` of participant `aid`, an agent, in the
/// turn the body names, alone (issue 276): the call is answered as
/// interrupted and the agent's turn goes on. Under the same permission as
/// cancelling the channel (`CancelChannel`, checked by the channel). A call
/// that does not run, or no longer, is `409 tool_call_not_running`.
pub async fn interrupt_tool_call(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid, call_id)): Path<(Uuid, Uuid, Uuid, String)>,
    user: AuthenticatedUser,
    Json(payload): Json<TurnRequest>,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let participant = actor_in(&channel, aid)
        .ok_or_else(|| ApiError::not_found("participant_not_found", format!("{} takes no part in channel {}", aid, cid)))?;
    channel.interrupt_tool_call(&user.security_context(), participant, payload.turn_id, &call_id).await?;
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
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    channel.reclassify(&user.security_context(), classification).await?;
    Ok(Json(channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?))
}

/// Has the channel's conversation held in the language the body names - a
/// BCP 47 tag such as `"de"` - or in none in particular (`null`, issue
/// 56): whoever may change the channel. Its agents are told from their next
/// turn on. Answers with the channel as the user sees it now.
pub async fn set_channel_language(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(language): Json<Option<String>>,
) -> Result<Json<ChannelResponse>, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    channel.update_language(&user.security_context(), language.as_deref()).await?;
    Ok(Json(channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?))
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
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    channel.update_approval_rules(&user.security_context(), rules).await
        .map_err(|e| change_error(e, "invalid_rule"))?;
    Ok(Json(channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?))
}

/// Says what the channel is for (issue 245): its summary and briefing,
/// `null` or empty to remove one - whoever may change the channel. Its
/// agents are told from their next turn on. Answers `400 invalid_value` for
/// one that does not fit, and otherwise the channel as the user sees it now.
pub async fn update_channel_description(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<DescriptionRequest>,
) -> Result<Json<ChannelResponse>, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    channel.update_description(&user.security_context(), payload.summary.as_deref(), payload.briefing.as_deref()).await?;
    Ok(Json(channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?))
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
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor.reclassify_message(MessageId(mid), classification).await?;
    Span::current().record("span_id", tracing::field::display(span_id));
    Ok(StatusCode::OK)
}

/// The commands the authenticated user may see in the channel, for
/// completion (issue 28).
pub async fn list_commands(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<Command>>, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
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
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let reply = user_actor.execute_command(&payload.name, &payload.args).await?;
    // An answer for the whole channel reaches this client on the channel's
    // stream, like everybody else; returning it here as well would show it twice.
    let output = reply.output.filter(|_| reply.visibility == CommandVisibility::Private);
    Ok(Json(CommandReplyResponse { command: reply.command, output, span_id: reply.span_id }))
}


/// What the actor `aid` saw when it produced message `mid` (issues 06, 47):
/// its context with the prompt parts where they were placed. The actor answers: only the agent that
/// produced a message has a context for it.
pub async fn message_context(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid, mid)): Path<(Uuid, Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<MessageContextResponse>, ApiError> {
    let channel = channel_of(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let member = member_of(&channel, aid).await?;
    let view = member.context().await?;
    let seen = view.context_of(MessageId(mid))?;
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
        knowledge.push(knowledge_response(channel.session(), read, &causes).await);
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

/// `knowledge` with the names of its channel and agent, as far as `session`
/// still has them, and whether it set the label of the message, which
/// `causes` tells.
async fn knowledge_response(session: &SessionView, knowledge: &KnowledgeRef, causes: &LabelCauses) -> KnowledgeRefResponse {
    // Named only if the viewer may read it - of this session or another
    // (issue 134) - and it is still there; by id alone otherwise.
    let reached = session.views().channel_of(knowledge.session, knowledge.channel).await.unwrap_or_else(|e| {
        tracing::warn!("Cannot reach channel {} of the knowledge read: {:#}", knowledge.channel, e);
        Reached::Never
    });
    let channel_deleted = matches!(reached, Reached::Gone(_));
    let channel = reached.there();
    let (source, agent_id, digest, history_start) = match knowledge.source {
        KnowledgeSource::Notes { agent, digest } => ("notes", Some(ActorId::Agent(agent)), digest, None),
        KnowledgeSource::PrivateNotes { agent, digest } => ("private_notes", Some(ActorId::Agent(agent)), digest, None),
        KnowledgeSource::History { digest, start } => ("history", None, digest, start),
        KnowledgeSource::Memory { agent, digest } => ("memory", Some(ActorId::Agent(agent)), digest, None),
        KnowledgeSource::Entity { kind, digest, .. } => (kind.name(), None, digest, None),
    };
    let entity_id = match knowledge.source {
        KnowledgeSource::Entity { id, .. } => Some(id),
        _ => None,
    };
    let agent_name = match (agent_id, &channel) {
        (Some(id), Some(channel)) => channel.member(id).await.ok().and_then(|m| m.name().map(str::to_string)),
        _ => None,
    };
    KnowledgeRefResponse {
        session_id: knowledge.session,
        channel_id: knowledge.channel,
        channel_name: channel.map(|c| c.name().to_string()),
        channel_deleted,
        source,
        entity_id,
        agent_id,
        agent_name,
        digest,
        history_start,
        classification: knowledge.classification.clone(),
        sets_label: causes.set_by(knowledge.classification.as_ref()),
    }
}

/// `message` of the agent whose context `view` is, as its history shows it.
fn history_message(view: &AgentContextView, message: Message) -> HistoryMessageResponse {
    HistoryMessageResponse { has_context: view.has_context(message.id), in_transcript: view.in_transcript(message.id), message }
}

/// All that was offered to actor `aid` in the channel and what it said
/// itself (issue 207), and how its turns ended (issue 258), oldest first,
/// for whoever may read what it saw. Not paged: like the channel's history,
/// it is returned whole.
pub async fn agent_history(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<HistoryItemResponse<HistoryMessageResponse>>>, ApiError> {
    let channel = channel_of(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let view = member_of(&channel, aid).await?.context().await?;
    let history = view.history().into_iter()
        .filter_map(|entry| HistoryItemResponse::of(entry, |message| history_message(&view, message)))
        .collect();
    Ok(Json(history))
}

/// The tools actor `aid` has now in the channel (issue 194), for whoever
/// may see them (`ReadAgentTools`).
pub async fn agent_tools(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<AgentToolsResponse>, ApiError> {
    let channel = channel_of(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let tool_sets = member_of(&channel, aid).await?.tool_sets().await?;
    Ok(Json(AgentToolsResponse { tool_sets }))
}

/// The components actor `aid` was built from in the channel, the
/// preconfigured ones it includes resolved (issue 270), for whoever may see
/// them (`ReadAgentComponents`).
pub async fn agent_components(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<AgentComponentsResponse>, ApiError> {
    let channel = channel_of(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let components = member_of(&channel, aid).await?.components().await?;
    Ok(Json(AgentComponentsResponse { components }))
}

pub async fn send_approval(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<SendApprovalRequest>,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let response = ApprovalResponse::new(ApprovalId(payload.approval_id), AgentId(payload.agent_id));
    let span_id = user_actor.send_approval(response, payload.approved).await
        .map_err(|e| ApiError::internal("approval_not_sent", "sending the approval", &e))?;
    Span::current().record("span_id", tracing::field::display(span_id));
    Ok(StatusCode::OK)
}

/// Answers the questions `qid` an agent's call put to the user (issue 165),
/// or declines them: exactly one of `responses` and `declined: true`.
pub async fn answer_questions(
    State(state): State<Arc<AppState>>,
    Path((id, cid, qid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<AnswerQuestionsRequest>,
) -> Result<StatusCode, ApiError> {
    let responses = match (payload.responses, payload.declined) {
        (Some(responses), false) => Some(responses),
        (None, true) => None,
        _ => return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid_answer", "give either the responses or that the questions are declined")),
    };
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor.answer_questions(QuestionId(qid), responses).await?;
    Span::current().record("span_id", tracing::field::display(span_id));
    Ok(StatusCode::OK)
}


#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::message::MessageId;
    use caelumora_model::security::Refused;
    use caelumora_runtime::channel::ChannelError;
    use caelumora_runtime::command::CommandError;
    use caelumora_runtime::view::{AgentContextError, ViewError};
    use anyhow::Error;

    fn history(count: usize) -> Vec<HistoryEntry> {
        (0..count).map(|i| HistoryEntry::Said { message: Message {
            id: MessageId::new(),
            content: Some(format!("message {i}")),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        }, context: None }).collect()
    }

    /// The contents of the messages of `items`.
    fn contents(items: &[HistoryEntry]) -> Vec<String> {
        items.iter().filter_map(|item| match item {
            HistoryEntry::Said { message, .. } => message.content.clone(),
            _ => None,
        }).collect()
    }

    #[test]
    fn a_snapshot_carries_the_end_of_the_history() {
        let (items, has_more) = recent_items(history(SNAPSHOT_ITEMS + 5));

        assert_eq!(items.len(), SNAPSHOT_ITEMS);
        assert!(has_more, "the client has to fetch the older ones over REST");
        let said = contents(&items);
        assert_eq!(said.first().map(String::as_str), Some("message 5"));
        assert_eq!(said.last().map(String::as_str), Some(&format!("message {}", SNAPSHOT_ITEMS + 4)[..]));
    }

    #[test]
    fn a_short_history_is_complete() {
        let (items, has_more) = recent_items(history(3));
        assert_eq!(items.len(), 3);
        assert!(!has_more);

        let (items, has_more) = recent_items(Vec::new());
        assert!(items.is_empty());
        assert!(!has_more);
    }

    #[test]
    fn a_command_that_does_not_run_is_answered_with_what_the_user_did() {
        let status = |error: CommandError| ApiError::from(error).status();
        assert_eq!(status(CommandError::Unknown("x".to_string())), StatusCode::BAD_REQUEST);
        assert_eq!(status(CommandError::Empty), StatusCode::BAD_REQUEST);
        assert_eq!(status(CommandError::InvalidArguments { command: "x".to_string(), reason: "y".to_string() }), StatusCode::BAD_REQUEST);
        let refused = Refused { subject: "command", action: "ExecuteCommand(/x)".to_string() };
        assert_eq!(status(CommandError::Forbidden { name: "x".to_string(), refused: refused.clone() }), StatusCode::FORBIDDEN);
        assert_eq!(status(CommandError::Failed { command: "x".to_string(), reason: "y".to_string() }), StatusCode::INTERNAL_SERVER_ERROR);

        let refused = Error::new(ChannelError::Forbidden { action: "waking the agents of", channel: ChannelId::default(), refused });
        assert_eq!(ApiError::from(refused).status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn a_context_that_cannot_be_shown_says_why() {
        let actor = ActorId::None;
        let message = MessageId::new();
        let refused = Refused { subject: "context", action: "ReadAgentContext".to_string() };
        assert_eq!(ApiError::from(ViewError::Forbidden { actor, refused }).code(), "context_forbidden");
        assert_eq!(ApiError::from(ViewError::NoContext { actor }).code(), "context_not_recorded");
        assert_eq!(ApiError::from(AgentContextError::NotRecorded { actor, message }).code(), "context_not_recorded");
        assert_eq!(ApiError::from(AgentContextError::Unavailable { actor, message }).code(), "context_unavailable");
        assert_eq!(ApiError::from(ViewError::Failed(anyhow::anyhow!("disk"))).status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
