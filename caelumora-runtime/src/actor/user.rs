use crate::actor::Shown;
use crate::actor::{Actor, Answering};
use crate::channel::{ChannelError, ChannelLoop};
use crate::command::{CommandContext, CommandError, CommandReply};
use crate::controller::{ChannelController, UserController};
use crate::runtime::ActorRuntime;
use crate::transport::{EventReceiver, EventSender, TransportError};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use caelumora_model::actor::{
    ActorDescription, ActorEvent, ActorId, ActorState, Envelope, RetryTarget,
};
use caelumora_model::agent::AgentId;
use caelumora_model::approval::{
    ApprovalDecision, ApprovalRequest, ApprovalResponse, ApprovalRule, ApprovalRuleId, RuleDecision,
};
use caelumora_model::channel::ChannelId;
use caelumora_model::command::{Command, CommandVisibility};
use caelumora_model::confidentiality::Classification;
use caelumora_model::content::ContentId;
use caelumora_model::mention::{handle_of, is_group_handle};
use caelumora_model::message::{Message, MessageId, MessageRole};
use caelumora_model::question::{QuestionError, QuestionId, QuestionRequest, Response};
use caelumora_model::security::{AccessPolicy, AccessQuestions, ActionType, SecurityContext};
use caelumora_model::session::SessionId;
use caelumora_model::value::name::Name;
use caelumora_model::work::TurnId;
use chrono::Utc;
use std::collections::HashMap;
use std::future;
use std::sync;
use std::sync::{Arc, PoisonError};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{broadcast, mpsc, Mutex, MutexGuard};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

#[derive(Debug)]
pub struct UserActor {
    id: ActorId,
    session_id: SessionId,
    channel_id: ChannelId,
    user: Arc<UserController>,
    actor_runtime: ActorRuntime,
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
    /// The questions shown to this user that are open (issue 165), as the
    /// agents asked them and until they close them: those put to the user
    /// are what they may answer, and what a message of theirs declines.
    open_questions: sync::Mutex<HashMap<QuestionId, QuestionRequest>>,
}

/// The channel as a user follows it (issues 104, 220, 386): what their
/// actor in it shows - what it is routed and says for them, and where its
/// runs begin and end -, across its runs: also while the channel does not
/// run, so that whoever follows it sees it run again. Only a participant
/// follows a channel live, and only through their actor's outbox.
pub struct ChannelWatch {
    shown: broadcast::Receiver<Shown>,
    /// Keeps the outbox while no actor runs.
    outbox: broadcast::Sender<Shown>,
}

impl ChannelWatch {
    /// Follows `outbox` from now on.
    pub(crate) fn of(outbox: &broadcast::Sender<Shown>) -> Self {
        Self { shown: outbox.subscribe(), outbox: outbox.clone() }
    }

    /// Another watch of the same outbox, from where this one stands.
    pub fn resubscribe(&self) -> Self {
        Self { shown: self.shown.resubscribe(), outbox: self.outbox.clone() }
    }

    /// What is shown next. One that falls `OUTBOX_CAPACITY` items behind
    /// loses the oldest (`RecvError::Lagged`) rather than stalling the
    /// actor, which would also stop it from forwarding what the user
    /// sends.
    ///
    /// # Errors
    /// `RecvError::Lagged` with how many were lost.
    pub async fn next(&mut self) -> Result<Shown, RecvError> {
        match self.shown.recv().await {
            // Held by this watch, the outbox is never closed.
            Err(RecvError::Closed) => future::pending().await,
            shown => shown,
        }
    }

    /// The next envelope shown, past where runs begin and end.
    ///
    /// # Errors
    /// As `next`.
    pub async fn recv(&mut self) -> Result<Envelope, RecvError> {
        loop {
            if let Shown::Envelope(envelope) = self.next().await? {
                return Ok(envelope);
            }
        }
    }

    /// The next envelope shown, if one was, past where runs begin and end.
    ///
    /// # Errors
    /// `TryRecvError::Empty` if none was, `Lagged` as `next`.
    pub fn try_recv(&mut self) -> Result<Envelope, TryRecvError> {
        loop {
            if let Shown::Envelope(envelope) = self.shown.try_recv()? {
                return Ok(envelope);
            }
        }
    }
}

impl UserActor {
    /// The actor of the user `user` - the session's controller of them,
    /// shared with their actors in its other channels (issue 120) - in the
    /// channel of `actor_runtime`.
    pub async fn new(user: Arc<UserController>, actor_runtime: ActorRuntime) -> Result<Self> {
        let channel_controller = Arc::clone(actor_runtime.channel_controller());
        let channel = Arc::clone(&channel_controller.load());
        let user_id = user.id();

        let (utx, urx) = mpsc::channel(100);
        let outbox = actor_runtime.actor_manager().outbox(channel.id, ActorId::User(user_id));

        Ok(Self {
            id: ActorId::User(user_id),
            session_id: channel.session_id,
            channel_id: channel.id,
            user,
            access_policy: Arc::clone(actor_runtime.access_policy()),
            actor_runtime,
            channel: channel_controller,
            user_rx: Mutex::new(urx),
            user_tx: utx,
            outbox,
            ended: CancellationToken::new(),
            run: sync::Mutex::new(Run::Before),
            open_questions: sync::Mutex::new(HashMap::new()),
        })
    }

    /// An envelope from this user, in the span `span_id`. Envelopes are only
    /// ever built here, so the sender, session, channel and security context
    /// are always this actor's own (`Answering::own`), whatever the caller of
    /// `send_message` is. Nothing of what a user says is for its owner
    /// alone: it is sent as it is (`Answering::envelope`).
    fn envelope(&self, event: ActorEvent, span_id: Uuid) -> Envelope {
        Answering::own(self.id(), self.name(), &self.security_context())
            .with_span(span_id)
            .envelope(event)
    }

    /// The questions put to this user that are open, forgotten: a message
    /// declines them all.
    fn take_open_questions(&self) -> Vec<(QuestionId, AgentId)> {
        let mut open = self.open_questions.lock().unwrap_or_else(PoisonError::into_inner);
        let asked: Vec<QuestionId> =
            open.values().filter(|r| r.questions.is_asked(self.id())).map(|r| r.id).collect();
        asked
            .iter()
            .filter_map(|id| open.remove(id))
            .map(|request| (request.id, request.agent_id))
            .collect()
    }

    /// Answers the questions `id` put to this user with `responses`, one per
    /// question (issue 165) - or, with `None`, declines them. Looked at
    /// here, so that the user learns at once if the answer cannot be given:
    /// whether the questions are open and put to them, and whether
    /// `responses` answer them as asked. Whether they may answer in the
    /// channel at all (`AnswerQuestion`) is the channel's to decide as it
    /// admits the answer, which tells them of a refusal (`MessageRejected`,
    /// issue 304); the agent asking checks again what it takes. Returns the
    /// `span_id`, as `send_message` does.
    ///
    /// # Errors
    /// `ChannelError::Question` with why the answer cannot be given; an
    /// error if the channel is gone.
    pub async fn answer_questions(
        &self,
        id: QuestionId,
        responses: Option<Vec<Response>>,
    ) -> Result<Uuid> {
        self.channel_loop()?;
        let refused =
            |reason| ChannelError::Question { question: id, channel: self.channel_id, reason };
        let agent_id = {
            let open = self.open_questions.lock().unwrap_or_else(PoisonError::into_inner);
            let request = open.get(&id).ok_or_else(|| refused(QuestionError::NotOpen))?;
            if !request.questions.is_asked(self.id()) {
                return Err(refused(QuestionError::NotAsked).into());
            }
            if let Some(responses) = &responses {
                request.questions.check_responses(responses).map_err(refused)?;
            }
            request.agent_id
        };
        let event = match responses {
            Some(responses) => ActorEvent::QuestionAnswered { id, agent_id, responses },
            None => ActorEvent::QuestionDeclined { id, agent_id },
        };
        let span_id = Uuid::new_v4();
        self.send(self.envelope(event, span_id)).await?;
        Ok(span_id)
    }

    /// Sends a message from this user into the channel - its text `content`
    /// and the attachments `attachments` names, which the user uploaded for
    /// the channel (`AttachmentManager::create`, issue 282) -, classified
    /// `classification` - the channel's if `None` (issue 133). A user may
    /// release what they write: classify it for a wider audience or a
    /// lower level than the channel, as long as it may flow into the
    /// channel. Whether it may is the channel's to decide as it admits the
    /// message, which tells the user of a refusal (`MessageRejected`, issue
    /// 304) - the user asks nothing of the policy for what they send.
    ///
    /// Returns the `span_id` of the envelopes it starts, which every envelope
    /// caused by the message carries as well (see `crate::logging`).
    ///
    /// # Errors
    /// `AttachmentError::NotFound` for an attachment that is not one of this
    /// user's uploads for the channel; an error if the channel is gone.
    pub async fn send_message(
        &self,
        content: Option<String>,
        classification: Option<Classification>,
        attachments: Vec<ContentId>,
    ) -> Result<Uuid> {
        let channel_loop = self.channel_loop()?;
        let classification = classification.unwrap_or_else(|| channel_loop.classification());
        let attachments = self.actor_runtime.attachment_manager().named(
            self.id,
            self.channel_id,
            &attachments,
            &[],
        )?;
        let msg = Message {
            role: MessageRole::User,
            mentions: content
                .as_deref()
                .map(|content| channel_loop.mentions_in(content))
                .unwrap_or_default(),
            content,
            attachments,
            id: MessageId(Uuid::new_v4()),
            sender_id: self.id(),
            sender_name: self.name().to_string(),
            timestamp: Utc::now(),
            ..Message::new(classification)
        };
        // All stem from one user action: one span. A message instead of an
        // answer declines every question put to the user (issue 165) -
        // before it, so that the agent asking goes on and then hears it.
        // Then the request to wake the channel's agents for it - the
        // channel does not wake by itself (issue 90). Said outside of any
        // turn, the message is answered by itself (issue 258).
        let span_id = Uuid::new_v4();
        for (id, agent_id) in self.take_open_questions() {
            self.send(self.envelope(ActorEvent::QuestionDeclined { id, agent_id }, span_id))
                .await?;
        }
        self.send(self.envelope(ActorEvent::WakeupRequested, span_id)).await?;
        self.send(self.envelope(ActorEvent::CompleteMessageGenerated(msg), span_id)).await?;
        Ok(span_id)
    }

    /// Modifies this user's message `message` to read `content` from now on
    /// (issue 234), with the attachments `attachments` names - those it
    /// carries that it keeps, and uploads of the user for the channel to add
    /// (issue 282) -, classified `classification` - as it is classified now
    /// if `None`. The modified message speaks as its author, as every
    /// message does, and only its author's actor can send it (issue 228):
    /// another's is refused here, without asking the policy. Whether they
    /// may modify it (`ModifyMessage`) and whether it may enter the channel
    /// so classified is the channel's to decide as it admits the
    /// modification, as for `send_message`. The message keeps its place;
    /// nobody is asked to respond to it.
    ///
    /// Returns the `span_id` of the envelope, as `send_message` does.
    ///
    /// # Errors
    /// `ChannelError::MessageNotFound` if the channel has no such message,
    /// `Forbidden` if it is not this user's, `MessageRevoked` if it was
    /// taken back; `AttachmentError::NotFound` for an attachment that is
    /// neither the message's nor an upload of the user's for the channel; an
    /// error if the channel is gone.
    pub async fn modify_message(
        &self,
        message: MessageId,
        content: Option<String>,
        classification: Option<Classification>,
        attachments: Vec<ContentId>,
    ) -> Result<Uuid> {
        let channel_loop = self.channel_loop()?;
        let original = self.message_to_change(&channel_loop, message).await?;
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
        let attachments = self.actor_runtime.attachment_manager().named(
            self.id,
            self.channel_id,
            &attachments,
            &original.attachments,
        )?;
        let modified = Message {
            mentions: content
                .as_deref()
                .map(|content| channel_loop.mentions_in(content))
                .unwrap_or_default(),
            content,
            attachments,
            classification,
            timestamp: Utc::now(),
            ..original
        };
        let span_id = Uuid::new_v4();
        self.send(self.envelope(ActorEvent::MessageModified(modified), span_id)).await?;
        Ok(span_id)
    }

    /// Takes this user's message `message` back (issue 234): it keeps its
    /// place, without what it said, and the agents are no longer given it.
    /// Whether they may (`RevokeMessage`) is the channel's to decide, as for
    /// `modify_message`. Not a deletion: what was said stays in the record
    /// of every participant that heard it.
    ///
    /// # Errors
    /// As `modify_message`, but for the classification.
    pub async fn revoke_message(&self, message: MessageId) -> Result<Uuid> {
        let channel_loop = self.channel_loop()?;
        self.message_to_change(&channel_loop, message).await?;
        let span_id = Uuid::new_v4();
        self.send(self.envelope(ActorEvent::MessageRevoked { message, at: Utc::now() }, span_id))
            .await?;
        Ok(span_id)
    }

    /// Reclassifies the message `message` as `classification` (issues 133,
    /// 239): its author's own, or any in a channel this user owns, never
    /// above the channel - the channel's to decide as it admits the
    /// reclassification (`Reclassify`, the flow), as for `send_message`.
    /// Recorded wherever the message is held, so that the agents' next turns
    /// are labelled by it as it stands. Nobody is asked to respond.
    ///
    /// Returns the `span_id` of the envelope, as `send_message` does.
    ///
    /// # Errors
    /// `ChannelError::MessageNotFound` if the channel has no such message,
    /// `MessageRevoked` if it was taken back; an error if the channel is
    /// gone.
    pub async fn reclassify_message(
        &self,
        message: MessageId,
        classification: Classification,
    ) -> Result<Uuid> {
        let channel_loop = self.channel_loop()?;
        self.message_to_change(&channel_loop, message).await?;
        let span_id = Uuid::new_v4();
        self.send(
            self.envelope(ActorEvent::MessageReclassified { message, classification }, span_id),
        )
        .await?;
        Ok(span_id)
    }

    /// Asks the agent that said `message` to answer again instead of it
    /// (issue 234). Whether this user may ask it (`RetryMessage`, whoever
    /// may read the channel) is the channel's to decide as it admits the
    /// request, as for `send_message`; it goes to that agent alone: whether it can answer again is
    /// the agent's to decide, by its own transcript, and it says so with `RetryAccepted`
    /// or `RetryRejected` - this returns once the request is on its way. A
    /// `WakeupRequested` goes ahead of it, as of a message, so that a quiet
    /// channel does not swallow it.
    ///
    /// Returns the `span_id` of the envelopes, as `send_message` does.
    ///
    /// # Errors
    /// `ChannelError::MessageNotFound` if the channel has no such message,
    /// `MessageRevoked` if it was taken back, `NotAnAgentsMessage` if no agent of the channel said
    /// it; an error if the channel is gone.
    pub async fn request_retry(&self, message: MessageId) -> Result<Uuid> {
        let channel_loop = self.channel_loop()?;
        let author = self.message_to_change(&channel_loop, message).await?.sender_id;
        if channel_loop.agent_actor(author).is_none() {
            return Err(
                ChannelError::NotAnAgentsMessage { message, channel: self.channel_id }.into()
            );
        }
        self.send_retry(RetryTarget::Message(message), author).await
    }

    /// Asks the agent `author` to answer again instead of its answer under
    /// way, its turn `turn` (issue 235). Whether this user may ask it
    /// (`RetryMessage`) is the channel's to decide, as for a message; the
    /// turn is looked at here: the turn under way, or the
    /// agent's last turn given up for a failure or a lowering, with nothing
    /// after it (issue 259) - the user learns at once if it is neither. The
    /// channel ends a turn under way as it delivers the request - if it is
    /// still the one under way, else it drops it - and the agent answers
    /// anew with `RetryAccepted`; this returns once the request is on its
    /// way, with a `WakeupRequested` ahead of it, as `request_retry` sends.
    ///
    /// # Errors
    /// `ChannelError::NotAParticipant` if `author` is no agent of the
    /// channel; `TurnNotRetryable` if `turn` can be asked for again
    /// neither so;
    /// an error if the channel is gone.
    pub async fn request_retry_of_turn(&self, author: ActorId, turn: TurnId) -> Result<Uuid> {
        let channel_loop = self.channel_loop()?;
        if channel_loop.agent_actor(author).is_none() {
            return Err(ChannelError::NotAParticipant {
                participant: author,
                channel: self.channel_id,
            }
            .into());
        }
        if !channel_loop.retryable_turn(author, turn).await {
            return Err(ChannelError::TurnNotRetryable { turn, channel: self.channel_id }.into());
        }
        self.send_retry(RetryTarget::Turn(turn), author).await
    }

    /// Asks `participant` - an agent, another user, any actor of the
    /// channel - to respond to the conversation as it stands (issue 278),
    /// without a message: a `NudgeRequested`, which the moderator turns into
    /// the request to `participant` alone. An agent answers it in a turn with
    /// this user's authority; a user is shown it. It asks nothing of the
    /// policy: an `@`-mention in a message asks the same. No
    /// `WakeupRequested` goes ahead of it - that would wake every agent -;
    /// the channel wakes the one it asks, and ends being quiet for it.
    ///
    /// Returns the `span_id` of the envelope, as `send_message` does.
    ///
    /// # Errors
    /// `ChannelError::NotAParticipant` if `participant` takes no part in the
    /// channel; `NudgesItself` if it is this user; `AlreadyResponding` if it
    /// is in a turn, which it would otherwise follow with a second one; an
    /// error if the channel is gone.
    pub async fn request_response(&self, participant: ActorId) -> Result<Uuid> {
        let channel_loop = self.channel_loop()?;
        let actor = channel_loop
            .actor(participant)
            .ok_or(ChannelError::NotAParticipant { participant, channel: self.channel_id })?;
        if participant == self.id {
            return Err(ChannelError::NudgesItself { participant, channel: self.channel_id }.into());
        }
        if actor.state().in_turn() {
            return Err(
                ChannelError::AlreadyResponding { participant, channel: self.channel_id }.into()
            );
        }
        let span_id = Uuid::new_v4();
        self.send(self.envelope(ActorEvent::NudgeRequested { participant }, span_id)).await?;
        Ok(span_id)
    }

    /// Sends the request to `author` to answer again instead of `target`,
    /// with a `WakeupRequested` ahead of it, as of a message, so that a
    /// quiet channel does not swallow it. Returns the envelopes' `span_id`.
    async fn send_retry(&self, target: RetryTarget, author: ActorId) -> Result<Uuid> {
        let span_id = Uuid::new_v4();
        self.send(self.envelope(ActorEvent::WakeupRequested, span_id)).await?;
        self.send(self.envelope(ActorEvent::RetryRequested { target, author }, span_id)).await?;
        Ok(span_id)
    }

    /// The channel this user is in, which every operation of theirs needs.
    ///
    /// # Errors
    /// An error if the channel is gone.
    fn channel_loop(&self) -> Result<Arc<ChannelLoop>> {
        self.actor_runtime
            .channel_loop()
            .ok_or_else(|| anyhow!("Channel {} is gone", self.channel_id))
    }

    /// The message `id` of `channel` as it stands, if it can still be
    /// changed: not taken back. Whether this user may change it is the
    /// channel's to decide as it admits the change (issue 304).
    async fn message_to_change(&self, channel: &ChannelLoop, id: MessageId) -> Result<Message> {
        let message = channel
            .message(id)
            .await
            .ok_or(ChannelError::MessageNotFound { message: id, channel: self.channel_id })?;
        if message.revoked.is_some() {
            return Err(
                ChannelError::MessageRevoked { message: id, channel: self.channel_id }.into()
            );
        }
        Ok(message)
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

    /// Sends this user's decision on an approval. Returns the `span_id`, as
    /// `send_message` does. A decision sent so is the user's own, never one
    /// of their rules (`ApprovalDecision::by_rule`): whatever `response` says
    /// of who decided is replaced by this user, now (issue 419).
    pub async fn send_approval(&self, response: ApprovalResponse, approved: bool) -> Result<Uuid> {
        let response = ApprovalResponse { decisions: vec![self.decision(None)], ..response };
        let span_id = Uuid::new_v4();
        self.send(self.envelope(Self::answer(response, approved), span_id)).await?;
        Ok(span_id)
    }

    fn answer(response: ApprovalResponse, approved: bool) -> ActorEvent {
        if approved {
            ActorEvent::ApprovalGranted(response)
        } else {
            ActorEvent::ApprovalDenied(response)
        }
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
        let agent = match self.actor_runtime.agent_attributes(request.agent_id).await {
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
        let response = ApprovalResponse {
            decisions: vec![self.decision(Some(rule_id))],
            ..ApprovalResponse::new(request.id, request.agent_id)
        };
        Some(self.envelope(Self::answer(response, approved), envelope.context.span_id))
    }

    /// Issues the user command `name` with `args` on behalf of this user
    /// (issue 28). Commands are for people, so only a `UserActor` issues
    /// them.
    ///
    /// A command of the channel runs here and answers either this user alone
    /// or the whole channel; in the latter case this actor announces it as a
    /// `CommandExecuted`, in its own name. The answer is returned in both
    /// cases. A command the session's set does not know is looked for among
    /// the agents of the channel (`request_command`, issue 269). Commands
    /// never enter the history; what an agent carried out does.
    ///
    /// # Errors
    /// A `CommandError` if the command is unknown, refused or fails; an error
    /// if the channel is gone.
    pub async fn execute_command(&self, name: &str, args: &str) -> Result<CommandReply> {
        let channel = self.channel_loop()?;
        let executor = self.actor_runtime.command_executor();
        let context = CommandContext { issuer: self, channel: &channel, executor };
        debug!("UserActor {} runs /{}", self.id, name);
        let output = match executor.execute(name, args, &context).await {
            Err(CommandError::Unknown(_)) => {
                return self.request_command(&channel, name, args).await
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
            self.send(self.envelope(event, Uuid::new_v4())).await?;
        }
        Ok(CommandReply {
            command: name.to_string(),
            output: Some(output.text),
            visibility: output.visibility,
            span_id: None,
        })
    }

    /// Asks agents of `channel` to carry out the command `name` (issue 269).
    /// `args` begins with whom: `@handle` names one agent, `@all` - or
    /// another group handle, as a mention (issue 401) - every agent here that
    /// carries the command out; the rest is the command's.
    /// Whom it names is found among the agents that carry it out as this
    /// user sees them (`ChannelLoop::agent_commands`, `ShowAgentCommand`) -
    /// offering, which asks nothing of the agent's queue. Whether this user
    /// may ask for it (`ExecuteAgentCommand`) is each agent's to decide, once,
    /// as it carries it out; a refusal is its `CommandFailed` (issue 304).
    /// Sent as `CommandRequested`, one per agent under one span, through
    /// this actor's transport; no wakeup ahead of it - a sleeping agent
    /// carries a command out asleep.
    ///
    /// # Errors
    /// `CommandError::Unknown` if no agent here carries it out as this user
    /// sees it; `InvalidArguments` if `args` names no agent that does; an
    /// error if a request cannot be sent.
    async fn request_command(
        &self,
        channel: &ChannelLoop,
        name: &str,
        args: &str,
    ) -> Result<CommandReply> {
        let understanding: Vec<_> = channel
            .agent_commands(&self.security_context())
            .await
            .into_iter()
            .filter(|(_, commands)| commands.iter().any(|c| c.name == name))
            .map(|(agent, _)| agent)
            .collect();
        if understanding.is_empty() {
            return Err(CommandError::Unknown(name.to_string()).into());
        }
        let handles: Vec<String> = understanding
            .iter()
            .filter_map(|m| m.handle.as_ref())
            .map(|h| format!("@{h}"))
            .collect();
        let invalid =
            |reason: String| CommandError::InvalidArguments { command: name.to_string(), reason };
        let (whom, rest) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
        let targets: Vec<_> = match whom.strip_prefix('@') {
            Some(group) if is_group_handle(&handle_of(group)) => understanding,
            Some(agent) => {
                let named = channel.agents_named(agent);
                if named.len() > 1 {
                    let names: Vec<&str> = named.iter().map(|m| m.name.as_str()).collect();
                    return Err(invalid(format!(
                        "{whom} names several agents: {}",
                        names.join(", ")
                    ))
                    .into());
                }
                understanding.into_iter().filter(|m| named.iter().any(|n| n.id == m.id)).collect()
            }
            None => {
                return Err(
                    invalid(format!("name the agent: {}, or @all", handles.join(", "))).into()
                )
            }
        };
        if targets.is_empty() {
            return Err(invalid(format!(
                "{whom} does not carry it out here; name {}, or @all",
                handles.join(", ")
            ))
            .into());
        }
        let span_id = Uuid::new_v4();
        for agent in targets {
            let event = ActorEvent::CommandRequested {
                command: name.to_string(),
                args: rest.trim().to_string(),
                agent: agent.id,
            };
            self.send(self.envelope(event, span_id)).await?;
        }
        Ok(CommandReply {
            command: name.to_string(),
            output: None,
            visibility: CommandVisibility::Private,
            span_id: Some(span_id),
        })
    }

    /// The commands this user may see, e.g. for completion.
    pub async fn visible_commands(&self) -> Vec<Command> {
        let channel = self.channel.load().security_attributes.clone();
        self.actor_runtime.command_executor().visible(&self.security_context(), &channel).await
    }

    /// Follows the channel as this user, from now on: see `ChannelWatch`.
    /// Any number may follow - two tabs, two devices (issue 220); without
    /// anyone following, what is shown is dropped.
    pub fn watch(&self) -> ChannelWatch {
        ChannelWatch::of(&self.outbox)
    }

    /// Hands `maybe_event` to whoever watches, and answers a request for
    /// approval that a rule of the user's answers (issue 13): sent as the
    /// user's own answer would be, and shown to the user as theirs. `true`
    /// once the actor is to end - its inbox closed, or the channel is gone.
    async fn handle_actor_envelope(
        &self,
        maybe_event: Option<Envelope>,
        tx: &dyn EventSender,
    ) -> bool {
        let Some(event) = maybe_event else { return true };
        debug!("UserActor {} received {}", self.id(), event.describe());
        let answer = match &event.event {
            ActorEvent::ApprovalRequested(request) => self.answer_by_rule(&event, request).await,
            ActorEvent::QuestionRequested(request) => {
                self.open_questions
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(request.id, request.clone());
                None
            }
            ActorEvent::QuestionClosed { id, .. } => {
                self.open_questions.lock().unwrap_or_else(PoisonError::into_inner).remove(id);
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

    /// Forwards what the user sent into the channel, and shows it to those
    /// who follow this actor first, so that why the channel did not let it
    /// in (`MessageRejected`, issue 304) always follows it: the channel
    /// sends nothing back to its sender, so as an agent takes its own
    /// messages in itself, this actor shows what its user says itself
    /// (issue 220) - all but the requests to wake the channel and to nudge
    /// an agent (issue 278), which are not said to anyone, and answers to
    /// questions, which are for the tool asking alone (issue 165): their
    /// closing tells the user's frontends that they were answered. Shown
    /// here rather than when it is queued, so that it falls within the run
    /// it is said in (issue 386).
    ///
    /// `false` once the channel's inbox is gone - the only way the
    /// in-memory transport fails - and with it the channel: the actor then
    /// ends (issue 79).
    async fn forward(&self, envelope: Envelope, tx: &dyn EventSender) -> bool {
        debug!(span_id = %envelope.context.span_id, "UserActor {} sends {}", self.id(), envelope.event.kind());
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

#[async_trait]
impl Actor for UserActor {
    /// The context this user acts in, here - what their answers carry, so
    /// that whoever decides on them asks about them in the same one (the
    /// approver, issue 14).
    fn security_context(&self) -> SecurityContext {
        self.user.security_context(self.session_id, self.channel_id)
    }

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
        }
        info!("UserActor {} has finished", self.id());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::manager::OUTBOX_CAPACITY;
    use crate::runtime::ChannelRuntime;
    use crate::session::testing::test_runtime;
    use crate::transport::memory::{InMemoryEventReceiver, InMemoryEventSender};
    use caelumora_model::actor::EventContext;
    use caelumora_model::channel::testing::make_channel;
    use caelumora_model::session::testing::bare_session;
    use caelumora_model::user::{User, UserId};
    use std::iter;
    use std::time::Duration;
    use tokio::time;

    /// A user's actor, with the channel it is in - to be kept while the
    /// actor is used: the actor's runtime holds it weakly.
    async fn user_actor() -> (UserActor, Arc<ChannelLoop>) {
        let runtime = test_runtime();
        let session_id = SessionId::new();
        let channel = make_channel(session_id, "main");
        let user = User {
            id: UserId::new(),
            username: Name::new("alice").unwrap(),
            email: String::new(),
            roles: Vec::new(),
            deleted: None,
            revision: Default::default(),
        };
        let channel_runtime = ChannelRuntime::outside_a_session(
            Arc::clone(&runtime),
            bare_session(session_id),
            channel,
        );
        let channel_loop = ChannelLoop::new(channel_runtime.clone()).await.unwrap();
        let actor_runtime = channel_runtime.new_actor_runtime(ActorId::User(user.id));
        runtime.entity_stores().users.create(&SecurityContext::system(), &user).await.unwrap();
        let actor = UserActor::new(
            runtime.controller_manager().user_controller(user.id).await.unwrap(),
            actor_runtime,
        )
        .await
        .unwrap();
        (actor, channel_loop)
    }

    fn envelope_from(sender: ActorId) -> Envelope {
        let context =
            EventContext::new(Default::default(), Default::default(), sender, "agent".to_string());
        Envelope::new(
            SecurityContext::default(),
            context,
            ActorEvent::ResponseStarted { turn_id: TurnId::new() },
        )
    }

    #[tokio::test]
    async fn send_message_asks_for_a_wakeup_and_returns_the_span_of_all_its_envelopes() {
        let (actor, _channel) = user_actor().await;

        let span_id =
            actor.send_message(Some("hello".to_string()), None, Vec::new()).await.unwrap();

        let mut rx = actor.user_rx.lock().await;
        let wakeup = rx.recv().await.expect("WakeupRequested");
        let complete = rx.recv().await.expect("CompleteMessageGenerated");
        assert!(
            matches!(wakeup.event, ActorEvent::WakeupRequested),
            "the wakeup first, so that the agents are awake when the message comes"
        );
        assert!(
            matches!(&complete.event, ActorEvent::CompleteMessageGenerated(m) if m.turn_id.is_none()),
            "said outside of any turn, answered by itself (issue 258)"
        );
        assert!(rx.try_recv().is_err(), "and nothing else: there is no turn to end");
        for envelope in [&wakeup, &complete] {
            assert_eq!(envelope.context.span_id, span_id, "one user action, one span");
            assert_eq!(envelope.context.sender_id, actor.id());
            assert_eq!(envelope.context.channel_id, actor.channel_id);
        }
        assert_ne!(wakeup.context.event_id, complete.context.event_id, "two events, two event ids");
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

        actor.send_message(Some("hello".to_string()), None, Vec::new()).await.unwrap();
        for _ in ["WakeupRequested", "CompleteMessageGenerated"] {
            channel_inbox.recv().await.expect("forwarded");
        }
        let shown: Vec<&str> =
            iter::from_fn(|| following.try_recv().ok()).map(|e| e.event.kind()).collect();
        assert_eq!(
            shown,
            ["CompleteMessageGenerated"],
            "what it said, as it forwarded it, not the request to wake the channel (issue 220)"
        );

        actor
            .send_approval(ApprovalResponse::new(Default::default(), Default::default()), true)
            .await
            .unwrap();
        channel_inbox.recv().await.expect("forwarded");
        assert_eq!(
            following.try_recv().map(|e| e.event.kind()).ok(),
            Some("ApprovalGranted"),
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
        assert!(
            ran.is_ok() && following.shown.try_recv().is_err(),
            "and nothing more: it does not run"
        );
    }

    #[tokio::test]
    async fn a_user_actor_whose_channel_is_gone_ends_without_an_error() {
        use crate::transport::{InMemoryEventReceiver, InMemoryEventSender};
        let (actor, _channel) = user_actor().await;
        let (_inbox, rx) = mpsc::channel(1);
        let (output, channel_inbox) = mpsc::channel(1);
        drop(channel_inbox);

        actor.send_message(Some("hello".to_string()), None, Vec::new()).await.unwrap();
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

    /// Where what the actor sends goes when nothing is to come of it.
    fn nowhere() -> InMemoryEventSender {
        InMemoryEventSender::new(mpsc::channel(1).0)
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

    #[tokio::test]
    async fn every_subscriber_receives_what_is_routed_and_a_lagging_one_loses_the_oldest() {
        let (actor, _channel) = user_actor().await;
        let agent = ActorId::Agent(Default::default());
        let (mut first, mut second) = (actor.watch(), actor.watch());

        assert!(!actor.handle_actor_envelope(Some(envelope_from(agent)), &nowhere()).await);
        assert!(first.try_recv().is_ok() && second.try_recv().is_ok(), "two tabs, both are told");

        for _ in 0..(OUTBOX_CAPACITY + 10) {
            assert!(!actor.handle_actor_envelope(Some(envelope_from(agent)), &nowhere()).await);
        }
        assert!(
            matches!(first.try_recv(), Err(broadcast::error::TryRecvError::Lagged(10))),
            "the oldest are lost, and it learns how many"
        );
        let mut received = 0;
        while first.try_recv().is_ok() {
            received += 1;
        }
        assert_eq!(received, OUTBOX_CAPACITY, "the newest are kept");
    }
}
