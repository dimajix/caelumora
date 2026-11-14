//! Channels in the API: making, reading, changing, deleting and restoring them, following them
//! live, and what is done in them - their participants, messages, commands, approvals and
//! questions.

use crate::auth::AuthenticatedUser;
use crate::error::ApiError;
use crate::error::{Json, Path, Query};
use crate::routes::{
    change_error, channel_response, channel_view, ensure_session, open_channel, views,
};
use crate::sse::{bounded_by_token, data_event, named_event};
use crate::state::AppState;
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response, Sse};
use axum::{extract::State, http::StatusCode};
use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::AgentId;
use caelumora_domain::approval::{ApprovalId, ApprovalRule};
use caelumora_domain::channel::{moderation_components, ChannelId};
use caelumora_domain::command::{Command, CommandVisibility};
use caelumora_domain::confidentiality::{Classification, LabelCauses};
use caelumora_domain::context::{ContextItem, KnowledgeRef, KnowledgeSource};
use caelumora_domain::message::{Message, MessageId};
use caelumora_domain::question::QuestionId;
use caelumora_domain::session::SessionId;
use caelumora_domain::snapshot::Detail;
use caelumora_kernel::actor::user::{ChannelWatch, Lagged, UserActor};
use caelumora_kernel::actor::Shown;
use caelumora_kernel::channel::ChannelLoop;
use caelumora_kernel::session::ChannelSpec;
use caelumora_kernel::view::{ChannelView, ParticipantView, Reached, SessionView, Views};
use caelumora_rest_api::channel::{
    AddParticipantRequest, AnswerQuestionsRequest, ChannelParticipantResponse, ChannelResponse,
    ChannelSnapshot, CommandReplyResponse, CreateChannelRequest, ExecuteCommandRequest,
    MessageRequest, PublishedNotesResponse, ReadRequest, SendApprovalRequest, TurnRequest,
};
use caelumora_rest_api::context::{
    AgentComponentsResponse, AgentToolsResponse, KnowledgeRefResponse, MessageContextResponse,
    SeenItemResponse,
};
use caelumora_rest_api::deleted::{DeletedResponse, ListOptions};
use caelumora_rest_api::history::{HistoryMessageResponse, HistoryPageResponse, HistoryQuery};
use caelumora_rest_api::session::DescriptionRequest;
use caelumora_rest_api::stream::StreamOptions;
use futures::stream::{self, Stream, StreamExt};
use std::convert::Infallible;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::field;
use tracing::Span;
use uuid::Uuid;

/// The channels of the session the user may read - or, with
/// `?deleted=true`, those deleted from it they may read as such (issue 164).
pub(super) async fn list_channels(
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
        return Ok(Json(deleted.into_iter().map(DeletedResponse::from).collect::<Vec<_>>())
            .into_response());
    }
    let mut channels: Vec<ChannelResponse> = Vec::new();
    for channel in session.channels().await {
        channels.push(channel.snapshot(Detail::PARTICIPANTS | Detail::UNREAD).await.into());
    }
    Ok(Json(channels).into_response())
}

/// Restores a channel deleted from the session (issue 164), starts it again
/// and answers it as `GET` does. Only who may change the session and
/// restore the channel may.
pub(super) async fn restore_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<ChannelResponse>, ApiError> {
    let session_id = SessionId(id);
    let session = ensure_session(&state, &user, session_id).await?;
    session.restore_channel(&user.security_context(), ChannelId(cid)).await?;
    Ok(Json(channel_response(&state, &user, session_id, ChannelId(cid)).await?))
}

/// Creates a channel in the session and starts it (issue 09). Only who may
/// change the session may do it.
pub(super) async fn create_channel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    user: AuthenticatedUser,
    Json(payload): Json<CreateChannelRequest>,
) -> Result<Json<ChannelResponse>, ApiError> {
    let session_id = SessionId(id);
    let session = ensure_session(&state, &user, session_id).await?;
    let context = user.security_context();

    let mut spec = ChannelSpec {
        components: moderation_components(
            payload.moderator.as_deref(),
            payload.responder_policy.as_deref(),
        ),
        participants: payload.participants,
        language: payload.language,
        internal_actors: payload.internal_actors,
        ..ChannelSpec::new(payload.name)
    };
    if let Some(classification) = payload.classification {
        spec.classification = classification;
    }
    if let Some(uses) = payload.uses_workspace {
        spec.uses_workspace = uses;
    }
    let channel = session.create_channel(&context, spec).await?;
    Ok(Json(channel_response(&state, &user, session_id, channel.id()).await?))
}

/// Stops and deletes a channel of the session, with its history (issue 09).
pub(super) async fn delete_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let session_id = SessionId(id);
    let session = ensure_session(&state, &user, session_id).await?;

    let channel_id = ChannelId(cid);
    match session.delete_channel(&user.security_context(), channel_id).await? {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found(
            "channel_not_found",
            format!("Session {session_id} has no channel {channel_id}"),
        )),
    }
}

pub(super) async fn get_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<ChannelResponse>, ApiError> {
    Ok(Json(channel_response(&state, &user, SessionId(id), ChannelId(cid)).await?))
}

/// How many items of its history a snapshot carries: the end of the
/// conversation, which is what a client needs to show it. Everything older
/// is fetched over REST (`GET .../history`), from the snapshot's `earlier`
/// on.
const SNAPSHOT_ITEMS: usize = 20;

/// A page of the history of the channel, as the user may see it (issues 46, 218): its messages and
/// how turns ended (issue 258), oldest first - the last ones, or those before `?before=`; with
/// `?attachments=true`, the messages carrying attachments alone (issue 509).
pub(super) async fn get_channel_history(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    Query(query): Query<HistoryQuery>,
    user: AuthenticatedUser,
) -> Result<Json<HistoryPageResponse<Message>>, ApiError> {
    let channel = channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let page = channel.history_page_of(query.before, query.limit(), query.filter()).await?;
    Ok(Json(HistoryPageResponse::of(page, |message, _| message)))
}

/// Marks the channel read by the user up to the message the body names
/// (issue 404): the newest message of someone else they were shown. `204`;
/// a message before the one marked changes nothing. Answers `404
/// channel_not_found` for a channel they do not read and
/// `message_not_found` for a message they do not see in it. Kept by the
/// user's controller, without starting the session.
pub(super) async fn mark_read(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<ReadRequest>,
) -> Result<StatusCode, ApiError> {
    let controller = state.kernel.controller_manager().user_controller(user.id).await?;
    controller
        .mark_read(&user.security_context(), SessionId(id), ChannelId(cid), payload.message_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The message `mid` of the channel as it stands (issue 218), if the user
/// sees it in the history - what a client reads back after a change of it
/// was not let in.
pub(super) async fn get_message(
    State(state): State<Arc<AppState>>,
    Path((id, cid, mid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<Message>, ApiError> {
    let channel = channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?;
    Ok(Json(channel.message(MessageId(mid)).await?))
}

/// The notes every agent of the channel published (issue 17) - what a
/// client reloads on the channel's `channel::AGENT_STATE_CHANGED` (issue
/// 65).
pub(super) async fn get_notes(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<PublishedNotesResponse>>, ApiError> {
    let channel = channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let notes = channel.notes().await.map_err(|e| {
        ApiError::internal(
            "notes_failed",
            &format!("reading the notes of channel {}", cid),
            &e.into(),
        )
    })?;
    Ok(Json(notes.into_iter().map(Into::into).collect()))
}

/// Follows a channel live (issues 104, 386): only as one of its
/// participants, through the user's own actor - and across its stops,
/// without starting it: nothing new is said in a channel that does not run,
/// and whoever acts in it starts it. The stream carries what the user's
/// actor shows; where a run of it begins and ends, the stream starts again
/// from the channel's state - with a snapshot, if asked for, or else
/// `started` and `stopped`. It ends when the user is no longer a
/// participant - they left, or the channel or its session was deleted
/// while it ran.
pub(super) async fn join_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    Query(options): Query<StreamOptions>,
    user: AuthenticatedUser,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let (session_id, channel_id) = (SessionId(id), ChannelId(cid));
    let views = views(&state, &user);
    let channel = views.channel(session_id, channel_id).await?;
    let watch = channel.user_follow().await?.ok_or_else(|| not_a_participant(channel_id))?;
    let follower = ChannelFollower {
        views,
        session_id,
        channel_id,
        watch,
        with_snapshot: options.snapshot,
        shutdown: state.shutdown.clone(),
    };
    // Read after the watch began, so that nothing is lost in between; what
    // is in both, clients drop by message id.
    let first = if follower.with_snapshot { follower.snapshot().await } else { None };
    Ok(Sse::new(bounded_by_token(
        &user,
        stream::iter(first.map(Ok)).chain(stream::unfold(follower, ChannelFollower::next)),
    )))
}

/// The stream of `join_channel` after its snapshot (issue 386): what the
/// user's actor shows, each run of it begun and ended with the channel as
/// it then is.
struct ChannelFollower {
    /// The views in the name of the user who follows the channel.
    views: Views,
    session_id: SessionId,
    channel_id: ChannelId,
    watch: ChannelWatch,
    with_snapshot: bool,
    /// Cancelled as the server shuts down (issue 297).
    shutdown: CancellationToken,
}

impl ChannelFollower {
    /// The next event of the stream and the follower after it; `None` once
    /// the user no longer follows the channel, or the server shuts down -
    /// after what the user's actor showed before, the end of its run among
    /// it, so that the stream ends with the channel's state as it stopped
    /// (issue 297).
    async fn next(mut self) -> Option<(Result<Event, Infallible>, Self)> {
        let shown = tokio::select! {
            biased;
            shown = self.watch.next() => shown,
            () = self.shutdown.cancelled() => return None,
        };
        let event = match shown {
            Ok(Shown::Envelope(envelope)) => data_event(&envelope),
            Ok(Shown::Started) => self.anew("started").await?,
            Ok(Shown::Ended) => self.anew("stopped").await?,
            Err(Lagged(lost)) => {
                tracing::warn!(
                    "SSE client of channel {} fell behind and lost {} events",
                    self.channel_id,
                    lost
                );
                let resynced = if self.with_snapshot { self.snapshot().await } else { None };
                resynced
                    .unwrap_or_else(|| Event::default().comment(format!("{} events lost", lost)))
            }
        };
        Some((Ok(event), self))
    }

    /// What begins the stream anew where a run of the user's actor began or
    /// ended: the snapshot if the client asked for snapshots, else `marker`;
    /// `None` if the user no longer takes part in the channel, or cannot
    /// reach it.
    async fn anew(&self, marker: &'static str) -> Option<Event> {
        if !self.views.listed_in(self.session_id, self.channel_id).await {
            return None;
        }
        match self.with_snapshot {
            true => self.snapshot().await,
            false => Some(Event::default().event(marker).data("")),
        }
    }

    /// The channel's snapshot as a named event, as it is now (issue 34); `None` if the channel
    /// cannot be described to the user any more - deleted, or no longer theirs to read - in which
    /// case the stream ends.
    async fn snapshot(&self) -> Option<Event> {
        let view = match self.views.channel(self.session_id, self.channel_id).await {
            Ok(view) => view,
            Err(e) => {
                tracing::warn!(
                    "Cannot describe channel {} for its snapshot: {:#}",
                    self.channel_id,
                    e
                );
                return None;
            }
        };
        let page = match view.history_page(None, SNAPSHOT_ITEMS).await {
            Ok(page) => page,
            Err(e) => {
                tracing::warn!(
                    "Cannot read the history of channel {} for its snapshot: {:#}",
                    self.channel_id,
                    e
                );
                return None;
            }
        };
        // The conversation can be shown without what of it cannot be read.
        let channel = view
            .snapshot(
                Detail::PARTICIPANTS
                    | Detail::USAGE
                    | Detail::NOTES
                    | Detail::APPROVALS
                    | Detail::BRIEFING
                    | Detail::UNREAD,
            )
            .await;
        let history = HistoryPageResponse::of(page, |message, _| message);
        Some(named_event("snapshot", &ChannelSnapshot::of(channel, history)))
    }
}

/// Adds a user or agent to the channel (issue 46), with as much of its
/// history as the request grants - one not in the session yet to the
/// session, too (issue 221). Only who may change the session and the
/// channel may do it.
pub(super) async fn add_participant(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<AddParticipantRequest>,
) -> Result<Json<ChannelResponse>, ApiError> {
    // Through the session: one not in it yet is taken into it, too (issue 221).
    let session = ensure_session(&state, &user, SessionId(id)).await?;
    session
        .add_participant(
            &user.security_context(),
            payload.participant,
            Some((ChannelId(cid), payload.history)),
        )
        .await?;
    Ok(Json(channel_response(&state, &user, SessionId(id), ChannelId(cid)).await?))
}

/// Removes a participant from the channel (issue 46); `aid` is the id of the
/// user or agent. Only who may change the channel may do it.
/// One participant of the channel, with what its model calls used (issue
/// 209) - what a client fetches again when an agent's turn ends, instead
/// of the whole channel.
pub(super) async fn get_participant(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<ChannelParticipantResponse>, ApiError> {
    let channel = channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let participant =
        participant_of(&channel, aid).await?.snapshot(Detail::USAGE | Detail::PROFILE).await;
    ChannelParticipantResponse::of(participant).map(Json).ok_or_else(|| {
        ApiError::not_found("actor_not_found", format!("Actor {} is not in channel {}", aid, cid))
    })
}

pub(super) async fn remove_participant(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let context = user.security_context();
    let participant = actor_in(&channel, aid)?;
    channel.remove_participant(&context, participant).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Takes participant `aid` - the authenticated user, or an agent of theirs - out of the channel by
/// their own act (issue 562), without the right to change it (`ChannelLoop::leave`).
pub(super) async fn leave(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let participant = actor_in(&channel, aid)?;
    channel.leave(&user.security_context(), participant).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn send_message(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<MessageRequest>,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor
        .send_message(payload.content, payload.classification, payload.attachments)
        .await?;
    Span::current().record("span_id", field::display(span_id));
    Ok(StatusCode::OK)
}

/// Modifies the authenticated user's message `mid` (issue 234): it reads as `content` from now on,
/// in its place, with the attachments named (issue 282). Answered once it is on its way: whether it
/// can be changed the channel decides as it admits it, and a refusal comes on the stream
/// (`EventRejected`, issue 567); otherwise the stream shows it as `MessageModified`. A message
/// not there, another's, or an attachment that is neither the message's as it stands nor an upload
/// of the user's for the channel fails at once.
pub(super) async fn modify_message(
    State(state): State<Arc<AppState>>,
    Path((id, cid, mid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<MessageRequest>,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor
        .modify_message(
            MessageId(mid),
            payload.content,
            payload.classification,
            payload.attachments,
        )
        .await?;
    Span::current().record("span_id", field::display(span_id));
    Ok(StatusCode::OK)
}

/// Asks the agent that said message `mid` to answer again instead of it (issue 234). Answered once
/// the request is on its way: whether it may be asked the channel decides as it admits it, and a
/// refusal comes on the stream (`EventRejected`, issue 567); whether the agent does is its own to
/// decide, and the stream shows its answer as `RetryAccepted` - followed by the message taken back
/// and the new answer - or `RetryRejected` with the reason.
pub(super) async fn retry_message(
    State(state): State<Arc<AppState>>,
    Path((id, cid, mid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor.request_retry(MessageId(mid)).await?;
    Span::current().record("span_id", field::display(span_id));
    Ok(StatusCode::OK)
}

/// Asks participant `aid`, an agent, to answer again instead of its answer under way, the turn the
/// body names (issue 235), or anew after its last turn given up (issue 259). Answered once the
/// request is on its way: whether the turn is either the channel decides as it admits it, and a
/// refusal comes on the stream (`EventRejected`); otherwise the channel ends a turn under way as it
/// delivers the request, and the stream shows `RetryAccepted` and the new answer.
pub(super) async fn retry_turn(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<TurnRequest>,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let participant = actor_in(&channel, aid)?;
    let span_id = user_actor.request_retry_of_turn(participant, payload.turn_id).await?;
    Span::current().record("span_id", field::display(span_id));
    Ok(StatusCode::OK)
}

/// Takes the authenticated user's message `mid` back (issue 234): it stays in its place without
/// what it said, and the agents are no longer given it - not a deletion. Answered once it is on its
/// way: whether it can be taken back the channel decides as it admits it, and a refusal comes on
/// the stream (`EventRejected`, issue 567); otherwise the stream shows it as `MessageRevoked`.
pub(super) async fn revoke_message(
    State(state): State<Arc<AppState>>,
    Path((id, cid, mid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor.revoke_message(MessageId(mid)).await?;
    Span::current().record("span_id", field::display(span_id));
    Ok(StatusCode::NO_CONTENT)
}

/// The actor of `channel` whose id is `id`, whatever kind it is - a path
/// names an actor by its UUID alone.
///
/// # Errors
/// `participant_not_found` if none of its participants has that id.
fn actor_in(channel: &ChannelLoop, id: Uuid) -> Result<ActorId, ApiError> {
    channel.participant_ids().into_iter().find(|actor| actor.id() == id).ok_or_else(|| {
        ApiError::not_found(
            "participant_not_found",
            format!("{} takes no part in channel {}", id, channel.id()),
        )
    })
}

/// The participant of `channel` whose id is `id`, whatever kind it is - or a
/// former participant (issue 164), whose history and context stay readable.
async fn participant_of(channel: &ChannelView, id: Uuid) -> Result<ParticipantView, ApiError> {
    let mut participants = channel.participants().await;
    participants.extend(channel.former_participants().await);
    if let Some(participant) =
        participants.into_iter().find(|participant| participant.id().id() == id)
    {
        return Ok(participant);
    }
    // An agent that left before the channel kept who left is known by what
    // it said.
    Ok(channel.participant(ActorId::Agent(AgentId(id))).await?)
}

/// The actor the authenticated user acts through in `channel` (issue 30).
///
/// Whoever writes or approves does so in their own name, so someone who is
/// not a participant of the channel has no actor here and is refused.
fn user_actor_of(
    channel: &ChannelLoop,
    user: &AuthenticatedUser,
) -> Result<Arc<UserActor>, ApiError> {
    channel.user_actor(user.id).ok_or_else(|| not_a_participant(channel.id()))
}

/// What a user is answered who acts as a participant of channel `id`
/// without being one.
fn not_a_participant(id: ChannelId) -> ApiError {
    ApiError::forbidden("not_a_participant", format!("You are not a participant of channel {}", id))
}

/// Ends the turn of every agent in the channel - an operation on the
/// channel, like renaming it, not an act of taking part: whoever the policy
/// lets cancel it may (`CancelChannel`, checked by the channel).
pub(super) async fn cancel_channel(
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
pub(super) async fn cancel_participant(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let participant = actor_in(&channel, aid)?;
    channel.cancel_actor(&user.security_context(), participant).await?;
    Ok(StatusCode::OK)
}

/// Interrupts the tool call `call_id` of participant `aid`, an agent, in the
/// turn the body names, alone (issue 276): the call is answered as
/// interrupted and the agent's turn goes on. Under the same permission as
/// cancelling the channel (`CancelChannel`, checked by the channel). A call
/// that does not run, or no longer, is `409 tool_call_not_running`.
pub(super) async fn interrupt_tool_call(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid, call_id)): Path<(Uuid, Uuid, Uuid, String)>,
    user: AuthenticatedUser,
    Json(payload): Json<TurnRequest>,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let participant = actor_in(&channel, aid)?;
    channel
        .interrupt_tool_call(&user.security_context(), participant, payload.turn_id, &call_id)
        .await?;
    Ok(StatusCode::OK)
}

/// Reclassifies the channel (issue 133): only its owner. Lowering it
/// releases what is in it. Answers with the channel as the user sees it now.
pub(super) async fn reclassify_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(classification): Json<Classification>,
) -> Result<Json<ChannelResponse>, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    channel.reclassify(&user.security_context(), classification).await?;
    Ok(Json(channel_response(&state, &user, SessionId(id), ChannelId(cid)).await?))
}

/// Has the channel's conversation held in the language the body names - a
/// BCP 47 tag such as `"de"` - or in none in particular (`null`, issue
/// 56): whoever may change the channel. Its agents are told from their next
/// turn on. Answers with the channel as the user sees it now.
pub(super) async fn set_channel_language(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(language): Json<Option<String>>,
) -> Result<Json<ChannelResponse>, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    channel.update_language(&user.security_context(), language.as_deref()).await?;
    Ok(Json(channel_response(&state, &user, SessionId(id), ChannelId(cid)).await?))
}

/// Sets what the channel restricts of the tool calls made in it (issue 13):
/// rules that ask or deny, replacing those it had - whoever may change the
/// channel. Answers `400 invalid_rule` for a rule that would allow, is
/// narrower than the channel or cannot be matched, and otherwise the
/// channel as the user sees it now.
pub(super) async fn update_channel_approval_rules(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(rules): Json<Vec<ApprovalRule>>,
) -> Result<Json<ChannelResponse>, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    channel
        .update_approval_rules(&user.security_context(), rules)
        .await
        .map_err(|e| change_error(e, "invalid_rule"))?;
    Ok(Json(channel_response(&state, &user, SessionId(id), ChannelId(cid)).await?))
}

/// Says what the channel is for (issue 245): its summary and briefing,
/// `null` or empty to remove one - whoever may change the channel. Its
/// agents are told from their next turn on. Answers `400 invalid_value` for
/// one that does not fit, and otherwise the channel as the user sees it now.
pub(super) async fn update_channel_description(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<DescriptionRequest>,
) -> Result<Json<ChannelResponse>, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    channel
        .update_description(
            &user.security_context(),
            payload.summary.as_deref(),
            payload.briefing.as_deref(),
        )
        .await?;
    Ok(Json(channel_response(&state, &user, SessionId(id), ChannelId(cid)).await?))
}

/// Reclassifies a message of the channel (issues 133, 239): its author or the channel's owner,
/// never above the channel - as a participant, through their actor, as every change to a message.
/// Answered once it is on its way: whether it may be the channel decides as it admits it, and a
/// refusal comes on the stream (`EventRejected`, issue 567); otherwise the stream shows it as
/// `MessageReclassified`.
pub(super) async fn reclassify_message(
    State(state): State<Arc<AppState>>,
    Path((id, cid, mid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(classification): Json<Classification>,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor.reclassify_message(MessageId(mid), classification).await?;
    Span::current().record("span_id", field::display(span_id));
    Ok(StatusCode::OK)
}

/// The commands the authenticated user may see in the channel, for
/// completion (issue 28).
pub(super) async fn list_commands(
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
pub(super) async fn execute_command(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<ExecuteCommandRequest>,
) -> Result<Json<CommandReplyResponse>, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let reply = user_actor.execute_command(&payload.line).await?;
    // An answer for the whole channel reaches this client on the channel's
    // stream, like everybody else; returning it here as well would show it twice.
    let output = reply.output.filter(|_| reply.visibility == CommandVisibility::Private);
    Ok(Json(CommandReplyResponse { command: reply.command, output, span_id: reply.span_id }))
}

/// What the actor `aid` saw when it produced message `mid` (issues 06, 47):
/// its context with the prompt parts where they were placed. The actor answers: only the agent that
/// produced a message has a context for it.
pub(super) async fn message_context(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid, mid)): Path<(Uuid, Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<MessageContextResponse>, ApiError> {
    let channel = channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let participant = participant_of(&channel, aid).await?;
    let view = participant.context().await?;
    let seen = view.context_of(MessageId(mid)).await?;
    // What set the message's label, of the parts classified (issue 170).
    let label = view.classification_of(MessageId(mid)).await?.unwrap_or_default();
    let mut classified: Vec<&Classification> =
        seen.reference.frame.knowledge.iter().filter_map(|k| k.classification.as_ref()).collect();
    for item in &seen.items {
        match item {
            ContextItem::Prompt { classification, .. } => classified.extend(classification),
            ContextItem::Conversation { message } => {
                classified.extend(seen.reference.frame.profile.classifications_given(message))
            }
        }
    }
    let causes = LabelCauses::new(label, classified);
    let mut knowledge = Vec::with_capacity(seen.reference.frame.knowledge.len());
    for read in &seen.reference.frame.knowledge {
        knowledge.push(knowledge_response(channel.session(), read, &causes).await);
    }
    Ok(Json(MessageContextResponse {
        message_id: MessageId(mid),
        actor_id: participant.id(),
        transcript_digest: seen.reference.transcript,
        items: seen
            .items
            .into_iter()
            .map(|item| SeenItemResponse::of(item, &causes, &seen.reference.frame.profile))
            .collect(),
        knowledge,
        tools_digest: seen.reference.frame.tools,
        tool_sets: seen.tool_sets,
    }))
}

/// `knowledge` with the names of its channel and agent, as far as `session`
/// still has them, and whether it set the label of the message, which
/// `causes` tells.
async fn knowledge_response(
    session: &SessionView,
    knowledge: &KnowledgeRef,
    causes: &LabelCauses,
) -> KnowledgeRefResponse {
    // Named only if the viewer may read it - of this session or another
    // (issue 134) - and it is still there; by id alone otherwise.
    let reached =
        session.views().channel_of(knowledge.session, knowledge.channel).await.unwrap_or_else(
            |e| {
                tracing::warn!(
                    "Cannot reach channel {} of the knowledge read: {:#}",
                    knowledge.channel,
                    e
                );
                Reached::Never
            },
        );
    let channel_deleted = matches!(reached, Reached::Gone(_));
    let channel = reached.there();
    let (source, agent_id, digest, history_start) = match knowledge.source {
        KnowledgeSource::Notes { agent, digest } => {
            ("notes", Some(ActorId::Agent(agent)), digest, None)
        }
        KnowledgeSource::PrivateNotes { agent, digest } => {
            ("private_notes", Some(ActorId::Agent(agent)), digest, None)
        }
        KnowledgeSource::History { digest, from } => ("history", None, digest, from),
        KnowledgeSource::Entity { kind, digest, .. } => (kind.name(), None, digest, None),
    };
    let entity_id = match knowledge.source {
        KnowledgeSource::Entity { id, .. } => Some(id),
        _ => None,
    };
    let agent_name = match (agent_id, &channel) {
        (Some(id), Some(channel)) => {
            channel.participant(id).await.ok().and_then(|m| m.name().map(str::to_string))
        }
        _ => None,
    };
    KnowledgeRefResponse {
        session_id: knowledge.session,
        channel_id: knowledge.channel,
        channel_name: channel.map(|c| c.name().to_string()),
        channel_deleted,
        source,
        entity_id,
        place: knowledge.source.place().map(str::to_string),
        agent_id,
        agent_name,
        digest,
        history_start,
        classification: knowledge.classification.clone(),
        sets_label: causes.set_by(knowledge.classification.as_ref()),
    }
}

/// A page of all that was offered to actor `aid` in the channel and what it said itself (issues
/// 207, 218), and how its turns ended (issue 258), oldest first - the last ones, or those before
/// `?before=` -, for whoever may read what it saw; with `?attachments=true`, the messages carrying
/// attachments alone (issue 509).
pub(super) async fn agent_history(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    Query(query): Query<HistoryQuery>,
    user: AuthenticatedUser,
) -> Result<Json<HistoryPageResponse<HistoryMessageResponse>>, ApiError> {
    let channel = channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let view = participant_of(&channel, aid).await?.context().await?;
    let page = view.history_page_of(query.before, query.limit(), query.filter()).await?;
    let heard: Vec<MessageId> =
        page.entries().filter_map(|item| item.said().map(|m| m.id)).collect();
    let passed_on = view.in_transcript(&heard).await?;
    Ok(Json(HistoryPageResponse::of(page, |message, has_context| HistoryMessageResponse {
        has_context,
        in_transcript: passed_on.contains(&message.id),
        message,
    })))
}

/// The tools actor `aid` has now in the channel (issue 194), for whoever
/// may see them (`ReadAgentTools`).
pub(super) async fn agent_tools(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<AgentToolsResponse>, ApiError> {
    let channel = channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let tool_sets = participant_of(&channel, aid).await?.tool_sets().await?;
    Ok(Json(AgentToolsResponse { tool_sets }))
}

/// The components actor `aid` was built from in the channel, the bundles it includes resolved
/// (issue 270), for whoever may see them (`ReadAgentComponents`).
pub(super) async fn agent_components(
    State(state): State<Arc<AppState>>,
    Path((id, cid, aid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Json<AgentComponentsResponse>, ApiError> {
    let channel = channel_view(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let components = participant_of(&channel, aid).await?.components().await?;
    Ok(Json(AgentComponentsResponse { components }))
}

/// Answers the approval request `approval_id` as the user (`SendApprovalRequest`), naming the
/// request alone: which agent asked, and whether the user may approve its calls, the approver
/// tells, which holds the request, and an answer it does not count it refuses on the user's stream
/// (`EventRefused`, issue 567). Answered `200` once the answer is on its way.
pub(super) async fn send_approval(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<SendApprovalRequest>,
) -> Result<StatusCode, ApiError> {
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor
        .send_approval(ApprovalId(payload.approval_id), payload.approved)
        .await
        .map_err(|e| ApiError::internal("approval_not_sent", "sending the approval", &e))?;
    Span::current().record("span_id", field::display(span_id));
    Ok(StatusCode::OK)
}

/// Answers the questions `qid` an agent's call put to the user (issue 165),
/// or declines them: exactly one of `responses` and `declined: true`.
pub(super) async fn answer_questions(
    State(state): State<Arc<AppState>>,
    Path((id, cid, qid)): Path<(Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
    Json(payload): Json<AnswerQuestionsRequest>,
) -> Result<StatusCode, ApiError> {
    let responses = match (payload.responses, payload.declined) {
        (Some(responses), false) => Some(responses),
        (None, true) => None,
        _ => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_answer",
                "give either the responses or that the questions are declined",
            ))
        }
    };
    let (_, channel) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let user_actor = user_actor_of(&channel, &user)?;
    let span_id = user_actor.answer_questions(QuestionId(qid), responses).await?;
    Span::current().record("span_id", field::display(span_id));
    Ok(StatusCode::OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::api;
    use crate::routes::testing::{login, read_until, send, session_of, state, user_id};
    use anyhow::Error;
    use axum::body::{to_bytes, Body, BodyDataStream};
    use axum::http::header::{CONTENT_TYPE, COOKIE};
    use axum::http::{Method, Request};
    use axum::Router;
    use caelumora_api::command::CommandError;
    use caelumora_api::reader::ViewError;
    use caelumora_domain::message::MessageId;
    use caelumora_domain::security::Refused;
    use caelumora_kernel::channel::ChannelError;
    use caelumora_kernel::view::{AgentContextError, Viewer};
    use caelumora_security::SecurityContext;
    use serde_json::{json, Value};
    use std::time::Duration;
    use tokio::time;
    use tower::ServiceExt;

    #[test]
    fn a_command_that_does_not_run_is_answered_with_what_the_user_did() {
        let status = |error: CommandError| ApiError::from(error).status();
        assert_eq!(status(CommandError::Unknown("x".to_string())), StatusCode::BAD_REQUEST);
        assert_eq!(status(CommandError::Empty), StatusCode::BAD_REQUEST);
        assert_eq!(
            status(CommandError::InvalidArguments {
                command: "x".to_string(),
                reason: "y".to_string()
            }),
            StatusCode::BAD_REQUEST
        );
        let refused = Refused { subject: "command", action: "ExecuteCommand(/x)".to_string() };
        assert_eq!(
            status(CommandError::Forbidden { name: "x".to_string(), refused: refused.clone() }),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(CommandError::Failed { command: "x".to_string(), reason: "y".to_string() }),
            StatusCode::INTERNAL_SERVER_ERROR
        );

        let refused = Error::new(ChannelError::Forbidden {
            action: "waking the agents of",
            channel: ChannelId::default(),
            refused,
        });
        assert_eq!(ApiError::from(refused).status(), StatusCode::FORBIDDEN);
    }

    /// The API on `state`, the cookie of alice, and a session of hers with
    /// her in its main channel, started and stopped again.
    async fn stopped_session_of_alice(
        state: &Arc<AppState>,
    ) -> (Router, String, SessionId, ChannelId) {
        let api = api(Arc::clone(state), &[]);
        let cookie = login(&api, "alice").await;
        let alice = user_id(&api, &cookie).await;
        let (session_id, channel_id) = session_of(state, alice).await;
        state.session_manager.stop(&SecurityContext::system(), session_id).await.unwrap();
        (api, cookie, session_id, channel_id)
    }

    /// The stream of the channel, as whoever `cookie` logs in follows it.
    async fn follow(
        api: &Router,
        cookie: &str,
        session: SessionId,
        channel: ChannelId,
        snapshot: bool,
    ) -> BodyDataStream {
        let uri =
            format!("/sessions/{}/channels/{}/events?snapshot={snapshot}", session.0, channel.0);
        let response = api
            .clone()
            .oneshot(Request::get(uri).header(COOKIE, cookie).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.into_body().into_data_stream()
    }

    /// Whether channel `channel` of `session` runs.
    async fn runs(state: &AppState, session: SessionId, channel: ChannelId) -> bool {
        let views = state.session_manager.views(Viewer::new(SecurityContext::system()));
        views.channel(session, channel).await.unwrap().running().is_some()
    }

    /// The page of a history `uri` names, as whoever `cookie` logs in reads it.
    async fn page_of(api: &Router, cookie: &str, uri: &str) -> Value {
        let (status, page) = send(api, cookie, Method::GET, uri, None).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        page
    }

    /// Has whoever `cookie` logs in say `text` in the channel, and waits until its history holds
    /// it - kept, before a stop would lose it on its way (issue 385).
    async fn say(api: &Router, cookie: &str, session: SessionId, channel: ChannelId, text: &str) {
        let channel_uri = format!("/sessions/{}/channels/{}", session.0, channel.0);
        let messages = format!("{channel_uri}/messages");
        let (status, _) =
            send(api, cookie, Method::POST, &messages, Some(json!({ "content": text }))).await;
        assert_eq!(status, StatusCode::OK);
        let history = format!("{channel_uri}/history");
        time::timeout(Duration::from_secs(5), async {
            while !page_of(api, cookie, &history).await.to_string().contains(text) {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("in the history");
    }

    /// Issue 386: following a channel that does not run does not start it;
    /// the stream waits for whoever acts in it - and shows what they said,
    /// after the channel's state -, follows it while it runs, starts from
    /// its state again when it stops, and ends once it is deleted.
    #[tokio::test]
    async fn a_stopped_channel_is_followed_without_being_started() {
        let state = state();
        let (api, cookie, session, channel) = stopped_session_of_alice(&state).await;
        let snapshots = |read: &str| read.matches("event: snapshot").count();

        let mut stream = follow(&api, &cookie, session, channel, true).await;
        let mut read = String::new();
        read_until(&mut stream, &mut read, |read| snapshots(read) == 1).await;
        assert!(!runs(&state, session, channel).await, "following starts nothing");

        say(&api, &cookie, session, channel, "hello there").await;
        let after = |read: &str, nth: usize| {
            read.split("event: snapshot").nth(nth).unwrap_or_default().to_string()
        };
        read_until(&mut stream, &mut read, |read| after(read, 2).contains("hello there")).await;
        assert!(
            runs(&state, session, channel).await,
            "whoever acts starts it, and is shown what they said after the state it started from"
        );

        // Seen at the stop, which only a stream following the actor sees.
        state.session_manager.stop(&SecurityContext::system(), session).await.unwrap();
        read_until(&mut stream, &mut read, |read| snapshots(read) == 3).await;
        assert!(
            read.rsplit("event: snapshot").next().is_some_and(|last| last.contains("hello there")),
            "{read}"
        );
        assert!(!runs(&state, session, channel).await, "the stream does not start it again");

        state.session_manager.open(&SecurityContext::system(), session).await.unwrap();
        read_until(&mut stream, &mut read, |read| snapshots(read) == 4).await;
        assert!(state.session_manager.delete(&SecurityContext::system(), session).await.unwrap());
        let ended =
            time::timeout(Duration::from_secs(5), async { while stream.next().await.is_some() {} })
                .await;
        assert!(ended.is_ok(), "the stream of a deleted channel ends: {read}");
    }

    /// Issue 297: as the server shuts down, a channel's stream ends - after
    /// the channel's state as it stopped, which the client is left with.
    #[tokio::test]
    async fn a_stream_ends_at_a_shutdown_with_the_channels_state_as_it_stopped() {
        let state = state();
        let (api, cookie, session, channel) = stopped_session_of_alice(&state).await;
        state.session_manager.open(&SecurityContext::system(), session).await.unwrap();
        let mut stream = follow(&api, &cookie, session, channel, true).await;
        let mut read = String::new();
        read_until(&mut stream, &mut read, |read| read.contains("event: snapshot")).await;

        state.session_manager.shutdown().await;
        state.shutdown.cancel();
        let ended = time::timeout(Duration::from_secs(5), async {
            while let Some(chunk) = stream.next().await {
                read.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
            }
        })
        .await;
        assert!(ended.is_ok(), "the stream ends: {read}");
        assert_eq!(
            read.matches("event: snapshot").count(),
            2,
            "with the state as the channel stopped: {read}"
        );
    }

    /// Issue 386: a client without snapshots is told that the channel
    /// stopped and started.
    #[tokio::test]
    async fn a_stream_without_snapshots_says_when_the_channel_stops_and_starts() {
        let state = state();
        let (api, cookie, session, channel) = stopped_session_of_alice(&state).await;
        let mut stream = follow(&api, &cookie, session, channel, false).await;
        let mut read = String::new();
        read_until(&mut stream, &mut read, |read| read.contains("retry:")).await;

        state.session_manager.open(&SecurityContext::system(), session).await.unwrap();
        read_until(&mut stream, &mut read, |read| read.contains("event: started")).await;
        state.session_manager.stop(&SecurityContext::system(), session).await.unwrap();
        read_until(&mut stream, &mut read, |read| read.contains("event: stopped")).await;
        assert!(!read.contains("event: snapshot"), "{read}");
    }

    /// Issue 384: a malformed query string is answered as problem details,
    /// like a malformed body or path.
    #[tokio::test]
    async fn a_malformed_query_is_answered_as_problem_details() {
        let state = state();
        let (api, cookie, session, channel) = stopped_session_of_alice(&state).await;
        let uri = format!("/sessions/{}/channels/{}/history?limit=abc", session.0, channel.0);
        let response = api
            .oneshot(Request::get(uri).header(COOKIE, &cookie).body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[CONTENT_TYPE], "application/problem+json");
        let json: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(json["code"], "invalid_query");
        assert!(
            json["detail"].as_str().unwrap().contains("limit"),
            "the detail says what is wrong: {}",
            json["detail"]
        );
    }

    /// Issue 509: `?attachments=true` reads a page of the messages carrying attachments alone -
    /// none of a message of words.
    #[tokio::test]
    async fn a_page_of_the_messages_carrying_attachments_is_asked_for_by_the_query() {
        let state = state();
        let (api, cookie, session, channel) = stopped_session_of_alice(&state).await;
        say(&api, &cookie, session, channel, "hello there").await;

        let history = format!("/sessions/{}/channels/{}/history", session.0, channel.0);
        let carrying = page_of(&api, &cookie, &format!("{history}?attachments=true")).await;
        assert!(
            !carrying["items"].as_array().unwrap().iter().any(|item| item["said"].is_object()),
            "words alone carry nothing: {carrying}"
        );
        state.session_manager.stop(&SecurityContext::system(), session).await.unwrap();
    }

    /// Who takes no part in a channel cannot follow it, whether it runs or
    /// not.
    #[tokio::test]
    async fn only_a_participant_follows_a_channel() {
        let state = state();
        let (api, _, session, channel) = stopped_session_of_alice(&state).await;
        let bob = login(&api, "bob").await;
        let uri = format!("/sessions/{}/channels/{}/events", session.0, channel.0);
        for _ in [false, true] {
            let (status, _) = send(&api, &bob, Method::GET, &uri, None).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            state.session_manager.open(&SecurityContext::system(), session).await.unwrap();
        }
    }

    #[test]
    fn a_context_that_cannot_be_shown_says_why() {
        let actor = ActorId::None;
        let message = MessageId::new();
        let refused = Refused { subject: "context", action: "ReadAgentContext".to_string() };
        assert_eq!(
            ApiError::from(ViewError::Forbidden { actor, refused }).code(),
            "context_forbidden"
        );
        assert_eq!(ApiError::from(ViewError::NoContext { actor }).code(), "context_not_recorded");
        assert_eq!(
            ApiError::from(AgentContextError::NotRecorded { actor, message }).code(),
            "context_not_recorded"
        );
        assert_eq!(
            ApiError::from(AgentContextError::Unavailable { actor, message }).code(),
            "context_unavailable"
        );
        assert_eq!(
            ApiError::from(ViewError::Failed(anyhow::anyhow!("disk"))).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
