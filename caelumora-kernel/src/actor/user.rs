//! `UserActor`: a user's actor in a channel, through which what they say, run and answer goes,
//! and through whose outbox a frontend follows the channel for them (`ChannelWatch`, issue 386).

use crate::actor::ChannelAnswering;
use crate::actor::Shown;
use crate::channel::{ChannelError, ChannelLoop};
use crate::command::{first_word, CommandContext, CommandReply};
use crate::controller::{ChannelController, UserController};
use crate::kernel::ParticipantRuntime;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use caelumora_api::actor::{Actor, Answering};
use caelumora_api::command::CommandError;
use caelumora_api::transport::{EventReceiver, EventSender, TransportError};
use caelumora_domain::actor::{
    ActorDescription, ActorEvent, ActorId, ActorState, Envelope, RetryTarget, SpanId,
};
use caelumora_domain::agent::AgentId;
use caelumora_domain::approval::{
    ApprovalDecision, ApprovalId, ApprovalRequest, ApprovalRule, ApprovalRuleId, RuleDecision,
};
use caelumora_domain::channel::{ChannelId, ThreadId};
use caelumora_domain::command::{addressed, CommandDescription, CommandVisibility};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::content::ContentId;
use caelumora_domain::mention::{handle_of, is_group_handle};
use caelumora_domain::message::{Message, MessageId, MessageRole};
use caelumora_domain::question::{QuestionError, QuestionId, QuestionRequest, Response};
use caelumora_domain::session::SessionId;
use caelumora_domain::snapshot::Waiting;
use caelumora_domain::tool::{ToolCall, ToolCallResult};
use caelumora_domain::value::name::Name;
use caelumora_domain::work::TurnId;
use caelumora_security::{AccessPolicy, AccessQuestions, ActionType, SecurityContext};
use chrono::Utc;
use std::collections::HashMap;
use std::future;
use std::sync;
use std::sync::{Arc, PoisonError};
use tokio::sync::broadcast::error::RecvError;
#[cfg(any(test, feature = "test-support"))]
use tokio::sync::broadcast::error::TryRecvError;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{broadcast, mpsc, Mutex, MutexGuard};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

#[derive(Debug)]
pub struct UserActor {
    id: ActorId,
    session_id: SessionId,
    channel_id: ChannelId,
    user: Arc<UserController>,
    participant_runtime: ParticipantRuntime,
    /// What the user's rules answer with (issue 13): whether they may
    /// approve the calls of an agent in this channel is asked first.
    access_policy: Arc<dyn AccessPolicy>,
    channel: Arc<ChannelController>,
    // Envelopes built by `send_message` / `send_approval`, forwarded into the
    // channel by `run`.
    user_rx: Mutex<Receiver<Envelope>>,
    user_tx: Sender<Envelope>,
    /// Where what the actor shows its user goes, to everyone who follows
    /// it (issues 220, 386): what it is routed and says, between the start
    /// and the end of each run. Its user's outbox in the channel, which
    /// outlives this actor (`ActorManager`).
    outbox: broadcast::Sender<Shown>,
    // Cancelled when the actor is to end - it left the channel, or the
    // channel stopped: what ends `run`.
    ended: CancellationToken,
    /// Where its run stands, so that its start and end are shown once each
    /// (issue 386).
    run: sync::Mutex<Run>,
    /// The questions shown to this user that are open (issue 165), as the agents asked them and
    /// until they close them, in the order they were first asked: those put to the user are what
    /// they may answer, and what a message of theirs declines. Those the user answered or declined
    /// are forgotten as the answer is sent, so that they are not offered again until they close,
    /// and come back only where an agent asks them anew (issue 580). Each with the thread it was
    /// asked in (issue 602), where it is answered: its context is that thread's.
    open_questions: sync::Mutex<Vec<(QuestionRequest, ThreadId)>>,
    /// The approvals the approver asked of the users (issue 594), until they end - decided,
    /// withdrawn -, each with the thread it was asked in and whether this user answered it
    /// (`OpenApproval`). Asked anew - by an agent that resumed its turn, or of the user as they
    /// joined (issue 603) -, one is kept once.
    open_approvals: sync::Mutex<HashMap<ApprovalId, OpenApproval>>,
}

/// An approval asked of the users that is open (issues 594, 603), as a user's actor keeps it: the
/// thread it was asked in, where the user answers it, and whether they did - by their hand or their
/// rule -, after which it waits for them no more and is not answered by their rule again.
#[derive(Debug, Clone, Copy)]
struct OpenApproval {
    thread: ThreadId,
    answered: bool,
}

/// The channel as a user follows it (issues 104, 220, 386): what their
/// actor in it shows - what it is routed and says for them, and where its
/// runs begin and end -, across its runs: also while the channel does not
/// run, so that whoever follows it sees it run again. Only a participant
/// follows a channel live, and only through their actor's outbox. Any number
/// may follow - two tabs, two devices (issue 220); without anyone following,
/// what is shown is dropped.
pub struct ChannelWatch {
    shown: broadcast::Receiver<Shown>,
    /// Keeps the outbox while no actor runs, so that it never closes: held, never read.
    _outbox: broadcast::Sender<Shown>,
}

impl ChannelWatch {
    /// Follows `outbox` from now on.
    pub(crate) fn of(outbox: &broadcast::Sender<Shown>) -> Self {
        Self { shown: outbox.subscribe(), _outbox: outbox.clone() }
    }

    /// What is shown next. One that falls `OUTBOX_CAPACITY` items behind
    /// loses the oldest (`Lagged`) rather than stalling the actor, which
    /// would also stop it from forwarding what the user sends.
    ///
    /// # Errors
    /// `Lagged` with how many were lost.
    pub async fn next(&mut self) -> Result<Shown, Lagged> {
        match self.shown.recv().await {
            Ok(shown) => Ok(shown),
            Err(RecvError::Lagged(lost)) => Err(Lagged(lost)),
            // Held by this watch, the outbox is never closed.
            Err(RecvError::Closed) => future::pending().await,
        }
    }
}

/// What the tests read of a watch past its own API: the envelopes alone, past where runs begin
/// and end, which whoever follows a channel takes in (`next`).
#[cfg(any(test, feature = "test-support"))]
impl ChannelWatch {
    /// The next envelope shown, past where runs begin and end: for tests alone, since whoever
    /// follows a channel takes in where its runs begin and end (`next`).
    ///
    /// # Errors
    /// As `next`.
    pub async fn recv(&mut self) -> Result<Envelope, Lagged> {
        loop {
            if let Shown::Envelope(envelope) = self.next().await? {
                return Ok(envelope);
            }
        }
    }

    /// The next envelope shown, if one was, past where runs begin and end; `None` if none was: for
    /// tests alone, as `recv`.
    ///
    /// # Errors
    /// As `next`.
    pub fn try_recv(&mut self) -> Result<Option<Envelope>, Lagged> {
        loop {
            match self.shown.try_recv() {
                Ok(Shown::Envelope(envelope)) => return Ok(Some(envelope)),
                Ok(Shown::Started | Shown::Ended) => {}
                Err(TryRecvError::Lagged(lost)) => return Err(Lagged(lost)),
                // Held by this watch, the outbox is never closed.
                Err(TryRecvError::Empty | TryRecvError::Closed) => return Ok(None),
            }
        }
    }
}

/// What a `ChannelWatch` that fell behind lost of what was shown: how many
/// (issue 561) - the one way following a channel fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{0} of what was shown were lost")]
pub struct Lagged(pub u64);

impl UserActor {
    /// The actor of the user `user` - the session's controller of them,
    /// shared with their actors in its other channels (issue 120) - in the
    /// channel of `participant_runtime`.
    pub async fn new(
        user: Arc<UserController>,
        participant_runtime: ParticipantRuntime,
    ) -> Result<Self> {
        let channel_controller = Arc::clone(participant_runtime.channel_controller());
        let channel = Arc::clone(&channel_controller.load());
        let user_id = user.id();

        let (utx, urx) = mpsc::channel(100);
        let outbox = participant_runtime.actor_manager().outbox(channel.id, ActorId::User(user_id));

        Ok(Self {
            id: ActorId::User(user_id),
            session_id: channel.session_id,
            channel_id: channel.id,
            user,
            access_policy: Arc::clone(participant_runtime.access_policy()),
            participant_runtime,
            channel: channel_controller,
            user_rx: Mutex::new(urx),
            user_tx: utx,
            outbox,
            ended: CancellationToken::new(),
            run: sync::Mutex::new(Run::Before),
            open_questions: sync::Mutex::new(Vec::new()),
            open_approvals: sync::Mutex::new(HashMap::new()),
        })
    }

    /// An envelope from this user, in the span `span_id`, in `thread` (issue 602). Envelopes are
    /// only ever built here, so the sender, session and channel are always this actor's own
    /// (`Answering::own`), whatever the caller of `send_message` is. Nothing of what a user says is
    /// for its owner alone: it is sent as it is (`Answering::envelope`).
    fn envelope(&self, event: ActorEvent, span_id: SpanId, thread: ThreadId) -> Envelope {
        Answering::own(self.session_id, self.channel_id, thread, self.id(), self.name())
            .with_span(span_id)
            .envelope(event)
    }

    /// The thread `thread` names, the channel's line if `None` (issue 602): where what this user
    /// says, asks or issues happens.
    fn in_thread(&self, thread: Option<ThreadId>) -> ThreadId {
        thread.unwrap_or(ThreadId::line(self.channel_id))
    }

    /// An envelope from this user in the channel's line, in the span `span_id`: what happens in
    /// the whole channel - a change of a message (issue 602).
    fn in_line(&self, event: ActorEvent, span_id: SpanId) -> Envelope {
        self.envelope(event, span_id, ThreadId::line(self.channel_id))
    }

    /// The questions put to this user that are open, forgotten, each with the agent that asked
    /// and the thread it was asked in: a message declines them all, each in its thread.
    fn take_open_questions(&self) -> Vec<(QuestionId, AgentId, ThreadId)> {
        let mut open = self.open_questions.lock().unwrap_or_else(PoisonError::into_inner);
        let (asked, others): (Vec<_>, Vec<_>) =
            open.drain(..).partition(|(request, _)| request.questions.is_asked(self.id()));
        *open = others;
        asked.into_iter().map(|(request, thread)| (request.id, request.agent_id, thread)).collect()
    }

    /// The open questions put to this user that they have not answered or declined yet, the oldest
    /// first: what they may answer, and what a message of theirs declines.
    pub fn questions_to_answer(&self) -> Vec<QuestionRequest> {
        let open = self.open_questions.lock().unwrap_or_else(PoisonError::into_inner);
        let asked = open.iter().map(|(request, _)| request);
        asked.filter(|request| request.questions.is_asked(self.id())).cloned().collect()
    }

    /// What waits for this user in the channel (issue 594): the approvals asked of the users that
    /// have neither ended nor been answered by them, and the questions put to them that are open,
    /// as `questions_to_answer` gives them.
    pub fn waiting(&self) -> Waiting {
        let open = self.open_approvals.lock().unwrap_or_else(PoisonError::into_inner);
        let approvals = open.values().filter(|approval| !approval.answered).count();
        drop(open);
        let open = self.open_questions.lock().unwrap_or_else(PoisonError::into_inner);
        let questions = open.iter().filter(|(request, _)| request.questions.is_asked(self.id()));
        Waiting { approvals, questions: questions.count() }
    }

    /// Forgets the approval `id` (issue 594): it ended.
    fn close_approval(&self, id: ApprovalId) {
        self.open_approvals.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
    }

    /// Announces that what waits for this user changed (issue 594), where it differs from
    /// `announced`, what was announced last in the run, which it becomes: once the actor took in or
    /// sent what changed it - an approval or questions asked or ended, an answer of the user's, a
    /// message of theirs declining questions.
    fn announce_waiting(&self, announced: &mut Waiting) {
        let waiting = self.waiting();
        if *announced != waiting {
            *announced = waiting;
            self.channel.announce_waiting_changed(self.id());
        }
    }

    /// Answers the questions `id` put to this user with `responses`, one per question (issue 165) -
    /// or, with `None`, declines them. Looked at here, so that the user learns at once if the
    /// answer cannot be given: whether the questions are open and put to them, and whether
    /// `responses` answer them as asked. Once the answer is sent, the questions are the user's to
    /// answer no more, as those a message declines (issue 580). Whether they may answer in the
    /// channel at all (`AnswerQuestion`) is the channel's to decide as it admits the answer, which
    /// tells them of a rejection (`EventRejected`, issue 304); the questions stay forgotten then,
    /// since the reason is a standing one, and asking them again would only be rejected again. The
    /// agent asking checks again what it takes. Sent in the thread the questions were asked in
    /// (issue 602), whose context they are of. Returns the `span_id`, as `send_message` does.
    ///
    /// # Errors
    /// `ChannelError::Question` with why the answer cannot be given; an
    /// error if the channel is gone.
    pub async fn answer_questions(
        &self,
        id: QuestionId,
        responses: Option<Vec<Response>>,
    ) -> Result<SpanId> {
        self.channel_loop()?;
        let refused =
            |reason| ChannelError::Question { question: id, channel: self.channel_id, reason };
        let (request, thread) = {
            let mut open = self.open_questions.lock().unwrap_or_else(PoisonError::into_inner);
            let at = open
                .iter()
                .position(|(request, _)| request.id == id)
                .ok_or_else(|| refused(QuestionError::NotOpen))?;
            let (request, _) = &open[at];
            if !request.questions.is_asked(self.id()) {
                return Err(refused(QuestionError::NotAsked).into());
            }
            if let Some(responses) = &responses {
                request.questions.check_responses(responses).map_err(refused)?;
            }
            open.remove(at)
        };
        let agent_id = request.agent_id;
        let event = match responses {
            Some(responses) => ActorEvent::QuestionAnswered { id, agent_id, responses },
            None => ActorEvent::QuestionDeclined { id, agent_id },
        };
        let span_id = SpanId::new();
        self.send(self.envelope(event, span_id, thread)).await?;
        Ok(span_id)
    }

    /// Sends a message from this user into the channel, in `thread` - its line if `None` (issue
    /// 602) -: its text `content` and the attachments `attachments` names, which the user uploaded
    /// for the channel (`AttachmentManager::create`, issue 282), classified `classification` - the
    /// channel's if `None` (issue 133). A user may release what they write: classify it for a wider
    /// audience or a lower level than the channel, as long as it may flow into the channel. Whether
    /// it may is the channel's to decide as it admits the message, which tells the user of a
    /// rejection (`EventRejected`, issue 304) - the user asks nothing of the policy for what they
    /// send.
    ///
    /// Returns the `span_id` of the envelopes it starts, which every envelope
    /// caused by the message carries as well (see `caelumora_api::logging`).
    ///
    /// # Errors
    /// `AttachmentError::NotFound` for an attachment that is not one of this
    /// user's uploads for the channel; an error if the channel is gone.
    pub async fn send_message(
        &self,
        thread: Option<ThreadId>,
        content: Option<String>,
        classification: Option<Classification>,
        attachments: Vec<ContentId>,
    ) -> Result<SpanId> {
        let channel_loop = self.channel_loop()?;
        let thread = self.in_thread(thread);
        let classification = classification.unwrap_or_else(|| channel_loop.classification());
        let attachments = self.participant_runtime.attachment_manager().named(
            self.id,
            self.channel_id,
            &attachments,
            &[],
        )?;
        let mut msg = Message {
            role: MessageRole::User,
            content,
            attachments,
            id: MessageId::new(),
            sender_id: self.id(),
            sender_name: self.name().to_string(),
            timestamp: Utc::now(),
            ..Message::new(classification)
        };
        // Resolved as the channel will, for the actor shows its own message before (issue 220).
        msg.resolve_mentions(&channel_loop.mentions(), &channel_loop.taking_part_in(thread));
        // All stem from one user action: one span. A message instead of an
        // answer declines every question put to the user (issue 165), each in its thread -
        // before it, so that the agent asking goes on and then hears it.
        // Then the request to wake the channel's agents for it - the
        // channel does not wake by itself (issue 90). Said outside of any
        // turn, the message is answered by itself (issue 258).
        let span_id = SpanId::new();
        for (id, agent_id, asked_in) in self.take_open_questions() {
            let declined = ActorEvent::QuestionDeclined { id, agent_id };
            self.send(self.envelope(declined, span_id, asked_in)).await?;
        }
        self.send(self.envelope(ActorEvent::WakeupRequested, span_id, thread)).await?;
        let said = ActorEvent::CompleteMessageGenerated(msg);
        self.send(self.envelope(said, span_id, thread)).await?;
        Ok(span_id)
    }

    /// Sends `call`, a call this user made themselves - a command they ran in the workspace (issue
    /// 587) -, with its `result` as their message, in `thread` (issue 602), classified as the
    /// channel: the others hear the call with its result, as what the user did, and it asks nobody
    /// to respond, saying nothing in words (`Message::says_something`); nor does it wake anyone.
    /// Whether the result may enter the channel is the channel's to decide as it admits the
    /// message. The kernel's alone: what a frontend hands over as a result would be its word for
    /// what came out.
    ///
    /// # Errors
    /// An error if the channel is gone.
    pub(crate) async fn send_call(
        &self,
        thread: ThreadId,
        call: ToolCall,
        result: ToolCallResult,
    ) -> Result<()> {
        let channel_loop = self.channel_loop()?;
        let msg = Message {
            role: MessageRole::User,
            content: None,
            tool_calls: vec![call],
            tool_results: vec![result],
            id: MessageId::new(),
            sender_id: self.id(),
            sender_name: self.name().to_string(),
            timestamp: Utc::now(),
            ..Message::new(channel_loop.classification())
        };
        let said = ActorEvent::CompleteMessageGenerated(msg);
        self.send(self.envelope(said, SpanId::new(), thread)).await
    }

    /// Modifies this user's message `message` to read `content` from now on (issue 234), with the
    /// attachments `attachments` names - those it carries that it keeps, and uploads of the user
    /// for the channel to add (issue 282) -, classified `classification` - as it is classified now
    /// if `None`. The modified message speaks as its author, as every message does, and only its
    /// author's actor can send it (issue 228): another's is refused here, without asking the
    /// policy, as its transport would not pass it on. Whether they may modify it (`ModifyMessage`),
    /// whether it can be changed - not taken back - and whether it may enter the channel so
    /// classified is the channel's to decide as it admits the modification, as for `send_message`
    /// (issue 567). The message keeps its place; nobody is asked to respond to it.
    ///
    /// Returns the `span_id` of the envelope, as `send_message` does.
    ///
    /// # Errors
    /// `ChannelError::MessageNotFound` if the channel has no such message, `Forbidden` if it is not
    /// this user's; `AttachmentError::NotFound` for an attachment that is neither the message's as
    /// it stands nor an upload of the user's for the channel - of a message taken back, which
    /// carries none any more, every one it carried (issue 567); an error if the channel is gone.
    pub async fn modify_message(
        &self,
        message: MessageId,
        content: Option<String>,
        classification: Option<Classification>,
        attachments: Vec<ContentId>,
    ) -> Result<SpanId> {
        let channel_loop = self.channel_loop()?;
        let not_found = ChannelError::MessageNotFound { message, channel: self.channel_id };
        let (original, thread) = channel_loop.said_in(message).await.ok_or(not_found)?;
        if original.sender_id != self.id {
            let refused = ActionType::ModifyMessage {
                author: original.sender_id,
                channel: &channel_loop.security_attributes(),
            }
            .refused();
            return Err(ChannelError::Forbidden {
                action: "modifying a message of",
                channel: self.channel_id,
                refused,
            }
            .into());
        }
        let classification = classification.unwrap_or_else(|| original.classification.clone());
        let attachments = self.participant_runtime.attachment_manager().named(
            self.id,
            self.channel_id,
            &attachments,
            &original.attachments,
        )?;
        let mut modified =
            Message { content, attachments, classification, timestamp: Utc::now(), ..original };
        // Resolved as the channel will, where it was said, for the actor shows its own message
        // before (issue 220).
        modified.resolve_mentions(&channel_loop.mentions(), &channel_loop.taking_part_in(thread));
        let span_id = SpanId::new();
        self.send(self.in_line(ActorEvent::MessageModified(modified), span_id)).await?;
        Ok(span_id)
    }

    /// Takes this user's message `message` back (issue 234): it keeps its place, without what it
    /// said, and the agents are no longer given it. Whether they may (`RevokeMessage`) is the
    /// channel's to decide, as for `modify_message`. Not a deletion: what was said stays in the
    /// record of every participant that heard it.
    ///
    /// # Errors
    /// An error if the channel is gone. Whether the message is there and can be taken back the
    /// channel decides as it admits the revocation (issue 567).
    pub async fn revoke_message(&self, message: MessageId) -> Result<SpanId> {
        let span_id = SpanId::new();
        self.send(self.in_line(ActorEvent::MessageRevoked { message, at: Utc::now() }, span_id))
            .await?;
        Ok(span_id)
    }

    /// Reclassifies the message `message` as `classification` (issues 133, 239): its author's own,
    /// or any in a channel this user owns, never above the channel - the channel's to decide as it
    /// admits the reclassification (`Reclassify`, the flow), as for `send_message`. Recorded
    /// wherever the message is held, so that the agents' next turns are labelled by it as it
    /// stands. Nobody is asked to respond.
    ///
    /// Returns the `span_id` of the envelope, as `send_message` does.
    ///
    /// # Errors
    /// An error if the channel is gone. Whether the message is there and can be reclassified the
    /// channel decides as it admits the reclassification (issue 567).
    pub async fn reclassify_message(
        &self,
        message: MessageId,
        classification: Classification,
    ) -> Result<SpanId> {
        let span_id = SpanId::new();
        self.send(
            self.in_line(ActorEvent::MessageReclassified { message, classification }, span_id),
        )
        .await?;
        Ok(span_id)
    }

    /// Asks the agent that said `message` to answer again instead of it (issue 234), in the
    /// thread it was said in (issue 602). Whether this user may ask it (`RetryMessage`, whoever
    /// may read the channel) is the channel's to decide as it admits the request, as for
    /// `send_message`; it goes to that agent alone: whether it can answer again is the agent's to
    /// decide, by its own transcript, and it says so with `RetryAccepted` or `RetryRefused` -
    /// this returns once the request is on its way. No `WakeupRequested` goes ahead of it - that
    /// would wake every agent -; the channel wakes the one it asks, as for a nudge (issue 567).
    ///
    /// Returns the `span_id` of the envelopes, as `send_message` does.
    ///
    /// # Errors
    /// `ChannelError::MessageNotFound` if the channel has no such message; an error if the channel
    /// is gone. Whether it was taken back, and whether an agent of the channel said it, the channel
    /// decides as it admits the request (issue 567).
    pub async fn request_retry(&self, message: MessageId) -> Result<SpanId> {
        let channel_loop = self.channel_loop()?;
        let not_found = ChannelError::MessageNotFound { message, channel: self.channel_id };
        let (said, thread) = channel_loop.said_in(message).await.ok_or(not_found)?;
        self.send_retry(RetryTarget::Message(message), said.sender_id, thread).await
    }

    /// Asks the agent `author` to answer again instead of its answer under way, its turn `turn`
    /// (issue 235), in the thread the turn runs or ran in (issue 602). Whether this user may ask it
    /// (`RetryMessage`), whether `author` is an agent of the channel, and whether the turn can be
    /// answered again - the turn under way, or the agent's last turn given up for a failure or a
    /// lowering, with nothing after it (issue 259) - the channel decides as it admits the request,
    /// and tells the user if it does not (`EventRejected`, issue 567). The channel ends a turn
    /// under way as it delivers the request - if it is still the one under way, else it drops it -
    /// and the agent answers anew with `RetryAccepted`; this returns once the request is on its
    /// way, as `request_retry` sends it.
    ///
    /// # Errors
    /// An error if the channel is gone.
    pub async fn request_retry_of_turn(&self, author: ActorId, turn: TurnId) -> Result<SpanId> {
        let channel_loop = self.channel_loop()?;
        let thread = channel_loop.retryable_turn(author, turn).await;
        self.send_retry(RetryTarget::Turn(turn), author, self.in_thread(thread)).await
    }

    /// Asks `participant` - an agent, another user, any actor of the channel - to respond to the
    /// conversation as it stands in `thread` - its line if `None` (issue 602) -, routed by that
    /// thread's moderator (issue 278), without a message: a `NudgeRequested`, which the moderator
    /// turns into the request to `participant` alone. An agent answers it in a turn with this
    /// user's authority - or, asked as a turn of its own in this user's authority ends, finds it
    /// answered by that turn (issue 567); a user is shown it. It asks nothing of the policy: an
    /// `@`-mention in a message asks the same. Whether `participant` takes part in `thread` is the
    /// channel's to decide as it admits the request. No `WakeupRequested` goes ahead of it - that
    /// would wake every agent -; the channel wakes the one it asks, and ends being quiet for it.
    ///
    /// Returns the `span_id` of the envelope, as `send_message` does.
    ///
    /// # Errors
    /// `ChannelError::NudgesItself` if `participant` is this user; an error if
    /// the channel is gone.
    pub async fn request_response(
        &self,
        participant: ActorId,
        thread: Option<ThreadId>,
    ) -> Result<SpanId> {
        if participant == self.id {
            return Err(ChannelError::NudgesItself { participant, channel: self.channel_id }.into());
        }
        let span_id = SpanId::new();
        let nudge = ActorEvent::NudgeRequested { participant };
        self.send(self.envelope(nudge, span_id, self.in_thread(thread))).await?;
        Ok(span_id)
    }

    /// Sends the request to `author` to answer again instead of `target`, in `thread`: the channel
    /// wakes `author` alone for it, as for a nudge (issue 567). Returns the envelope's `span_id`.
    async fn send_retry(
        &self,
        target: RetryTarget,
        author: ActorId,
        thread: ThreadId,
    ) -> Result<SpanId> {
        let span_id = SpanId::new();
        let retry = ActorEvent::RetryRequested { target, author };
        self.send(self.envelope(retry, span_id, thread)).await?;
        Ok(span_id)
    }

    /// The channel this user is in, which every operation of theirs needs.
    ///
    /// # Errors
    /// An error if the channel is gone.
    fn channel_loop(&self) -> Result<Arc<ChannelLoop>> {
        self.participant_runtime
            .channel_loop()
            .ok_or_else(|| anyhow!("Channel {} is gone", self.channel_id))
    }

    /// Hands `envelope` to `run`, which shows it and forwards it into the
    /// channel (`forward`).
    async fn send(&self, envelope: Envelope) -> Result<()> {
        self.user_tx.send(envelope).await?;
        Ok(())
    }

    /// Shows `shown` to whoever follows this actor. An error only says that
    /// nobody does.
    fn show(&self, shown: Shown) {
        let _ = self.outbox.send(shown);
    }

    /// Begins the run and shows it - unless the actor was ended before it
    /// ran. Returns whether it began.
    fn begin_run(&self) -> bool {
        let mut run = self.run.lock().unwrap_or_else(PoisonError::into_inner);
        if *run != Run::Before {
            return false;
        }
        *run = Run::Running;
        self.show(Shown::Started);
        true
    }

    /// Ends the run and shows it, if it stands at `from`: an actor ended
    /// while it runs shows its end once its task has, after everything it
    /// showed; one ended before it ran, at once.
    fn end_run(&self, from: Run) {
        let mut run = self.run.lock().unwrap_or_else(PoisonError::into_inner);
        if *run == from {
            *run = Run::Over;
            self.show(Shown::Ended);
        }
    }

    /// Sends this user's answer to the approval request `id` (issue 567): whether they approve,
    /// with their part alone - their own, never one of their rules (`ApprovalDecision::by_rule`,
    /// issue 419). Which agent asked, and whether they may approve its calls, the approver tells,
    /// which holds the request: an answer it does not count it refuses (`EventRefused`). Sent in
    /// the thread the approval was asked in (issue 602), as this actor keeps it (`OpenApproval`,
    /// issue 603) - the line for one it does not know; the approver counts an answer to a request
    /// it holds whatever thread it comes in. Returns the `span_id`, as `send_message` does.
    ///
    /// # Errors
    /// An error if the channel is gone.
    pub async fn send_approval(&self, id: ApprovalId, approved: bool) -> Result<SpanId> {
        self.channel_loop()?;
        let decision = self.decision(None);
        let span_id = SpanId::new();
        let answer = ActorEvent::ApprovalAnswered { id, approved, decision };
        let thread = {
            let open = self.open_approvals.lock().unwrap_or_else(PoisonError::into_inner);
            open.get(&id).map(|approval| approval.thread)
        };
        self.send(self.envelope(answer, span_id, self.in_thread(thread))).await?;
        Ok(span_id)
    }

    /// This user's part in an approval, now - by their rule `by_rule` if
    /// one answers for them (issue 419).
    fn decision(&self, by_rule: Option<ApprovalRuleId>) -> ApprovalDecision {
        ApprovalDecision { by: self.id(), name: self.name().to_string(), by_rule, at: Utc::now() }
    }

    /// The answer the user's rules give to `request`, which `envelope`
    /// carries (issue 13), as an envelope of this user's - the way a click
    /// would be sent, carrying the rule. `None` if the approver did not ask,
    /// a rule of a place asked for the user's own decision (`explicit`), no
    /// rule of theirs applies, one only asks - a rule that denies, of a command
    /// line it cannot judge (issue 247) -, or they may not approve the agent's
    /// calls here - the approver would leave their answer out. That is offering,
    /// as a client asks it to show its buttons, not permitting: the approver
    /// asks it again as it counts the answer (issue 304, rule 5).
    async fn answer_by_rule(
        &self,
        envelope: &Envelope,
        request: &ApprovalRequest,
    ) -> Option<Envelope> {
        if request.explicit || !matches!(envelope.context.sender_id, ActorId::Approver(_)) {
            return None;
        }
        let (rule_id, decision) = {
            let profile = self.user.profile();
            let (rule, decision) = ApprovalRule::decide(
                profile.approval_rules(),
                &request.tool_call,
                request.session_id,
                request.channel_id,
                Utc::now(),
            )?;
            (rule.id, decision)
        };
        if decision == RuleDecision::Ask {
            return None;
        }
        let agent = match self.participant_runtime.agent_attributes(request.agent_id).await {
            Ok(attributes) => attributes,
            Err(e) => {
                warn!("UserActor {} cannot read agent {}: {}", self.id(), request.agent_id, e);
                return None;
            }
        };
        if !self.access_policy.may_approve(&self.security_context(), &agent).await {
            debug!(rule = %rule_id, "UserActor {} may not approve the calls of agent {}; its rule does not answer", self.id(), request.agent_id);
            return None;
        }
        let approved = decision == RuleDecision::Allow;
        info!(rule = %rule_id, approved, "UserActor {} answers approval {} by a rule", self.id(), request.id);
        let decision = self.decision(Some(rule_id));
        let answer = ActorEvent::ApprovalAnswered { id: request.id, approved, decision };
        Some(self.envelope(answer, envelope.context.span_id, envelope.context.thread_id))
    }

    /// Issues the user command `line` - the line after the slash - on behalf of this user (issue
    /// 28), in `thread` - the line if `None` (issue 602): where what it sends happens. Commands are
    /// for people, so only a `UserActor` issues them.
    ///
    /// A command of the channel - the one `line` begins with, by its name or an alias
    /// (`CommandExecutor::find`, issue 560) - runs here and answers either this user alone or the
    /// whole channel; in the latter case this actor announces it as a `CommandExecuted`, in its own
    /// name, by the command's name, whichever it was issued by. The answer is returned in both
    /// cases. A line no command of the session's set begins, as this user sees them, is looked for
    /// among the agents of the channel (`request_command`, issue 269). A channel-wide answer enters
    /// the history with its announcement (issue 417), a private one does not; what a command sends
    /// in the issuer's name - a call they made (`send_call`, issue 587) - enters it as theirs.
    ///
    /// # Errors
    /// A `CommandError` if the line is empty, or the command is unknown, refused or fails; an error
    /// if the channel is gone.
    pub async fn execute_command(
        &self,
        thread: Option<ThreadId>,
        line: &str,
    ) -> Result<CommandReply> {
        if line.trim().is_empty() {
            return Err(CommandError::Empty.into());
        }
        let channel = self.channel_loop()?;
        let executor = self.participant_runtime.command_executor();
        let thread = self.in_thread(thread);
        let context = CommandContext { issuer: self, channel: &channel, executor, thread };
        let Some((command, args)) = executor.find(line) else {
            return self.request_command(&channel, thread, line).await;
        };
        let name = &command.description().name;
        debug!("UserActor {} runs /{}", self.id, name);
        let output = match executor.execute(command, args, &context).await {
            Err(CommandError::Unknown(_)) => {
                return self.request_command(&channel, thread, line).await
            }
            result => result?,
        };
        if output.visibility == CommandVisibility::Channel {
            let event = ActorEvent::CommandExecuted {
                command: name.to_string(),
                args: args.to_string(),
                output: output.text.clone(),
                to: None,
                turn_id: None,
            };
            self.send(self.envelope(event, SpanId::new(), thread)).await?;
        }
        Ok(CommandReply {
            command: name.to_string(),
            output: Some(output.text),
            visibility: output.visibility,
            span_id: None,
        })
    }

    /// Asks agents of `channel` to carry out the command `line` begins with, in `thread` (issues
    /// 269, 602), by its name or an alias, as each agent's commands split the line (issue 560).
    /// What follows the command begins with whom: `@handle` names one agent, `@all` - or another
    /// group handle, as a mention (issue 401) - every agent here that carries the command out;
    /// the rest is the command's. Whom it names is found among the agents that take part in
    /// `thread` and carry it out as this user sees them (`ChannelLoop::agent_commands`,
    /// `ShowAgentCommand`) - offering, which asks nothing of the agent's queue. Whether this user
    /// may ask for it (`ExecuteAgentCommand`) is decided at the channel's door; a rejection is
    /// its `EventRejected` (issue 516). Sent as `CommandRequested`, by the command's name, one
    /// per agent under one span, through this actor's transport; no wakeup ahead of it - a
    /// sleeping agent carries a command out asleep.
    ///
    /// # Errors
    /// `CommandError::Unknown` if no agent here carries it out as this user sees it;
    /// `InvalidArguments` if the line names no agent that does; an error if a request cannot be
    /// sent.
    async fn request_command(
        &self,
        channel: &ChannelLoop,
        thread: ThreadId,
        line: &str,
    ) -> Result<CommandReply> {
        // Each agent that carries out a command the line begins with, with the command's name,
        // whom the line names after it and the rest, as that agent's commands split the line.
        // Of the agents that take part in the thread it is issued in (issue 602).
        let understanding: Vec<_> = channel
            .agent_commands(&self.security_context())
            .await
            .into_iter()
            .filter(|(agent, _)| channel.runs_in(agent.id, thread))
            .filter_map(|(agent, commands)| {
                let (name, args) = commands.into_iter().find_map(|command| {
                    command.strip_from(line).map(|args| (command.name, args))
                })?;
                let (whom, rest) = addressed(args);
                Some((agent, name, whom, rest.unwrap_or_default().trim_end()))
            })
            .collect();
        let Some((_, name, whom, _)) = understanding.first() else {
            return Err(CommandError::Unknown(first_word(line).to_string()).into());
        };
        let invalid =
            |reason: String| CommandError::InvalidArguments { command: name.to_string(), reason };
        let mut targets = Vec::new();
        for (agent, name, whom, rest) in &understanding {
            let addressed = match whom.strip_prefix('@') {
                Some(group) if is_group_handle(&handle_of(group)) => true,
                Some(handle) => match channel
                    .agents_named(handle)
                    .into_iter()
                    .filter(|named| channel.runs_in(named.id, thread))
                    .collect::<Vec<_>>()
                    .as_slice()
                {
                    [one] => one.id == agent.id,
                    [] => false,
                    several => {
                        let names: Vec<&str> = several.iter().map(|m| m.name.as_str()).collect();
                        let reason = format!("{whom} names several agents: {}", names.join(", "));
                        return Err(invalid(reason).into());
                    }
                },
                None => false,
            };
            if addressed {
                targets.push((agent.id, name, *rest));
            }
        }
        if targets.is_empty() {
            let handles: Vec<String> = understanding
                .iter()
                .filter_map(|(agent, _, _, _)| agent.handle.as_ref())
                .map(|h| format!("@{h}"))
                .collect();
            let reason = match whom.starts_with('@') {
                true => format!(
                    "{whom} does not carry it out here; name {}, or @all",
                    handles.join(", ")
                ),
                false => format!("name the agent: {}, or @all", handles.join(", ")),
            };
            return Err(invalid(reason).into());
        }
        let span_id = SpanId::new();
        for (agent, name, args) in &targets {
            let event = ActorEvent::CommandRequested {
                command: name.to_string(),
                args: args.to_string(),
                agent: *agent,
            };
            self.send(self.envelope(event, span_id, thread)).await?;
        }
        Ok(CommandReply {
            command: name.to_string(),
            output: None,
            visibility: CommandVisibility::Private,
            span_id: Some(span_id),
        })
    }

    /// The commands this user may see, e.g. for completion.
    pub async fn visible_commands(&self) -> Vec<CommandDescription> {
        let channel = self.channel.load().security_attributes.clone();
        self.participant_runtime
            .command_executor()
            .visible(&self.security_context(), &channel)
            .await
    }

    /// Hands `maybe_event` to whoever watches, and answers a request for approval that a rule of
    /// the user's answers (issue 13): sent as the user's own answer would be, and shown to the user
    /// as theirs. Keeps what waits for the user: the questions shown to them, and the approvals the
    /// approver asks, until they end (issue 594). `true` once the actor is to end - its inbox
    /// closed, or the channel is gone.
    async fn handle_actor_envelope(
        &self,
        maybe_event: Option<Envelope>,
        tx: &dyn EventSender,
    ) -> bool {
        let Some(event) = maybe_event else { return true };
        debug!("UserActor {} received {}", self.id(), event.describe());
        let answer = match &event.event {
            ActorEvent::ApprovalRequested { request, .. } => {
                let answered = matches!(event.context.sender_id, ActorId::Approver(_))
                    && self
                        .open_approvals
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .entry(request.id)
                        .or_insert(OpenApproval {
                            thread: event.context.thread_id,
                            answered: false,
                        })
                        .answered;
                if answered {
                    None
                } else {
                    self.answer_by_rule(&event, request).await
                }
            }
            ActorEvent::ApprovalGranted(verdict)
            | ActorEvent::ApprovalDenied(verdict)
            | ActorEvent::ApprovalTimedOut(verdict) => {
                self.close_approval(verdict.id);
                None
            }
            ActorEvent::ApprovalRequestWithdrawn { id } => {
                self.close_approval(*id);
                None
            }
            ActorEvent::QuestionRequested(request) => {
                // Asked anew by an agent that resumed its turn: kept where it was first asked.
                let asked = (request.clone(), event.context.thread_id);
                let mut open = self.open_questions.lock().unwrap_or_else(PoisonError::into_inner);
                match open.iter_mut().find(|(open, _)| open.id == request.id) {
                    Some(known) => *known = asked,
                    None => open.push(asked),
                }
                None
            }
            ActorEvent::QuestionClosed { id, .. } => {
                let mut open = self.open_questions.lock().unwrap_or_else(PoisonError::into_inner);
                open.retain(|(request, _)| request.id != *id);
                None
            }
            _ => None,
        };
        self.show(Shown::Envelope(event));
        match answer {
            Some(answer) => !self.forward(answer, tx).await,
            None => false,
        }
    }

    /// Forwards what the user sent into the channel, and shows it to those who follow this actor
    /// first, so that why the channel did not let it in (`EventRejected`, issue 304) always follows
    /// it: the channel sends nothing back to its sender, so as an agent takes its own messages in
    /// itself, this actor shows what its user says itself (issue 220) - all but the requests to
    /// wake the channel and to nudge an agent (issue 278), which are not said to anyone, and
    /// answers to questions, which are for the tool asking alone (issue 165): their closing tells
    /// the user's frontends that they were answered. Shown here rather than when it is queued, so
    /// that it falls within the run it is said in (issue 386). An answer of theirs to an approval
    /// is the end of its waiting for them (issue 594).
    ///
    /// `false` once the channel's inbox is gone - the only way the
    /// in-memory transport fails - and with it the channel: the actor then
    /// ends (issue 79).
    async fn forward(&self, envelope: Envelope, tx: &dyn EventSender) -> bool {
        debug!(span_id = %envelope.context.span_id, "UserActor {} sends {}", self.id(), envelope.event.kind());
        if let ActorEvent::ApprovalAnswered { id, .. } = &envelope.event {
            let mut open = self.open_approvals.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(approval) = open.get_mut(id) {
                approval.answered = true;
            }
        }
        if !matches!(
            envelope.event,
            ActorEvent::WakeupRequested
                | ActorEvent::NudgeRequested { .. }
                | ActorEvent::QuestionAnswered { .. }
        ) {
            self.show(Shown::Envelope(envelope.clone()));
        }
        match tx.send(envelope).await {
            Ok(()) => true,
            Err(TransportError::Closed) => {
                info!("UserActor {} ends: its channel is gone", self.id());
                false
            }
            Err(e) => {
                warn!("UserActor {} could not forward a message: {}", self.id(), e);
                true
            }
        }
    }
}

/// Where the run of a user's actor stands (issue 386).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Run {
    Before,
    Running,
    Over,
}

/// The end of the actor's task, however it ends - also when it is aborted
/// with its channel: the queue closes, so that what the user sends to a
/// stopped channel fails at once instead of filling a queue nobody reads
/// (issue 90), and whoever follows the actor is told, after everything it
/// showed (issue 386).
struct EndOfRun<'a> {
    queue: MutexGuard<'a, Receiver<Envelope>>,
    actor: &'a UserActor,
}

impl Drop for EndOfRun<'_> {
    fn drop(&mut self) {
        self.queue.close();
        self.actor.ended.cancel();
        self.actor.end_run(Run::Running);
    }
}

impl UserActor {
    /// The context this user acts in, here: what the commands they run and
    /// what they ask of the channel are checked in. What they send is
    /// admitted in the context the channel builds for them (issue 516).
    pub(crate) fn security_context(&self) -> SecurityContext {
        self.participant_runtime.acting_for(self.id)
    }
}

#[async_trait]
impl Actor for UserActor {
    fn id(&self) -> ActorId {
        self.id
    }

    fn name(&self) -> Name {
        self.user.load().username.clone()
    }

    /// With the introduction of their profile as it is now.
    fn describe(&self) -> ActorDescription {
        ActorDescription {
            id: self.id,
            name: self.name(),
            introduction: self.user.profile().introduction.clone(),
        }
    }

    fn state(&self) -> ActorState {
        ActorState::Idle
    }

    /// Ends the actor: its channel stops (issue 377). Its task ends, which
    /// it tells whoever follows it; it holds nothing to keep.
    fn stop(&self) {
        self.ended.cancel();
        self.end_run(Run::Before);
    }

    /// Ends the actor: it left the channel (issue 46). Its task ends, which
    /// it tells whoever follows it.
    fn leave(&self) {
        self.ended.cancel();
        self.end_run(Run::Before);
    }

    fn event_filter(&self) -> fn(&Envelope) -> bool {
        |_| true
    }

    async fn run(&self, mut rx: Box<dyn EventReceiver>, tx: Box<dyn EventSender>) -> Result<()> {
        let queue = self
            .user_rx
            .try_lock()
            .map_err(|_| anyhow!("UserActor {} is already running", self.id()))?;
        let mut run = EndOfRun { queue, actor: self };
        // Shown before anything of the run, which follows it.
        if !self.begin_run() {
            return Ok(());
        }

        info!("UserActor {} is starting", self.id());
        // What waits for the user as it was announced last (issue 594): nothing as the run begins.
        let mut announced = Waiting::default();
        // Until the channel closes this actor's inbox - the user left, or
        // the channel stopped - or the actor is told to stop.
        loop {
            tokio::select! {
                maybe_event = rx.recv() => {
                    if self.handle_actor_envelope(maybe_event, &*tx).await { break }
                }
                Some(envelope) = run.queue.recv() => {
                    if !self.forward(envelope, &*tx).await { break }
                }
                _ = self.ended.cancelled() => break,
            }
            self.announce_waiting(&mut announced);
        }
        info!("UserActor {} has finished", self.id());
        Ok(())
    }
}

/// What the tests do with a user's actor past its own API: follow the channel as the user, past
/// the views, and hand it what the channel would route to it.
#[cfg(any(test, feature = "test-support"))]
impl UserActor {
    /// Follows the channel as this user, from now on: see `ChannelWatch`. For tests alone: whoever
    /// follows a channel does so through the views, as the user, where the channel lists them
    /// (`ChannelView::user_follow`).
    pub fn watch(&self) -> ChannelWatch {
        ChannelWatch::of(&self.outbox)
    }

    /// Takes `envelope` in as if the channel had routed it to this actor: shown to whoever follows
    /// it, a question or an approval of the approver kept as open. What the actor would send for
    /// it - the answer of a rule of the user's to an approval - goes nowhere, and the actor takes
    /// the channel for gone; so answers whether the actor would end.
    pub async fn take_in_as_routed(&self, envelope: Envelope) -> bool {
        self.handle_actor_envelope(Some(envelope), &testing::nowhere()).await
    }
}

/// What the tests of a user's actor and its block for tests share.
#[cfg(any(test, feature = "test-support"))]
mod testing {
    use crate::transport::memory::InMemoryEventSender;
    use tokio::sync::mpsc;

    /// Where what an actor sends goes when nothing is to come of it.
    pub(super) fn nowhere() -> InMemoryEventSender {
        InMemoryEventSender::new(mpsc::channel(1).0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::manager::OUTBOX_CAPACITY;
    use crate::actor::user::testing::nowhere;
    use crate::kernel::{ChannelRuntime, Kernel};
    use crate::session::testing::test_kernel;
    use crate::transport::memory::{InMemoryEventReceiver, InMemoryEventSender};
    use caelumora_domain::actor::{EventContext, Rejection};
    use caelumora_domain::channel::testing::make_channel;
    use caelumora_domain::event::GlobalEvent;
    use caelumora_domain::question::testing::make_question_request;
    use caelumora_domain::session::testing::bare_session;
    use caelumora_domain::user::testing::make_user;
    use caelumora_domain::user::UserId;
    use caelumora_store::testing::record_users;
    use std::iter;
    use std::time::Duration;
    use tokio::time;

    /// A user's actor, with the channel it is in - to be kept while the
    /// actor is used: the actor's runtime holds it weakly.
    async fn user_actor() -> (UserActor, Arc<ChannelLoop>) {
        user_actor_of(test_kernel()).await
    }

    /// A user's actor of `kernel`, as `user_actor`.
    async fn user_actor_of(kernel: Arc<Kernel>) -> (UserActor, Arc<ChannelLoop>) {
        let session_id = SessionId::new();
        let channel = make_channel(session_id, "main");
        let user = make_user("alice");
        let channel_runtime = ChannelRuntime::outside_a_session(
            Arc::clone(&kernel),
            bare_session(session_id),
            channel,
        );
        let channel_loop = ChannelLoop::new(channel_runtime.clone()).await.unwrap();
        let participant_runtime = channel_runtime.new_participant_runtime(ActorId::User(user.id));
        record_users(kernel.entity_stores(), &[&user]).await;
        let actor = UserActor::new(
            kernel.controller_manager().user_controller(user.id).await.unwrap(),
            participant_runtime,
        )
        .await
        .unwrap();
        (actor, channel_loop)
    }

    fn envelope_from(sender: ActorId) -> Envelope {
        let context =
            EventContext::new(Default::default(), Default::default(), sender, "agent".to_string());
        Envelope::new(context, ActorEvent::ResponseStarted { turn_id: TurnId::new() })
    }

    #[tokio::test]
    async fn send_message_asks_for_a_wakeup_and_returns_the_span_of_all_its_envelopes() {
        let (actor, channel) = user_actor().await;
        let actor = Arc::new(actor);
        let (_inbox, rx) = mpsc::channel(1);
        let (output, mut channel_inbox) = mpsc::channel(10);
        let running = tokio::spawn({
            let actor = Arc::clone(&actor);
            async move {
                actor
                    .run(
                        Box::new(InMemoryEventReceiver::new(rx)),
                        Box::new(InMemoryEventSender::new(output)),
                    )
                    .await
            }
        });

        let span_id =
            actor.send_message(None, Some("hello".to_string()), None, Vec::new()).await.unwrap();

        let wakeup = channel_inbox.recv().await.expect("WakeupRequested");
        let complete = channel_inbox.recv().await.expect("CompleteMessageGenerated");
        assert!(
            matches!(wakeup.event, ActorEvent::WakeupRequested),
            "the wakeup first, so that the agents are awake when the message comes"
        );
        assert!(
            matches!(&complete.event, ActorEvent::CompleteMessageGenerated(m) if m.turn_id.is_none()),
            "said outside of any turn, answered by itself (issue 258)"
        );
        let more = time::timeout(Duration::from_millis(100), channel_inbox.recv()).await;
        assert!(more.is_err(), "and nothing else: there is no turn to end");
        for envelope in [&wakeup, &complete] {
            assert_eq!(envelope.context.span_id, span_id, "one user action, one span");
            assert_eq!(envelope.context.sender_id, actor.id());
            assert_eq!(envelope.context.channel_id, channel.id());
        }
        assert_ne!(wakeup.context.event_id, complete.context.event_id, "two events, two event ids");
        actor.stop();
        running.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn the_actor_shows_its_own_messages_to_those_who_follow_it_within_its_run() {
        let (actor, _channel) = user_actor().await;
        let actor = Arc::new(actor);
        let mut following = actor.watch();
        let (_inbox, rx) = mpsc::channel(1);
        let (output, mut channel_inbox) = mpsc::channel(10);
        let running = tokio::spawn({
            let actor = Arc::clone(&actor);
            async move {
                actor
                    .run(
                        Box::new(InMemoryEventReceiver::new(rx)),
                        Box::new(InMemoryEventSender::new(output)),
                    )
                    .await
            }
        });
        assert!(
            matches!(following.next().await, Ok(Shown::Started)),
            "its run begins first (issue 386)"
        );

        actor.send_message(None, Some("hello".to_string()), None, Vec::new()).await.unwrap();
        for _ in ["WakeupRequested", "CompleteMessageGenerated"] {
            channel_inbox.recv().await.expect("forwarded");
        }
        let shown: Vec<&str> =
            iter::from_fn(|| following.try_recv().ok().flatten()).map(|e| e.event.kind()).collect();
        assert_eq!(
            shown,
            ["CompleteMessageGenerated"],
            "what it said, as it forwarded it, not the request to wake the channel (issue 220)"
        );

        actor.send_approval(Default::default(), true).await.unwrap();
        channel_inbox.recv().await.expect("forwarded");
        assert_eq!(
            following.try_recv().ok().flatten().map(|e| e.event.kind()),
            Some("ApprovalAnswered"),
            "everything it sends"
        );

        actor.stop();
        running.await.unwrap().unwrap();
        assert!(
            matches!(following.next().await, Ok(Shown::Ended)),
            "and its end, after everything it showed"
        );
    }

    #[tokio::test]
    async fn an_actor_ended_before_it_ran_shows_its_end_alone() {
        let (actor, _channel) = user_actor().await;
        let mut following = actor.watch();
        actor.leave();
        assert!(
            matches!(following.next().await, Ok(Shown::Ended)),
            "its followers are told at once (issue 386)"
        );
        let ran = actor
            .run(Box::new(InMemoryEventReceiver::new(mpsc::channel(1).1)), Box::new(nowhere()))
            .await;
        let more = time::timeout(Duration::from_millis(100), following.next()).await;
        assert!(ran.is_ok() && more.is_err(), "and nothing more: it does not run");
    }

    #[tokio::test]
    async fn a_user_actor_whose_channel_is_gone_ends_without_an_error() {
        use crate::transport::{InMemoryEventReceiver, InMemoryEventSender};
        let (actor, _channel) = user_actor().await;
        let (_inbox, rx) = mpsc::channel(1);
        let (output, channel_inbox) = mpsc::channel(1);
        drop(channel_inbox);

        actor.send_message(None, Some("hello".to_string()), None, Vec::new()).await.unwrap();
        let ended = time::timeout(
            Duration::from_secs(5),
            actor.run(
                Box::new(InMemoryEventReceiver::new(rx)),
                Box::new(InMemoryEventSender::new(output)),
            ),
        )
        .await
        .expect("the actor ends");
        assert!(ended.is_ok(), "a channel that is gone is an end, not an error: {ended:?}");
    }

    #[tokio::test]
    async fn envelopes_without_a_subscriber_are_dropped_without_blocking() {
        let (actor, _channel) = user_actor().await;
        let agent = ActorId::Agent(Default::default());
        // More than any queue would hold: must neither block nor fail.
        for _ in 0..(OUTBOX_CAPACITY * 2) {
            assert!(!actor.handle_actor_envelope(Some(envelope_from(agent)), &nowhere()).await);
        }
        assert!(
            actor.handle_actor_envelope(None, &nowhere()).await,
            "a closed input ends the actor"
        );
    }

    /// A question of an agent, put to `addressee`, as the agent's actor routes it.
    fn asked_of(addressee: ActorId) -> (QuestionRequest, Envelope) {
        let request =
            make_question_request(SessionId::new(), ChannelId::new(), AgentId::new(), addressee);
        let mut envelope = envelope_from(ActorId::Agent(request.agent_id));
        envelope.event = ActorEvent::QuestionRequested(request.clone());
        (request, envelope)
    }

    /// Issue 602: questions are answered in the thread they were asked in, whose context they are
    /// of - also where a message, said elsewhere, declines them.
    #[tokio::test]
    async fn questions_are_answered_in_the_thread_they_were_asked_in() {
        let (actor, _channel) = user_actor().await;
        let actor = Arc::new(actor);
        let (_inbox, rx) = mpsc::channel(1);
        let (output, mut channel_inbox) = mpsc::channel(10);
        let running = tokio::spawn({
            let actor = Arc::clone(&actor);
            async move {
                actor
                    .run(
                        Box::new(InMemoryEventReceiver::new(rx)),
                        Box::new(InMemoryEventSender::new(output)),
                    )
                    .await
            }
        });
        let thread = ThreadId::new();
        let asked_there = |envelope: Envelope| {
            let context = EventContext { thread_id: thread, ..envelope.context };
            Envelope { context, ..envelope }
        };
        let (answered, asked) = asked_of(actor.id());
        assert!(!actor.handle_actor_envelope(Some(asked_there(asked)), &nowhere()).await);
        let responses = vec![Response::Text { text: "Kaya".to_string() }];
        actor.answer_questions(answered.id, Some(responses)).await.expect("answered");
        let answer = channel_inbox.recv().await.expect("the answer");
        assert_eq!(answer.context.thread_id, thread, "where the questions were asked");

        let (_, asked) = asked_of(actor.id());
        assert!(!actor.handle_actor_envelope(Some(asked_there(asked)), &nowhere()).await);
        actor.send_message(None, Some("rather not".to_string()), None, Vec::new()).await.unwrap();
        let declined = channel_inbox.recv().await.expect("the decline");
        assert!(matches!(declined.event, ActorEvent::QuestionDeclined { .. }), "declined first");
        assert_eq!(declined.context.thread_id, thread, "where they were asked");
        let said = channel_inbox.recv().await.expect("the wakeup");
        assert_ne!(said.context.thread_id, thread, "the message where it is said, the line");
        actor.stop();
        running.await.unwrap().unwrap();
    }

    /// The questions to answer are those put to the user, the oldest first, until they close;
    /// one asked anew keeps its place.
    #[tokio::test]
    async fn the_questions_to_answer_are_those_put_to_the_user_until_they_close() {
        let (actor, _channel) = user_actor().await;
        let (first, asked_first) = asked_of(actor.id());
        let (_, asked_of_another) = asked_of(ActorId::User(Default::default()));
        let (second, asked_second) = asked_of(actor.id());
        for envelope in [asked_first.clone(), asked_of_another, asked_second, asked_first] {
            assert!(!actor.handle_actor_envelope(Some(envelope), &nowhere()).await);
        }
        let ids = |actor: &UserActor| -> Vec<QuestionId> {
            actor.questions_to_answer().iter().map(|request| request.id).collect()
        };
        assert_eq!(ids(&actor), [first.id, second.id], "put to the user, the first asked first");

        let mut closed = envelope_from(ActorId::Agent(first.agent_id));
        closed.event = ActorEvent::QuestionClosed { id: first.id, shown_to: None };
        assert!(!actor.handle_actor_envelope(Some(closed), &nowhere()).await);
        assert_eq!(ids(&actor), [second.id], "until it closes");
    }

    /// Questions the user answered are theirs to answer no more as the answer is sent (issue 580),
    /// also where the channel rejects it - for a standing reason, so that asking them again would
    /// only be rejected again -, until an agent asks them anew.
    #[tokio::test]
    async fn questions_answered_are_to_answer_no_more_until_asked_anew() {
        let (actor, _channel) = user_actor().await;
        let (request, asked) = asked_of(actor.id());
        assert!(!actor.handle_actor_envelope(Some(asked.clone()), &nowhere()).await);
        let responses = vec![Response::Text { text: "Kaya".to_string() }];
        actor.answer_questions(request.id, Some(responses.clone())).await.expect("answered");
        assert!(actor.questions_to_answer().is_empty(), "answered");
        let again = actor.answer_questions(request.id, Some(responses)).await;
        assert!(
            matches!(
                again.expect_err("answered once").downcast_ref::<ChannelError>(),
                Some(ChannelError::Question { reason: QuestionError::NotOpen, .. })
            ),
            "not to be answered twice"
        );

        let mut rejected = envelope_from(ActorId::System);
        rejected.event = ActorEvent::EventRejected {
            rejected: "QuestionAnswered".to_string(),
            message: None,
            reason: Rejection::Forbidden,
        };
        assert!(!actor.handle_actor_envelope(Some(rejected), &nowhere()).await);
        assert!(actor.questions_to_answer().is_empty(), "not given back by a rejection");

        assert!(!actor.handle_actor_envelope(Some(asked), &nowhere()).await);
        let ids: Vec<QuestionId> = actor.questions_to_answer().iter().map(|q| q.id).collect();
        assert_eq!(ids, [request.id], "asked anew");
    }

    /// Issue 594: what waits for the user - the approvals the approver asks until they end or the
    /// user answers them, the questions put to them until they close or the user answers or
    /// declines them - is announced as it changes, naming the user, and only then: not for an
    /// approval asked anew, also once the user answered it (issue 603), nor for questions put to
    /// another.
    #[tokio::test]
    async fn what_waits_for_the_user_is_announced_as_it_changes() {
        use caelumora_domain::approval::testing::make_approval_request;
        use caelumora_domain::approval::{ApprovalVerdict, ApproverId};
        use caelumora_domain::channel::{Channel, WAITING_CHANGED};
        let kernel = test_kernel();
        let mut announced = kernel.event_bus().subscribe_all();
        let (actor, _channel) = user_actor_of(Arc::clone(&kernel)).await;
        let actor = Arc::new(actor);
        let (inbox, rx) = mpsc::channel(10);
        let (output, _channel_inbox) = mpsc::channel(100);
        let running = tokio::spawn({
            let actor = Arc::clone(&actor);
            async move {
                actor
                    .run(
                        Box::new(InMemoryEventReceiver::new(rx)),
                        Box::new(InMemoryEventSender::new(output)),
                    )
                    .await
            }
        });
        let changed_for = |event: &GlobalEvent| {
            event.about::<Channel>().and_then(|(_, change)| change.data::<ActorId>(WAITING_CHANGED))
        };
        let mut next_waiting = async || {
            time::timeout(Duration::from_secs(5), async {
                loop {
                    let event = announced.recv().await.expect("the bus runs");
                    if let Some(user) = changed_for(&event) {
                        return user;
                    }
                }
            })
            .await
            .expect("announced")
        };
        let approver = ActorId::Approver(ApproverId::new());
        let from = |sender, event| Envelope { event, ..envelope_from(sender) };
        let agent = AgentId::new();
        let asking = |request: &ApprovalRequest| {
            from(approver, ActorEvent::ApprovalRequested { request: request.clone(), to: None })
        };
        let request = make_approval_request(SessionId::new(), ChannelId::new(), agent);
        let asked = asking(&request);
        let waits = |approvals, questions| Waiting { approvals, questions };

        inbox.send(asked.clone()).await.unwrap();
        assert_eq!(next_waiting().await, actor.id(), "naming the user");
        assert_eq!(actor.waiting(), waits(1, 0), "the approval asked");
        inbox.send(asked.clone()).await.unwrap();
        let (questions, put_to_them) = asked_of(actor.id());
        let (_, put_to_another) = asked_of(ActorId::User(UserId::new()));
        inbox.send(put_to_another).await.unwrap();
        inbox.send(put_to_them).await.unwrap();
        next_waiting().await;
        assert_eq!(actor.waiting(), waits(1, 1), "asked anew, and put to another: unchanged");

        actor.send_approval(request.id, true).await.unwrap();
        next_waiting().await;
        assert_eq!(actor.waiting(), waits(0, 1), "answered by the user");
        inbox.send(asked).await.unwrap();
        let decided = make_approval_request(SessionId::new(), ChannelId::new(), agent);
        inbox.send(asking(&decided)).await.unwrap();
        next_waiting().await;
        let verdict = ActorEvent::ApprovalGranted(ApprovalVerdict::new(decided.id, agent));
        inbox.send(from(approver, verdict)).await.unwrap();
        next_waiting().await;
        assert_eq!(actor.waiting(), waits(0, 1), "decided");
        let closed = ActorEvent::QuestionClosed { id: questions.id, shown_to: None };
        inbox.send(from(ActorId::Agent(agent), closed)).await.unwrap();
        next_waiting().await;
        assert_eq!(actor.waiting(), waits(0, 0), "the questions closed");

        let withdrawn = make_approval_request(SessionId::new(), ChannelId::new(), agent);
        inbox.send(asking(&withdrawn)).await.unwrap();
        next_waiting().await;
        let withdrawal = ActorEvent::ApprovalRequestWithdrawn { id: withdrawn.id };
        inbox.send(from(approver, withdrawal)).await.unwrap();
        next_waiting().await;
        assert_eq!(actor.waiting(), waits(0, 0), "withdrawn");
        let (answered, asked) = asked_of(actor.id());
        inbox.send(asked).await.unwrap();
        next_waiting().await;
        let responses = vec![Response::Text { text: "Kaya".to_string() }];
        actor.answer_questions(answered.id, Some(responses)).await.unwrap();
        next_waiting().await;
        assert_eq!(actor.waiting(), waits(0, 0), "answered");
        let (_, asked) = asked_of(actor.id());
        inbox.send(asked).await.unwrap();
        next_waiting().await;
        actor.send_message(None, Some("rather not".to_string()), None, Vec::new()).await.unwrap();
        next_waiting().await;
        assert_eq!(actor.waiting(), waits(0, 0), "declined by a message");
        let more = announced.drain().await;
        assert!(more.iter().all(|event| changed_for(event).is_none()), "nothing more announced");
        actor.stop();
        running.await.unwrap().unwrap();
    }

    /// Issue 603: the user answers an approval in the thread it was asked in, as their actor keeps
    /// it, and one it does not know in the line.
    #[tokio::test]
    async fn an_approval_is_answered_where_it_was_asked() {
        use caelumora_domain::approval::testing::make_approval_request;
        use caelumora_domain::approval::ApproverId;
        let (actor, _channel) = user_actor().await;
        let actor = Arc::new(actor);
        let (_inbox, rx) = mpsc::channel(1);
        let (output, mut channel_inbox) = mpsc::channel(10);
        let running = tokio::spawn({
            let actor = Arc::clone(&actor);
            async move {
                actor
                    .run(
                        Box::new(InMemoryEventReceiver::new(rx)),
                        Box::new(InMemoryEventSender::new(output)),
                    )
                    .await
            }
        });
        let thread = ThreadId::new();
        let request = make_approval_request(SessionId::new(), ChannelId::new(), AgentId::new());
        let mut asked = envelope_from(ActorId::Approver(ApproverId::new()));
        asked.event = ActorEvent::ApprovalRequested { request: request.clone(), to: None };
        asked.context.thread_id = thread;
        assert!(!actor.handle_actor_envelope(Some(asked), &nowhere()).await);

        actor.send_approval(request.id, true).await.unwrap();
        let answer = channel_inbox.recv().await.expect("the answer");
        assert_eq!(answer.context.thread_id, thread, "where it was asked");
        actor.send_approval(ApprovalId::new(), true).await.unwrap();
        let unknown = channel_inbox.recv().await.expect("the answer");
        assert_eq!(unknown.context.thread_id, ThreadId::line(actor.channel_id), "in the line");
        actor.stop();
        running.await.unwrap().unwrap();
    }

    /// Issue 603: an approval a rule of the user's answers waits for them no more as it is asked -
    /// nothing is announced -, and asked anew it is not answered by the rule again.
    #[tokio::test]
    async fn an_approval_a_rule_answers_waits_for_nobody_and_is_answered_once() {
        use caelumora_domain::agent::testing::make_personality;
        use caelumora_domain::approval::testing::make_approval_request;
        use caelumora_domain::approval::{ApproverId, Reach, ToolMatcher};
        use caelumora_domain::channel::{Channel, WAITING_CHANGED};
        use caelumora_store::testing::stored_agent;
        let kernel = test_kernel();
        let mut announced = kernel.event_bus().subscribe_all();
        let (actor, _channel) = user_actor_of(Arc::clone(&kernel)).await;
        let theirs =
            stored_agent(kernel.entity_stores(), "helper", make_personality("helper"), actor.id())
                .await;
        let matcher = ToolMatcher { tool: "test_tool".to_string(), arguments: Default::default() };
        let rule =
            ApprovalRule::new(matcher, RuleDecision::Allow, Reach::Everywhere, None).unwrap();
        let ActorId::User(user) = actor.id() else { unreachable!("a user") };
        let hers = SecurityContext::new(actor.id(), Default::default(), Default::default());
        let controller = kernel.controller_manager().user_controller(user).await.unwrap();
        controller.add_approval_rule(&hers, rule).await.unwrap();
        let actor = Arc::new(actor);
        let mut watch = actor.watch();
        let (inbox, rx) = mpsc::channel(10);
        let (output, mut channel_inbox) = mpsc::channel(10);
        let running = tokio::spawn({
            let actor = Arc::clone(&actor);
            async move {
                actor
                    .run(
                        Box::new(InMemoryEventReceiver::new(rx)),
                        Box::new(InMemoryEventSender::new(output)),
                    )
                    .await
            }
        });
        let request = make_approval_request(SessionId::new(), ChannelId::new(), theirs.id);
        let asked = Envelope {
            event: ActorEvent::ApprovalRequested { request: request.clone(), to: None },
            ..envelope_from(ActorId::Approver(ApproverId::new()))
        };

        inbox.send(asked.clone()).await.unwrap();
        let answer = time::timeout(Duration::from_secs(5), channel_inbox.recv()).await.unwrap();
        assert!(
            matches!(
                answer.map(|a| a.event),
                Some(ActorEvent::ApprovalAnswered { id, .. }) if id == request.id
            ),
            "answered by the rule"
        );
        // Asked anew, then something else: taken in in turn, so that once the latter is shown, the
        // former was answered or not.
        inbox.send(asked).await.unwrap();
        let after = envelope_from(ActorId::Agent(theirs.id));
        let after_id = after.context.event_id;
        inbox.send(after).await.unwrap();
        time::timeout(Duration::from_secs(5), async {
            while watch.recv().await.expect("followed").context.event_id != after_id {}
        })
        .await
        .expect("taken in");
        actor.stop();
        running.await.unwrap().unwrap();
        assert!(channel_inbox.try_recv().is_err(), "answered once");
        assert_eq!(actor.waiting(), Waiting::default(), "waiting for nobody");
        let changed = announced.drain().await.into_iter().filter(|event| {
            event.about::<Channel>().and_then(|(_, c)| c.data::<ActorId>(WAITING_CHANGED)).is_some()
        });
        assert_eq!(changed.count(), 0, "nothing announced");
    }

    #[tokio::test]
    async fn every_subscriber_receives_what_is_routed_and_a_lagging_one_loses_the_oldest() {
        let (actor, _channel) = user_actor().await;
        let agent = ActorId::Agent(Default::default());
        let (mut first, mut second) = (actor.watch(), actor.watch());

        assert!(!actor.handle_actor_envelope(Some(envelope_from(agent)), &nowhere()).await);
        assert!(
            matches!((first.try_recv(), second.try_recv()), (Ok(Some(_)), Ok(Some(_)))),
            "two tabs, both are told"
        );

        for _ in 0..(OUTBOX_CAPACITY + 10) {
            assert!(!actor.handle_actor_envelope(Some(envelope_from(agent)), &nowhere()).await);
        }
        assert!(
            matches!(first.try_recv(), Err(Lagged(10))),
            "the oldest are lost, and it learns how many"
        );
        let mut received = 0;
        while let Ok(Some(_)) = first.try_recv() {
            received += 1;
        }
        assert_eq!(received, OUTBOX_CAPACITY, "the newest are kept");
        assert!(matches!(first.try_recv(), Ok(None)), "and then nothing");
        assert!(matches!(second.next().await, Err(Lagged(10))), "as the next shown tells");
    }
}
