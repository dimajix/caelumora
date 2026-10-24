//! The run loop of a channel: starting and stopping its actors, admitting, routing, recording and
//! appending to the history what they send.

use super::inbox::{Behind, Inbox, InboxReceiver, CAPACITY};
use super::{stopped, ActorChange, ChannelActor, ChannelLoop};
use crate::actor::Answering;
use crate::logging::envelope_span;
use crate::moderator::Routing;
use caelumora_model::chain::Position;
use caelumora_model::history::{Audience, Heard, Reader, Recorded};
use crate::transport::{InMemoryEventSender, OriginEventSender};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use caelumora_model::channel::ChannelState;
use caelumora_model::store::StoreResult;
use caelumora_model::actor::{ActorEvent, ActorId, Envelope, Rejection, RetryTarget};
use caelumora_model::confidentiality::{Classification, ConfidentialityQuestions};
use caelumora_model::message::Message;
use caelumora_model::security::{ActionType, Enforcement, Reclassified, SecurityContext};
use caelumora_model::value::name::Name;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use tokio::sync::mpsc;
use tokio::sync::mpsc::Sender;
use tokio::task::{AbortHandle, JoinSet};
use tokio_util::task::TaskTracker;
use tracing::{debug, error, info, trace, warn, Instrument};
use std::iter;

/// The task of a running actor and its inbox. Owned by `run`, which keeps
/// them apart from `ChannelLoop::actors`: that table says who takes part and
/// is read from anywhere; this one is `run`'s, whose `JoinSet` owns the
/// tasks - so that they end with the channel's own task - and which starts and
/// stops them as `ActorChange`s tell it (issue 91).
struct RunningActor {
    id: ActorId,
    abort_handle: AbortHandle,
    /// What to wait on for the task to end (`ChannelActor::task`).
    task: TaskTracker,
    /// Its inbox - none once it was told to stop: nothing is delivered to
    /// an actor that is leaving, though what it sends is still taken.
    inbox: Option<Inbox>,
    /// On whose behalf it was removed, once it was told to stop (issue
    /// 262), what it was called as it left, and the position from which on
    /// nothing was delivered to it (issue 417): its task still runs - the
    /// step it was in, its farewell - and `run` announces the removal in
    /// this name once the task has ended.
    leaving: Option<(SecurityContext, Name, Position)>,
}

impl ChannelLoop {
    pub async fn run(&self) -> Result<()> {
        let _guard = self.run_lock.try_lock()
            .map_err(|_| anyhow!("Already running"))?;

        info!("Channel {}/{} is starting", self.channel.name(), self.id);
        let mut changes_rx = self.changes_rx.lock().await;
        let (event_tx, mut event_rx) = mpsc::channel(10);
        let mut join_set = JoinSet::new();
        let mut running = HashMap::new();
        // The actors taken in before it ran - as the channel was built, and
        // the founders of a channel just created (issue 420) - are started,
        // and who joined announced, before anything is routed: what one of
        // them says must come after the joins of all who hear it.
        while let Ok(change) = changes_rx.try_recv() {
            self.apply_change(change, &mut running, &event_tx, &mut join_set).await;
        }

        loop {
            tokio::select! {
                // Events before changes and ended tasks (issues 171, 262):
                // what a removed actor sent before it ended is routed before
                // its removal is announced, and before an actor that joins
                // under its id starts.
                biased;
                maybe_event = event_rx.recv() => {
                    if self.handle_actor_event(maybe_event, &running).await {
                        break;
                    }
                }
                Some(change) = changes_rx.recv() => {
                    self.apply_change(change, &mut running, &event_tx, &mut join_set).await;
                }
                // Finished tasks are dropped as they end - a removed actor's
                // with its removal announced (issue 262); once the last one
                // is gone, the channel stops - the sender kept for actors
                // joining later keeps the event queue from closing by itself.
                Some(ended) = join_set.join_next_with_id(), if !join_set.is_empty() => {
                    let task = match &ended {
                        Ok((task, _)) => *task,
                        Err(e) => e.id(),
                    };
                    let ended = running.iter().find(|(_, actor)| actor.abort_handle.id() == task).map(|(id, _)| *id);
                    if let Some(RunningActor { id, leaving: Some((remover, name, until)), .. }) = ended.and_then(|id| running.remove(&id)) {
                        self.announce_removal(id, name, remover, until, &running).await;
                    }
                    if join_set.is_empty() {
                        break;
                    }
                }
            }
        }

        info!("Channel {}/{} stopped", self.channel.name(), self.id);
        Ok(())
    }

    /// Starts an actor taken in, as the channel was built or as someone
    /// joined, or stops one that left (issue 46).
    async fn apply_change(&self, change: ActorChange, running: &mut HashMap<ActorId, RunningActor>, event_tx: &Sender<Envelope>, join_set: &mut JoinSet<ActorId>) {
        match change {
            ActorChange::Start(started, from, joined) => {
                // Removed again before `run` got to start it: never started,
                // so that it cannot run next to whoever joins under its id
                // after it (issue 171).
                let id = started.actor.id();
                let still_taken_in = self.actors.load().get(&id).is_some_and(|current| Arc::ptr_eq(&current.actor, &started.actor));
                // Its old actor has ended - `add_participant` waited for
                // that (`left`) - but its task's end may not have been seen
                // yet: the removal is announced before anything of the new
                // actor (issue 262).
                if running.get(&id).is_some_and(|old| old.leaving.is_some()) {
                    if let Some(RunningActor { leaving: Some((remover, name, until)), .. }) = running.remove(&id) {
                        self.announce_removal(id, name, remover, until, running).await;
                    }
                }
                // Who joined is announced before the newcomer runs, so that
                // its join is recorded - and with it where its view of the
                // history begins - before it reads (issue 223); also if it
                // was removed again before it got to run: it was added, and
                // its removal follows. Right before it, the part of the
                // history it was given, to it alone and in the system's
                // name, if it was given anything before its join (issue 417).
                // An actor that starts again with its channel joins nothing.
                let end = self.state.lock().await.history.end();
                if let Some(adder) = joined.clone() {
                    if let Some(from) = from.filter(|from| *from < end) {
                        let shared = ActorEvent::HistoryShared { participant: id, from, by: adder.caller_id, by_name: self.name_of(adder.caller_id).await };
                        self.announce(shared, SecurityContext::system(), running).await;
                    }
                    let name = started.actor.name().to_string();
                    self.announce(ActorEvent::ParticipantJoined { participant: id, name }, adder, running).await;
                }
                if still_taken_in {
                    let heard = Self::opening(started.actor.heard().await, joined.is_some().then(|| from.unwrap_or(end)), end);
                    let handle = self.spawn_actor(&started, heard, event_tx.clone(), join_set).await;
                    running.insert(handle.id, handle);
                }
            }
            ActorChange::Leave(id, name, remover) => {
                // Dropping its inbox closes it; told to leave (`Actor::leave`),
                // the actor ends its task itself once its state is stored
                // consistent. Only if it does not - a tool that hangs - is it
                // aborted. It keeps running until then: what it sends is
                // taken (`handle_actor_event`), and the removal is announced
                // once its task has ended (issue 262). An actor never started
                // has nothing to say: announced at once.
                // Nothing is delivered to it from here on (issue 417).
                let until = self.state.lock().await.history.end();
                match running.get_mut(&id) {
                    Some(handle) => {
                        handle.inbox = None;
                        handle.leaving = Some((remover, name, until));
                        let (abort_handle, task) = (handle.abort_handle.clone(), handle.task.clone());
                        let (channel, actor) = (self.id, id);
                        tokio::spawn(async move {
                            if !stopped(&[task]).await {
                                warn!("Actor {} did not stop by itself after leaving channel {}: aborted", actor, channel);
                                abort_handle.abort();
                            }
                        });
                        info!("Actor {} is stopping: it left channel {}", id, self.id);
                    }
                    None => self.announce_removal(id, name, remover, until, running).await,
                }
            }
        }
    }

    /// Announces that `participant`, called `name`, left, in the name of
    /// `remover`'s caller (`ParticipantLeft`, issue 162) - between the
    /// events `run` routes, once the participant's task has ended: what it
    /// sent before is in the channel, nothing of it follows (issue 262).
    /// Nothing was delivered to it from the position `until` on (issue 417).
    async fn announce_removal(&self, participant: ActorId, name: Name, remover: SecurityContext, until: Position, running: &HashMap<ActorId, RunningActor>) {
        let removed = ActorEvent::ParticipantLeft { participant, name: name.to_string(), until };
        self.announce(removed, remover, running).await;
    }

    /// What `actor` is called, as the channel names whoever it announces
    /// something in the name of; its id if it cannot be named.
    async fn name_of(&self, actor: ActorId) -> String {
        self.controllers.actor_name(actor).await.unwrap_or_else(|e| {
            warn!("Cannot name {} for channel {}: {}", actor, self.id, e);
            actor.to_string()
        })
    }

    /// Announces `event`, a change of who takes part (issue 408), in the
    /// channel in the name of `by`'s caller: the channel's own word, which
    /// enters it without `admission` - that lets no actor say it -, is
    /// recorded in its history and told to everybody.
    async fn announce(&self, event: ActorEvent, by: SecurityContext, running: &HashMap<ActorId, RunningActor>) {
        let name = self.name_of(by.caller_id).await;
        let envelope = Answering::in_channel(self.session_id, self.id, name, by).envelope(event);
        debug!("Announcing {}", envelope.describe());
        self.enter(envelope, running).await;
    }

    async fn handle_actor_event(&self, maybe_event: Option<Envelope>, running: &HashMap<ActorId, RunningActor>) -> bool {
        if let Some(event) = maybe_event {
            // An actor that was removed still finishes the step it was in
            // and ends its turn: what it sends until its task has ended
            // enters the channel as its own (issue 262); what arrives after
            // that - its task aborted - does not (issue 162). Its sender is
            // the actor's own: its `OriginEventSender` passes on nothing
            // else (issue 228).
            if !running.contains_key(&event.context.sender_id) {
                debug!("Dropped {}: its sender left channel {}", event.describe(), self.id);
                return false;
            }
            let span = envelope_span(&event.context, event.event.kind());
            self.process_envelope(event, running).instrument(span).await;
            false
        } else {
            true
        }
    }

    async fn process_envelope(&self, event: Envelope, running: &HashMap<ActorId, RunningActor>) {
        match event.event {
            // One per streamed chunk: far finer than per message.
            ActorEvent::PartialMessageGenerated(_) | ActorEvent::IntermediatePartialGenerated(_) => {
                trace!("Received {}", event.describe())
            }
            _ => debug!("Received {}", event.describe()),
        }
        // The full envelope carries message content: trace level only.
        trace!("Envelope: {:?}", event);
        if let Err(reason) = self.admission(&event).await {
            self.reject(&event, reason, running).await;
            return;
        }
        self.enter(event, running).await;
    }

    /// Lets `event` enter the channel, admitted or the channel's own: its
    /// mentions resolved, appended to the history and routed.
    /// What its sender's owner alone sees is recorded with it, and given to
    /// nobody else (issue 417): the moderator decides on what everybody may
    /// see of it, and that is what is delivered; of an event with nothing
    /// left to show, nothing is, and its entry went to nobody.
    ///
    /// An answer under way asked for again (issue 235) has its turn ended
    /// first, so that the agent finds it given up as it takes the request
    /// in, and only if it is still the turn under way. A turn given up
    /// already is asked for as it is, if it is the agent's last (issue 259).
    /// Otherwise there is nothing to answer again, and the request is
    /// dropped, unrecorded: its sender asked before, its users see the
    /// answer end, and what is not delivered is not recorded, so that the
    /// history never delivers it either (issue 223).
    async fn enter(&self, event: Envelope, running: &HashMap<ActorId, RunningActor>) {
        if let ActorEvent::RetryRequested { target: RetryTarget::Turn(turn), author } = &event.event {
            let interrupted = self.actor(*author).is_some_and(|actor| actor.retry_turn(*turn));
            if !interrupted && !self.given_up_last(*author, *turn).await {
                warn!(to = %author, "Dropped {}: turn {} is neither under way nor the last given up", event.describe(), turn);
                return;
            }
        }
        let event = self.resolve_mentions(event);
        let shown = event.published();
        let routing = match &shown {
            Some(shown) => Some(self.moderator.route(shown).await),
            None => None,
        };
        let audience = routing.as_ref().map_or_else(|| Audience::Only(Vec::new()), |routing| routing.audience.clone());
        let position = self.append_history(&event, audience).await;
        if let Some(shown) = shown {
            self.route_event(running, shown, routing, position).await;
        }
    }

    /// Where the inbox of an actor opens (issue 223) in a history that ended
    /// at `end` before its join was announced: where it stands as it kept it
    /// (`kept`, `Actor::heard`), or, as it joins, where its view begins
    /// (`joins_at`) if that lies after - what it heard before is in its
    /// transcript already (issue 387). An actor that keeps nothing opens at
    /// `end`, and so reads its own join alone.
    fn opening(kept: Option<Heard>, joins_at: Option<Position>, end: Position) -> Heard {
        match (kept, joins_at) {
            (Some(kept), Some(view)) if kept.next() < view => Heard::at(view),
            (Some(kept), _) => kept,
            (None, _) => Heard::at(end),
        }
    }

    /// Starts the task of `channel_actor` with its inbox, opened at `heard`
    /// (issue 223): behind if that lies before the history's end, so that
    /// the actor reads on from there before what is delivered from now on.
    async fn spawn_actor(&self, channel_actor: &ChannelActor, heard: Heard, actor_output: Sender<Envelope>, join_set: &mut JoinSet<ActorId>) -> RunningActor {
        let ChannelActor { actor, task, .. } = channel_actor.clone();
        let id = actor.id();
        let end = self.state.lock().await.history.end();
        let behind: Weak<dyn Behind> = match self.runtime.channel_loop() {
            Some(channel) => {
                let channel: Arc<dyn Behind> = channel;
                Arc::downgrade(&channel)
            }
            None => Weak::<ChannelLoop>::new(),
        };
        let (inbox, actor_rx) = Inbox::open(id, actor.event_filter(), heard, end, behind);
        let actor_rx: Box<InboxReceiver> = Box::new(actor_rx);
        // It sends only in its own name (issue 228).
        let actor_tx = Box::new(OriginEventSender::new(id, Box::new(InMemoryEventSender::new(actor_output))));

        // Tracked inside the `JoinSet`'s task: a task that is aborted counts
        // as ended, too.
        let abort_handle = join_set.spawn(task.track_future(async move {
            let result = actor.run(actor_rx, actor_tx).await;
            if let Err(e) = result {
                error!("Actor {} stopped with an error: {:#}", actor.id(), e)
            }
            id
        }));

        RunningActor { id, abort_handle, task, inbox: Some(inbox), leaving: None }
    }

    /// Stamps `envelope` as the channel delivers it, live or read from the
    /// history (issue 223): in the channel and its session, one step deeper
    /// in its span than what it was delivered of.
    fn stamp(&self, envelope: &mut Envelope) {
        envelope.context.channel_id = self.id;
        envelope.context.session_id = self.session_id;
        envelope.context.span_depth += 1;
    }

    /// The security context of `sender`, whose envelope with the initiator
    /// `initiator` is read from the history (issue 223): its actor's as it
    /// is now; one without rights if it takes no part any more, or is none
    /// - the channel, the system. The initiator is the one recorded.
    fn security_context_of(&self, sender: ActorId, initiator: ActorId) -> SecurityContext {
        let now = match self.actor(sender) {
            Some(actor) => actor.security_context(),
            None => SecurityContext::new(ActorId::None, self.session_id, self.id),
        };
        SecurityContext { initiator_id: initiator, session_id: self.session_id, channel_id: self.id, ..now }
    }

    /// Nothing if `event` may enter the channel; why not otherwise (issue
    /// 304) - the channel's one decision of what enters it, made here, in
    /// order, against the channel as it is when the event is taken in, so
    /// that nothing changed between a check and the entry. Actors ask
    /// nothing for what they send; who sent what is not let in is told
    /// (`MessageRejected`). Whether whoever sends it may act so is asked
    /// before the flow of what it carries. Decided:
    ///
    /// - a message, by the actor that is acting - only who acts puts
    ///   something into the channel -, each part of it, the message, its
    ///   attachments and its tool results, as it is classified
    ///   (`ConfidentialityPolicy::may_publish`, issue 133): its sender
    ///   misclassified what does not flow in; a tool's result carries no
    ///   media yet (issue 282, step 5);
    /// - a change of a message (issue 234): the message is in its history,
    ///   its author - the sender the history names - may change it, and it
    ///   can be changed so as it stands (`Message::modify`,
    ///   `Message::revoke`); a reclassification (issue 239) by its author or
    ///   the channel's owner (`Reclassify`), never above the channel;
    /// - a request to answer again (`RetryMessage`): of a message, not
    ///   taken back, or of a turn, of an agent taking part (issue 362);
    /// - an answer to an agent's questions (`AnswerQuestion`, issue 165) -
    ///   whether its sender was asked is the agent's to check;
    /// - a user's answer to an approval request, by whoever may approve the
    ///   calls of the agent it names (`ApproveToolCall`, issues 14, 312) -
    ///   whether that agent asked is the approver's to check; a decision
    ///   comes from an approver alone;
    /// - a command for an agent (issue 269) comes from a user and names an
    ///   agent taking part - whether the issuer may ask for it, its actor
    ///   asked of the agent's commands, and the agent asks again; a nudge
    ///   (issue 278) comes from a user and names someone else taking part;
    /// - what the channel records of a turn besides its messages - where a
    ///   step of its plan began, a command or an activity it carried out,
    ///   how it ended - and the start of an activity come from an agent in
    ///   its own name (issue 284): nobody else has turns; so does what an
    ///   agent was given to read and a compaction of its context (issue
    ///   417), what it was given classified so that it may flow in;
    /// - who joined or left is the channel's own word (`announce`, issue
    ///   408), never an actor's;
    /// - last, once everything else is admitted, a message said or changed
    ///   claims its attachments (`AttachmentManager::claim`, issue 282): it
    ///   carries what it carried before or what the actor acting uploaded
    ///   for the channel, never what lies elsewhere nor another's upload. A
    ///   message streamed, or repeated as its turn ends, claims nothing - it
    ///   is not what the history keeps.
    async fn admission(&self, event: &Envelope) -> Result<(), Rejection> {
        let context = &event.security;
        let messages = event.event.messages();
        if !messages.is_empty() && context.caller_id != event.context.sender_id {
            return Err(Rejection::Invalid);
        }
        let classification = self.classification();
        let channel = self.security_attributes();
        let forbidden = |_| Rejection::Forbidden;
        // The message said or changed, with what it carried before: what claims attachments.
        let mut said = None;
        match &event.event {
            ActorEvent::CompleteMessageGenerated(message) | ActorEvent::IntermediateMessageGenerated(message) => {
                said = Some((message, Vec::new()));
                Ok(())
            }
            ActorEvent::MessageModified(modified) => {
                let mut message = self.message(modified.id).await.ok_or(Rejection::MessageNotFound)?;
                self.access_policy.require(context, ActionType::ModifyMessage { author: message.sender_id, channel: &channel }, forbidden).await?;
                let kept = message.attachments.clone();
                message.modify(modified).then_some(()).ok_or(Rejection::MessageUnchangeable)?;
                said = Some((modified, kept));
                Ok(())
            }
            ActorEvent::MessageRevoked { message, at } => {
                let mut message = self.message(*message).await.ok_or(Rejection::MessageNotFound)?;
                self.access_policy.require(context, ActionType::RevokeMessage { author: message.sender_id, channel: &channel }, forbidden).await?;
                message.revoke(*at).then_some(()).ok_or(Rejection::MessageUnchangeable)
            }
            ActorEvent::MessageReclassified { message, classification: new } => {
                let mut said = self.message(*message).await.ok_or(Rejection::MessageNotFound)?;
                let target = Reclassified::Message { author: said.sender_id, attributes: &channel };
                self.access_policy.require(context, ActionType::Reclassify(target), forbidden).await?;
                self.confidentiality.require_publish(context, new, &classification, || Rejection::ClassificationRefused)?;
                said.reclassify(new).then_some(()).ok_or(Rejection::MessageUnchangeable)
            }
            ActorEvent::RetryRequested { target: RetryTarget::Message(message), author } => {
                let message = self.message(*message).await.ok_or(Rejection::MessageNotFound)?;
                self.access_policy.require(context, ActionType::RetryMessage(&channel), forbidden).await?;
                if message.revoked.is_some() {
                    return Err(Rejection::MessageUnchangeable);
                }
                (message.sender_id == *author && self.agent_actor(*author).is_some()).then_some(()).ok_or(Rejection::Invalid)
            }
            // Whether the turn is still under way is decided as the request
            // is delivered, where the turn is ended (`route_event_impl`).
            ActorEvent::RetryRequested { target: RetryTarget::Turn(_), author } => {
                self.access_policy.require(context, ActionType::RetryMessage(&channel), forbidden).await?;
                self.agent_actor(*author).is_some().then_some(()).ok_or(Rejection::Invalid)
            }
            ActorEvent::QuestionAnswered { .. } | ActorEvent::QuestionDeclined { .. } => {
                self.access_policy.require(context, ActionType::AnswerQuestion(&channel), forbidden).await
            }
            ActorEvent::ApprovalGranted(response) | ActorEvent::ApprovalDenied(response) if event.context.sender_id.as_user_id().is_some() => {
                // A person's answer says their part alone (issue 419): who
                // decided is the approver's to say, from their answers.
                if !matches!(response.decisions.as_slice(), [decision] if decision.by == event.context.sender_id) {
                    return Err(Rejection::Invalid);
                }
                let agent = self.controllers.agent_attributes(response.agent_id).await.map_err(|_| Rejection::Invalid)?;
                self.access_policy.require(context, ActionType::ApproveToolCall { agent: &agent, channel: &channel }, forbidden).await
            }
            ActorEvent::ApprovalGranted(_) | ActorEvent::ApprovalDenied(_) | ActorEvent::ApprovalTimedOut(_) => {
                matches!(event.context.sender_id, ActorId::Approver(_)).then_some(()).ok_or(Rejection::Invalid)
            }
            ActorEvent::CommandRequested { agent, .. } => {
                (event.context.sender_id.as_user_id().is_some() && self.agent_actor(*agent).is_some())
                    .then_some(()).ok_or(Rejection::Invalid)
            }
            // It asks nothing of the policy beyond what a message does - an
            // `@`-mention asks the same.
            ActorEvent::NudgeRequested { participant } => {
                (event.context.sender_id.as_user_id().is_some() && *participant != event.context.sender_id && self.actor(*participant).is_some())
                    .then_some(()).ok_or(Rejection::Invalid)
            }
            ActorEvent::MessageGiven { message } => {
                let sender = event.context.sender_id;
                (sender.as_agent_id().is_some() && context.caller_id == sender).then_some(()).ok_or(Rejection::Invalid)?;
                self.confidentiality.require_publish(context, &message.classification, &classification, || Rejection::ClassificationRefused)
            }
            ActorEvent::PlanStepStarted { .. }
            | ActorEvent::ContextCompacted { .. }
            | ActorEvent::CommandExecuted { turn_id: Some(_), .. }
            | ActorEvent::ActivityStarted { .. }
            | ActorEvent::ActivityFinished { .. }
            | ActorEvent::TurnFinished { .. }
            | ActorEvent::TurnGivenUp { .. } => {
                let sender = event.context.sender_id;
                (sender.as_agent_id().is_some() && context.caller_id == sender).then_some(()).ok_or(Rejection::Invalid)
            }
            event if event.is_announcement() => Err(Rejection::Invalid),
            _ => Ok(()),
        }?;
        for message in messages {
            self.require_parts(context, message, &classification)?;
        }
        match said {
            Some((message, kept)) => self.attachments.claim(context.caller_id, self.id, &message.attachments, &kept).then_some(()).ok_or(Rejection::Invalid),
            None => Ok(()),
        }
    }

    /// Nothing if every part of `message` - the message, its attachments and its tool results - may
    /// enter the channel, classified `classification`, in `context` (issue 133); why not otherwise.
    /// What a tool gave or showed its model is admitted from step 5 of issue 282 on.
    fn require_parts(&self, context: &SecurityContext, message: &Message, classification: &Classification) -> Result<(), Rejection> {
        if message.tool_results.iter().any(|result| !result.media.is_empty() || !result.shown.is_empty()) {
            return Err(Rejection::Invalid);
        }
        let attachments = message.attachments.iter().map(|content| &content.classification);
        let parts = iter::once(&message.classification).chain(attachments).chain(message.tool_results.iter().map(|r| &r.classification));
        for part in parts {
            self.confidentiality.require_publish(context, part, classification, || Rejection::ClassificationRefused)?;
        }
        Ok(())
    }

    /// Tells the sender of `event`, which the channel did not let in, why
    /// (`MessageRejected`, issue 304) - it alone, in the channel's name; not
    /// one that left, whose inbox is closed. The notice is recorded, to the
    /// sender alone, so that it is shown after a reload (issue 417); what
    /// was rejected entered nothing and is not.
    async fn reject(&self, event: &Envelope, reason: Rejection, running: &HashMap<ActorId, RunningActor>) {
        warn!("Dropped {}: {}", event.describe(), reason);
        let sender = event.context.sender_id;
        let Some(inbox) = running.get(&sender).and_then(|actor| actor.inbox.as_ref()) else { return };
        let rejected = ActorEvent::MessageRejected { rejected: event.event.kind().to_string(), message: event.event.subject_message(), reason };
        let mut envelope = Answering::in_channel(self.session_id, self.id, ActorId::System.to_string(), SecurityContext::system()).replying_to(&event.context).envelope(rejected);
        envelope.context.position = self.append_history(&envelope, Audience::Only(vec![sender])).await;
        inbox.deliver(envelope);
    }

    /// Resolves the `@`-mentions in every complete message carried by `event`
    /// and stores them in `Message::mentions`. Done here, once, as the message
    /// enters the channel, so that every frontend, the history and the
    /// moderator all see the same resolution (issue 29).
    fn resolve_mentions(&self, mut event: Envelope) -> Envelope {
        let resolve = |m: &mut Message| {
            if let Some(content) = &m.content {
                m.mentions = self.mentions_in(content);
            }
        };
        match &mut event.event {
            ActorEvent::CompleteMessageGenerated(m) | ActorEvent::IntermediateMessageGenerated(m) | ActorEvent::MessageModified(m) => resolve(m),
            ActorEvent::TurnFinished { messages: msgs, .. } => msgs.iter_mut().for_each(resolve),
            _ => {}
        }
        event
    }

    async fn route_event(&self, running: &HashMap<ActorId, RunningActor>, event: Envelope, routing: Option<Routing>, position: Option<Position>) {
        // Asked for by an actor - a user's actor asks with every message it
        // sends (issue 90); the channel does not decide by itself.
        if matches!(event.event, ActorEvent::WakeupRequested) {
            self.wake_for_delivery();
        }
        // A nudge is deliberate, as `wakeup_actor` is (issue 278): the
        // channel is quiet no longer, so that the request it becomes is
        // delivered, and whom it asks is woken with it (`route_event_impl`).
        if matches!(event.event, ActorEvent::NudgeRequested { .. }) {
            self.quiet.store(false, Ordering::SeqCst);
        }

        if let Some(routing) = routing {
            self.route_event_impl(running, &event, routing, position).await;
        }
        #[cfg(any(test, feature = "test-support"))]
        let _ = self.observed.send(event);
    }

    async fn route_event_impl(&self, running: &HashMap<ActorId, RunningActor>, event: &Envelope, routing: Routing, position: Option<Position>) {
        let quiet = self.quiet.load(Ordering::SeqCst);
        let decision = self.deliveries(event, routing, position, quiet).await;

        // Whom the moderator asks to respond is woken first, before anything
        // of the decision is delivered: a sleeping agent ignores a
        // `ResponseRequested`, and one that took in the message still asleep
        // would ignore the request that follows it. Since the policies never
        // choose a sleeping agent (`actor_candidates`), only the agents a
        // message mentions are woken this way (issue 45). A quiet channel
        // asks nobody, and so wakes nobody.
        if !quiet {
            let actors = self.actors.load();
            let asked = decision.iter()
                .filter(|(target_id, route)| *target_id != event.context.sender_id && matches!(route.event, ActorEvent::ResponseRequested { .. } | ActorEvent::RetryRequested { .. }));
            for (target_id, _) in asked {
                if let Some(ChannelActor { actor, .. }) = actors.get(target_id) {
                    actor.wakeup();
                }
            }
        }

        // First, route all messages to all agents...
        for route in decision {
            let target_id = route.0;

            // Do not propagate message to sender again - unless the
            // moderator sends something of its own in another name, e.g. a
            // `ChainCutOff` to the agent whose chain it cut off (issue 200),
            // or the channel announces something in the sender's name, which
            // it did not send itself (issue 408).
            if target_id == event.context.sender_id && route.1.context.sender_id == event.context.sender_id && !event.event.is_announcement() {
                continue;
            }
            // Not to an actor that is leaving: its inbox is closed.
            if let Some(inbox) = running.get(&target_id).and_then(|actor| actor.inbox.as_ref()) {
                debug!(to = %target_id, "Forwarding {}", route.1.describe());
                inbox.deliver(route.1);
            }
        }
    }

    /// What the channel delivers of `event`, recorded at `position`, as
    /// `routing` decided (issue 418), each envelope built here, stamped
    /// with the position of its entry (issue 417) and as everything is that
    /// the channel delivers (`stamp`, issue 223): the event as it entered,
    /// to whom it goes among the actors (`Envelope::recipients`) or whom
    /// the moderator named; each notice of the moderator's to whom it names,
    /// in the system's name; a request to respond to each it asks, in the
    /// event's contexts, with the hops it counted - unless the channel is
    /// `quiet` after a cancel: what was said still reaches everyone, so that
    /// every transcript keeps up with the history, but nobody is asked to
    /// answer it. Each notice and request is recorded, in the system's name,
    /// to whom it goes, before it is stamped and delivered: the history keeps
    /// it as it entered, to be stamped again as it is read from there. Whom
    /// none of it goes back to is `route_event_impl`'s to see to.
    async fn deliveries(&self, event: &Envelope, routing: Routing, position: Option<Position>, quiet: bool) -> Vec<(ActorId, Envelope)> {
        let Routing { audience, ask, reason, agent_hops, notices } = routing;
        let deliver_to = match audience {
            Audience::Everybody => {
                let recipients = event.recipients();
                self.actors.load().keys().copied().filter(|id| recipients.names(*id)).collect()
            }
            Audience::Only(only) => only,
        };
        let mut delivered = event.clone();
        delivered.context.position = position;
        self.stamp(&mut delivered);
        let mut deliveries: Vec<(ActorId, Envelope)> = deliver_to.into_iter().map(|id| (id, delivered.clone())).collect();
        for (recipients, notice) in notices {
            let mut told = event.forward(notice.into());
            told.context.sender_id = ActorId::System;
            told.context.sender_name = ActorId::System.fixed_name().unwrap_or_default().to_string();
            told.context.position = self.append_history(&told, Audience::Only(recipients.clone())).await;
            self.stamp(&mut told);
            deliveries.extend(recipients.into_iter().map(|id| (id, told.clone())));
        }
        if quiet {
            if !ask.is_empty() {
                debug!("Not asking for a response: the channel is quiet after a cancel");
            }
        } else if !ask.is_empty() {
            let mut request = event.forward(ActorEvent::ResponseRequested { reason });
            request.context.agent_hops = agent_hops;
            request.context.position = match Recorded::of(&request, ActorId::System, Audience::Only(ask.clone())) {
                Some(recorded) => self.record(recorded).await,
                None => None,
            };
            self.stamp(&mut request);
            deliveries.extend(ask.into_iter().map(|id| (id, request.clone())));
        }
        deliveries
    }

    /// Records `envelope` in the channel's history, if it is something the
    /// history records (`Envelope::entry`), sent by its sender to
    /// `audience` (issue 417): what is said as it arrives, a turn's steps
    /// too - nothing said is unsaid (issue 258) -, where a step of a plan
    /// begins before the step's messages (issue 263), a turn's end after
    /// them, a command an agent carried out (issue 269) and what came of
    /// what it did of its own accord (issue 284) before its turn's end, who
    /// joined or left where it happened (issue 408), and a change of a
    /// message, admitted, so that it applies (`admission`, issues 234, 304).
    /// Stored at once; what others hear of a message, as everybody may see
    /// it, is announced as news once it is stored (issue 404). Returns the
    /// position of its entry, which what is delivered of it carries; `None`
    /// if nothing was recorded.
    async fn append_history(&self, envelope: &Envelope, audience: Audience) -> Option<Position> {
        self.record(Recorded::of(envelope, envelope.context.sender_id, audience)?).await
    }

    /// Records `recorded` in the channel's history and stores it
    /// (`append_history`); returns its position, or `None` if it was not
    /// recorded - a change of a message that changes nothing.
    async fn record(&self, recorded: Recorded) -> Option<Position> {
        let said = recorded.entry.published().and_then(|entry| entry.said().cloned()).filter(Message::is_heard);
        let mut state = self.state.lock().await;
        if !state.history.enter(recorded) {
            return None;
        }
        let position = state.history.end().previous();
        match self.store_state(&mut state).await {
            Ok(()) => if let Some(message) = said {
                self.channel.announce_said(&message);
            },
            Err(e) => warn!("Cannot store state of channel {}: {}", self.id, e),
        }
        position
    }

    /// Stores `state`, the channel's, and then lets go of what its history
    /// holds before its window (`ChannelState::keep_window`, issue 379): the
    /// store has it.
    ///
    /// # Errors
    /// Whatever the `StateStore` reports; nothing is let go then.
    pub(super) async fn store_state(&self, state: &mut ChannelState) -> StoreResult<()> {
        self.state_store.put_channel_state(self.id, state).await?;
        state.keep_window();
        Ok(())
    }
}

/// What an actor is delivered of the channel's history where its inbox fell
/// short (issue 223): read as the actor, so that it is given what went to
/// it, as it went to it; without what it sent itself, as `route_event_impl`
/// delivers nothing back to its sender but what the channel announced in its
/// name; each envelope with its sender's security context as it is now, a
/// turn's end with its answer, stamped as what is delivered live.
#[async_trait]
impl Behind for ChannelLoop {
    async fn recorded_for(&self, actor: ActorId, from: Position) -> Result<(Vec<Envelope>, Position)> {
        let end = self.state.lock().await.history.end();
        let to = from.after(CAPACITY as u64).min(end);
        if from >= to {
            return Ok((Vec::new(), from));
        }
        let reader = Reader::Participant(actor);
        let mut delivered = Vec::new();
        for node in self.history_store.entries(self.id, &reader, from..to).await? {
            let context = &node.entry.context;
            let security = self.security_context_of(context.sender_id, context.initiator_id);
            let Some(mut envelope) = node.entry.envelope(node.seq, security) else { continue };
            if envelope.context.sender_id == actor && !envelope.event.is_announcement() {
                continue;
            }
            if let ActorEvent::TurnFinished { messages, .. } = &mut envelope.event {
                *messages = self.history_store.answer(self.id, &reader, node.seq).await?;
            }
            self.stamp(&mut envelope);
            delivered.push(envelope);
        }
        Ok((delivered, to))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::history::{HistoryEntry, Recipients};
    use crate::actor::Actor;
    use crate::component::ComponentImpl;
    use crate::transport::{EventSender, EventReceiver};
    use anyhow::Result;
    use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext, Rejection, RequestReason, RetryTarget};
    use caelumora_model::agent::AgentId;
    use caelumora_model::confidentiality::{Classification, Confidentiality};
    use caelumora_model::message::{Message, MessageId, MessageRole};
    use caelumora_model::security::SecurityContext;
    use caelumora_model::user::UserId;
    use caelumora_model::work::TurnId;
    use crate::testing::StandIn;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::task::JoinSet;
    use crate::channel::tests::*;
    use crate::channel::{ActorChange, ChannelError, ResolvedParticipant};
    use crate::testing::Script;
    use caelumora_model::security::AllowAllPolicy;
    use caelumora_model::tool::{ToolCall, ToolCallResult};
    use caelumora_model::turn::GivenUpReason;
    use chrono::Utc;
    use crate::moderator::{Moderator, Notice, ResponderPolicy, Routing};
    use crate::moderator::broadcast_policy::BroadcastPolicy;
    use uuid::Uuid;
    use std::mem;
    use tokio::time;
    use crate::testing::{file, shown, World};
    use caelumora_model::content::ContentRef;
    use std::slice;
    use crate::view::Viewer;
    use caelumora_model::session::ChannelId;

    /// An inbox the test reads what the channel delivers from, with nothing
    /// behind it.
    fn opened() -> (Inbox, InboxReceiver) {
        Inbox::open(ActorId::None, |_| true, Heard::default(), Position::START, Weak::<ChannelLoop>::new())
    }

    /// Routes `event` in `channel` to `running` as the channel does once it
    /// recorded it: decided by its moderator, then delivered.
    async fn routed(channel: &ChannelLoop, running: &HashMap<ActorId, RunningActor>, event: Envelope) {
        let routing = channel.moderator.route(&event).await;
        channel.route_event(running, event, Some(routing), None).await;
    }

    #[tokio::test]
    async fn a_users_own_message_is_shown_with_its_mentions_resolved() {
        let (alice, bob) = (user("alice"), user("bob"));
        let channel = channel_with(&[&alice, &bob], Arc::new(AllowAllPolicy::new())).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let mut following = actor.watch();
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

        actor.send_message(Some("@bob, look".to_string()), None, Vec::new()).await.expect("sent");

        let shown = time::timeout(Duration::from_secs(5), following.recv()).await
            .expect("shown by its actor, as it forwards it to the channel").unwrap();
        running.abort();
        let ActorEvent::CompleteMessageGenerated(message) = shown.event else { panic!("its message, got {}", shown.event.kind()) };
        assert_eq!(message.mentions, vec![ActorId::User(bob.id)], "as the channel resolves them (issue 220)");
    }

    /// Sends every event back to its sender, once as it is and once as a
    /// `ChainCutOff` of its own, in the name of the system.
    #[derive(Debug)]
    struct TellsTheSender;

    #[async_trait::async_trait]
    impl Moderator for TellsTheSender {
        async fn route(&self, event: &Envelope) -> Routing {
            let sender = event.context.sender_id;
            let notice = Notice::ChainCutOff { message: MessageId::new(), max_agent_hops: 0, not_asked: Vec::new() };
            Routing { audience: Audience::Only(vec![sender]), notices: vec![(vec![sender], notice)], ..Routing::default() }
        }
        fn add_actor(&self, _: Arc<dyn Actor>) {}
        fn remove_actor(&self, _: ActorId) {}
    }

    #[tokio::test]
    async fn the_sender_gets_back_only_what_the_moderator_sends_in_another_name() {
        let alice = user("alice");
        let bob = ActorId::Agent(AgentId::new());
        let channel = channel_moderated_by(&[&alice], Arc::new(TellsTheSender)).await;
        let (tx, mut rx) = opened();
        let abort_handle = tokio::spawn(async {}).abort_handle();
        let actors = HashMap::from([(bob, RunningActor { id: bob, abort_handle, task: TaskTracker::new(), inbox: Some(tx), leaving: None })]);
        let said = Envelope::new(
            SecurityContext::default(),
            EventContext { sender_id: bob, ..EventContext::default() },
            ActorEvent::TurnFinished { turn_id: TurnId::new(), messages: vec![Message::new(Classification::CHANNEL_DEFAULT)] },
        );

        routed(&channel, &actors, said).await;
        let mut kinds = Vec::new();
        while let Some(envelope) = rx.try_recv().await { kinds.push(envelope.event.kind()); }
        assert_eq!(kinds, ["ChainCutOff"], "its own message is not sent back, but what the system tells it is (issue 200)");
    }

    /// An actor that sends the envelopes it is given as it starts, as they
    /// are written - in whatever name, with whatever hops - and waits:
    /// what no actor of the runtime can, which builds its envelopes itself
    /// (`Answering`). For the tests of what the channel does with such
    /// envelopes, and no fixture to reuse (issue 372).
    #[derive(Debug)]
    struct Forger {
        id: ActorId,
        envelopes: Mutex<Vec<Envelope>>,
    }

    impl Forger {
        fn sending(id: ActorId, envelopes: Vec<Envelope>) -> Arc<Self> {
            Arc::new(Self { id, envelopes: Mutex::new(envelopes) })
        }
    }

    #[async_trait::async_trait]
    impl Actor for Forger {
        fn id(&self) -> ActorId { self.id }
        fn name(&self) -> Name { Name::new("talker").expect("a name") }
        fn event_filter(&self) -> fn(&Envelope) -> bool { |_| true }
        async fn run(&self, mut rx: Box<dyn EventReceiver>, tx: Box<dyn EventSender>) -> Result<()> {
            let envelopes = mem::take(&mut *self.envelopes.lock().unwrap());
            for envelope in envelopes {
                let _ = tx.send(envelope).await;
            }
            while rx.recv().await.is_some() {}
            Ok(())
        }
        fn state(&self) -> ActorState { ActorState::Idle }
        fn stop(&self) {}
        fn leave(&self) {}
    }

    #[tokio::test]
    async fn a_user_following_the_channel_is_told_when_a_chain_is_cut_off() {
        use crate::moderator::policy_moderator::{PolicyModerator, DEFAULT_CONTEXT_MESSAGES};
        let alice = user("alice");
        let responders = ComponentImpl { id: "responders".to_string(), fail_policy: Default::default(), facet: Arc::new(BroadcastPolicy) as Arc<dyn ResponderPolicy> };
        let moderator = Arc::new(PolicyModerator::new(responders, 1, DEFAULT_CONTEXT_MESSAGES));
        let channel = channel_moderated_by(&[&alice], Arc::<PolicyModerator>::clone(&moderator)).await;
        // One hop into a chain between agents, as the answer to a request.
        let talker = ActorId::Agent(AgentId::new());
        let context = EventContext { sender_id: talker, agent_hops: 1, ..EventContext::default() };
        let said = Message { sender_id: talker, role: MessageRole::Assistant, content: Some("and you?".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let answer = Envelope::new(SecurityContext::default().next(talker, ActorId::None, ActorId::None), context, ActorEvent::TurnFinished { turn_id: TurnId::new(), messages: vec![said] });
        channel.adopt(Forger::sending(talker, vec![answer]), ResolvedParticipant::approver(channel.session_id, channel.id), None, None).await;
        let mut following = channel.user_actor(alice.id).unwrap().watch();
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

        let told = time::timeout(Duration::from_secs(5), async {
            loop {
                if let ActorEvent::ChainCutOff { max_agent_hops, .. } = following.recv().await.expect("the channel runs").event {
                    break max_agent_hops;
                }
            }
        }).await.expect("the web UI follows the user's actor, which is told (issues 200, 220)");
        assert_eq!(told, 1);
        running.abort();
    }

    #[tokio::test]
    async fn an_actor_cannot_speak_in_the_name_of_another() {
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let talker = ActorId::Agent(AgentId::new());
        let her = ActorId::User(alice.id);
        let said = |sender: ActorId, caller: ActorId, author: ActorId, text: &str| {
            let message = Message { sender_id: author, role: MessageRole::Assistant, content: Some(text.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
            Envelope::new(SecurityContext::default().next(caller, ActorId::None, ActorId::None), EventContext { sender_id: sender, ..EventContext::default() }, ActorEvent::CompleteMessageGenerated(message))
        };
        let envelopes = vec![
            said(her, talker, talker, "as alice's envelope"),
            said(talker, her, talker, "in alice's security context"),
            said(talker, talker, her, "as alice's message"),
            // On alice's behalf - she asked - but said by itself: as an agent answers.
            Envelope { security: SecurityContext::new(her, channel.session_id, channel.id).next(talker, her, her), ..said(talker, talker, talker, "its own") },
        ];
        channel.adopt(Forger::sending(talker, envelopes), ResolvedParticipant::approver(channel.session_id, channel.id), None, None).await;
        let mut events = channel.observe();
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

        let entered = time::timeout(Duration::from_secs(5), async {
            loop {
                if let ActorEvent::CompleteMessageGenerated(message) = events.recv().await.expect("the channel runs").event {
                    break message.content.into_iter().collect::<Vec<_>>();
                }
            }
        }).await.expect("what it said in its own name enters");
        assert_eq!(entered, ["its own"], "nothing it said in another's name entered before it");
        let history: Vec<_> = channel.history().await.iter().filter_map(|m| m.content.clone()).collect();
        assert_eq!(history, ["its own"], "nor the history");
        running.abort();
    }

    #[tokio::test]
    async fn who_joined_or_left_is_the_channel_s_word_alone() {
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let her = ActorId::User(alice.id);
        let stranger = ActorId::User(UserId::new());
        let from_her = |event| Envelope::new(SecurityContext::new(her, channel.session_id, channel.id), EventContext { sender_id: her, ..EventContext::default() }, event);
        let (to_her, mut hers) = opened();
        let actors = HashMap::from([(her, RunningActor { id: her, abort_handle: tokio::spawn(async {}).abort_handle(), task: TaskTracker::new(), inbox: Some(to_her), leaving: None })]);

        // In her own name, but no actor's to say (issue 408).
        channel.process_envelope(from_her(ActorEvent::ParticipantJoined { participant: stranger, name: "mallory".to_string() }), &actors).await;
        channel.process_envelope(from_her(ActorEvent::ParticipantLeft { participant: her, name: "alice".to_string(), until: Position::new(0) }), &actors).await;

        let recorded = channel.history_entries().await;
        assert!(matches!(recorded.as_slice(), [HistoryEntry::ParticipantJoined { .. }, HistoryEntry::MessageRejected { .. }, HistoryEntry::MessageRejected { .. }]),
            "after her own join, nothing of it is recorded, only that it was not let in (issue 417): {recorded:?}");
        let state = channel.state.lock().await;
        assert!(state.history.entries().skip(1).all(|node| node.entry.sender == ActorId::System && node.entry.recipients == Recipients::only(vec![her])), "told to her alone, in the system's name");
        drop(state);
        let mut rejected = Vec::new();
        while let Some(envelope) = hers.try_recv().await {
            if let ActorEvent::MessageRejected { rejected: kind, reason, .. } = envelope.event {
                rejected.push((kind, reason));
            }
        }
        assert_eq!(rejected, [("ParticipantJoined".to_string(), Rejection::Invalid), ("ParticipantLeft".to_string(), Rejection::Invalid)], "and she is told so");
    }

    /// Issue 419: a person's answer to an approval says their own part
    /// alone - who decided is the approver's to say -, so one that says
    /// nobody's, or someone else's too, is not let in, before anything is
    /// asked of it.
    #[tokio::test]
    async fn an_answer_to_an_approval_says_its_senders_part_alone() {
        use caelumora_model::approval::{ApprovalDecision, ApprovalResponse};
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let (her, bob) = (ActorId::User(alice.id), ActorId::User(UserId::new()));
        let from_her = |event| Envelope::new(SecurityContext::new(her, channel.session_id, channel.id), EventContext { sender_id: her, ..EventContext::default() }, event);
        let (to_her, mut hers) = opened();
        let actors = HashMap::from([(her, RunningActor { id: her, abort_handle: tokio::spawn(async {}).abort_handle(), task: TaskTracker::new(), inbox: Some(to_her), leaving: None })]);
        let part = |by: ActorId| ApprovalDecision { by, name: String::new(), by_rule: None, at: Utc::now() };
        let answer = |decisions| ApprovalResponse { decisions, ..ApprovalResponse::new(Default::default(), AgentId::new()) };

        for decisions in [Vec::new(), vec![part(bob)], vec![part(her), part(bob)]] {
            channel.process_envelope(from_her(ActorEvent::ApprovalGranted(answer(decisions))), &actors).await;
        }

        let mut rejected = Vec::new();
        while let Some(envelope) = hers.try_recv().await {
            if let ActorEvent::MessageRejected { rejected: kind, reason, .. } = envelope.event {
                rejected.push((kind, reason));
            }
        }
        assert_eq!(rejected, vec![("ApprovalGranted".to_string(), Rejection::Invalid); 3], "nobody's, someone else's, or more than hers: none let in");
    }

    #[tokio::test]
    async fn what_is_recorded_of_a_turn_comes_from_an_agent_in_its_own_name() {
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let agent = ActorId::Agent(AgentId::new());
        let her = ActorId::User(alice.id);
        let sent = |sender: ActorId, caller: ActorId, event: ActorEvent| Envelope::new(
            SecurityContext::default().next(caller, ActorId::None, ActorId::None),
            EventContext { sender_id: sender, ..EventContext::default() },
            event,
        );
        let turn_id = TurnId::new();
        let activity = "compacting its context".to_string();
        let of_a_turn = [
            ActorEvent::PlanStepStarted { turn_id, step_index: 0, total_steps: 1, description: "first".to_string() },
            ActorEvent::CommandExecuted { command: "compact".to_string(), args: String::new(), output: "done".to_string(), to: None, turn_id: Some(turn_id) },
            ActorEvent::ActivityStarted { turn_id, activity: activity.clone() },
            ActorEvent::ActivityFinished { turn_id, activity, output: "done".to_string() },
            ActorEvent::TurnFinished { turn_id, messages: Vec::new() },
            ActorEvent::TurnGivenUp { turn_id, reason: GivenUpReason::Left },
        ];
        for event in of_a_turn {
            let kind = event.kind();
            assert_eq!(channel.admission(&sent(agent, agent, event.clone())).await, Ok(()), "{kind} of an agent's turn, in its own name");
            assert_eq!(channel.admission(&sent(her, her, event.clone())).await, Err(Rejection::Invalid), "{kind}: a user has no turns");
            assert_eq!(channel.admission(&sent(agent, her, event)).await, Err(Rejection::Invalid), "{kind} in another's name");
        }
        let of_the_session = ActorEvent::CommandExecuted { command: "wakeup".to_string(), args: String::new(), output: "Woke.".to_string(), to: None, turn_id: None };
        assert_eq!(channel.admission(&sent(her, her, of_the_session)).await, Ok(()), "a command of the session is run by a user, in no turn");
    }

    #[tokio::test]
    async fn the_channel_wakes_only_whom_the_moderator_asks_to_respond() {
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let (asked, other) = (sleeper(), sleeper());
        let channel = channel_moderated_by(&[&alice], Arc::new(AskBack(asked.id()))).await;
        for actor in [&asked, &other] {
            // Who it is does not matter to routing: the channel only looks it up.
            channel.adopt(Arc::<StandIn>::clone(actor), ResolvedParticipant::approver(channel.session_id, channel.id), None, None).await;
        }
        let said = || Envelope::new(
            context.clone(),
            EventContext { sender_id: ActorId::User(alice.id), ..EventContext::default() },
            ActorEvent::TurnFinished { turn_id: TurnId::new(), messages: vec![Message::new(Classification::CHANNEL_DEFAULT)] },
        );

        routed(&channel, &HashMap::new(), said()).await;
        assert!(asked.woken(), "asked to respond, it is woken to do so");
        assert!(!other.woken(), "not addressed, it sleeps on");

        // A quiet channel asks nobody, and wakes nobody.
        asked.sleep();
        channel.cancel(&context).await.expect("permitted");
        routed(&channel, &HashMap::new(), said()).await;
        assert!(!asked.woken(), "nobody is asked in a quiet channel");
    }

    #[tokio::test]
    async fn a_channel_admits_only_what_may_flow_into_it_in_its_senders_name() {
        use caelumora_model::security::OwnershipPolicy;
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(OwnershipPolicy::new())).await;
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(UserId::new()));
        let from = |sender: ActorId, context: ActorId, classification: Classification| Envelope::new(
            SecurityContext::new(context, Default::default(), Default::default()),
            EventContext { sender_id: sender, ..EventContext::default() },
            ActorEvent::CompleteMessageGenerated(Message::new(classification)),
        );
        let internal = Classification::general(Confidentiality::Internal);
        let actors = HashMap::new();
        channel.process_envelope(from(alice_id, alice_id, internal.clone()), &actors).await;
        channel.process_envelope(from(alice_id, alice_id, Classification::general(Confidentiality::Confidential)), &actors).await;
        channel.process_envelope(from(alice_id, bob_id, internal.clone()), &actors).await;
        assert_eq!(channel.history().await.len(), 1, "neither above the channel nor in another's name");

        // A tool result is a part of its own.
        let call = ToolCall { id: None, function_name: "f".to_string(), arguments: serde_json::json!({}), visibility: Default::default(), presentation: Default::default() };
        let result = ToolCallResult::of(&call, Default::default(), "secret".to_string(), Classification::general(Confidentiality::Confidential));
        let mut with_result = from(alice_id, alice_id, internal);
        if let ActorEvent::CompleteMessageGenerated(message) = &mut with_result.event {
            message.tool_results.push(result);
        }
        channel.process_envelope(with_result, &actors).await;
        assert_eq!(channel.history().await.len(), 1, "nor with a result above the channel");
    }

    #[tokio::test]
    async fn a_change_to_a_message_enters_the_channel_only_from_its_author_as_it_stands() {
        use caelumora_model::security::OwnershipPolicy;
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(OwnershipPolicy::new())).await;
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(UserId::new()));
        let from = |sender: ActorId, event| Envelope::new(
            SecurityContext::new(sender, Default::default(), Default::default()),
            EventContext { sender_id: sender, ..EventContext::default() },
            event,
        );
        let said = Message { id: MessageId::new(), sender_id: alice_id, role: MessageRole::User, content: Some("helo".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let modified = |sender: ActorId, content: &str| Message { sender_id: sender, content: Some(content.to_string()), timestamp: Utc::now(), ..said.clone() };
        let revoked = || ActorEvent::MessageRevoked { message: said.id, at: Utc::now() };
        let actors = HashMap::new();
        channel.process_envelope(from(alice_id, ActorEvent::CompleteMessageGenerated(said.clone())), &actors).await;
        let entries = || async { channel.state.lock().await.history.all_entries().len() };

        channel.process_envelope(from(bob_id, ActorEvent::MessageModified(modified(bob_id, "bob's"))), &actors).await;
        channel.process_envelope(from(bob_id, revoked()), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::MessageModified(Message { id: MessageId::new(), ..modified(alice_id, "nothing said") })), &actors).await;
        assert_eq!(entries().await, 2, "her join and what she said: neither a change by another nor of what was never said");

        channel.process_envelope(from(alice_id, ActorEvent::MessageModified(modified(alice_id, "hello"))), &actors).await;
        let standing = channel.message(said.id).await.expect("said");
        assert_eq!((standing.content.as_deref(), standing.modified.is_some()), (Some("hello"), true), "its author modifies it");

        // Reclassified as the policy says (issue 239): here, with nobody
        // owning the channel, by its author alone.
        let reclassified = |classification| ActorEvent::MessageReclassified { message: said.id, classification };
        let above = Classification::general(Confidentiality::Confidential);
        channel.process_envelope(from(bob_id, reclassified(Classification::PUBLIC)), &actors).await;
        channel.process_envelope(from(alice_id, reclassified(above)), &actors).await;
        assert_eq!(entries().await, 3, "neither by another nor above the channel");
        channel.process_envelope(from(alice_id, reclassified(Classification::PUBLIC)), &actors).await;
        assert_eq!(channel.message(said.id).await.map(|m| m.classification), Some(Classification::PUBLIC), "its author releases it");

        channel.process_envelope(from(alice_id, revoked()), &actors).await;
        assert!(channel.message(said.id).await.is_some_and(|m| m.revoked.is_some() && m.content.is_none()), "and takes it back");
        channel.process_envelope(from(alice_id, revoked()), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::MessageModified(modified(alice_id, "again"))), &actors).await;
        channel.process_envelope(from(alice_id, reclassified(Classification::CHANNEL_DEFAULT)), &actors).await;
        assert_eq!(entries().await, 5, "nothing changes it once taken back");
        assert_eq!(channel.history().await.len(), 1, "it stays in its place");

        // Only an agent is asked to answer again, and only for its own message.
        let mut routed = channel.observe();
        channel.process_envelope(from(alice_id, ActorEvent::RetryRequested { target: RetryTarget::Message(said.id), author: alice_id }), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::RetryRequested { target: RetryTarget::Message(said.id), author: ActorId::Agent(AgentId::new()) }), &actors).await;
        // An answer under way only of an agent of the channel.
        channel.process_envelope(from(alice_id, ActorEvent::RetryRequested { target: RetryTarget::Turn(TurnId::new()), author: alice_id }), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::RetryRequested { target: RetryTarget::Turn(TurnId::new()), author: ActorId::Agent(AgentId::new()) }), &actors).await;
        // A nudge only of someone else in the channel (issue 278).
        channel.process_envelope(from(alice_id, ActorEvent::NudgeRequested { participant: alice_id }), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::NudgeRequested { participant: ActorId::Agent(AgentId::new()) }), &actors).await;
        assert!(routed.try_recv().is_err(), "neither a user's message nor one another said, nor a turn of someone who is no agent here, nor a nudge of oneself or of a stranger");
    }

    #[tokio::test]
    async fn the_channel_keeps_the_end_of_its_history_and_reads_the_rest_from_the_store() {
        use caelumora_model::history::WINDOW;
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let alice_id = ActorId::User(alice.id);
        let from = |event| Envelope::new(SecurityContext::new(alice_id, Default::default(), Default::default()), EventContext { sender_id: alice_id, ..EventContext::default() }, event);
        let said = |n: usize| Message { id: MessageId::new(), sender_id: alice_id, role: MessageRole::User, content: Some(format!("message {n}")), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let (to_alice, _alices) = opened();
        let actors = HashMap::from([(alice_id, RunningActor { id: alice_id, abort_handle: tokio::spawn(async {}).abort_handle(), task: TaskTracker::new(), inbox: Some(to_alice), leaving: None })]);
        let first = said(0);
        channel.process_envelope(from(ActorEvent::CompleteMessageGenerated(first.clone())), &actors).await;
        for n in 1..250 {
            channel.process_envelope(from(ActorEvent::CompleteMessageGenerated(said(n))), &actors).await;
        }
        let (base, end, digest) = {
            let state = channel.state.lock().await;
            (state.history.base(), state.history.end(), state.history.digest())
        };
        assert_eq!((base, end), (end.before(WINDOW), Position::new(251)), "the channel holds the end of its history (issue 379), after her join");
        assert_eq!(channel.visible_messages(alice_id).await.len(), 250, "the store all of it");

        // A message before the window is changed as any: the check reads the store.
        channel.process_envelope(from(ActorEvent::MessageModified(Message { content: Some("changed".to_string()), ..first.clone() })), &actors).await;
        assert_eq!(channel.message(first.id).await.and_then(|m| m.content).as_deref(), Some("changed"));

        let loaded = channel.state_store.get_channel_state(channel.id).await.unwrap();
        assert!(loaded.history.base() > Position::START, "loaded from where its window began");
        assert_ne!(loaded.history.digest(), digest, "with the change after it");
        assert_eq!(loaded.history.end(), Position::new(252));
    }

    #[tokio::test]
    async fn a_user_changes_only_their_own_message_and_learns_why_not() {
        use caelumora_model::security::OwnershipPolicy;
        let (alice, bob) = (user("alice"), user("bob"));
        let channel = channel_with(&[&alice, &bob], Arc::new(OwnershipPolicy::new())).await;
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(bob.id));
        let said = Message { id: MessageId::new(), sender_id: alice_id, role: MessageRole::User, content: Some("helo".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let from = |sender: ActorId, event| Envelope::new(SecurityContext::new(sender, Default::default(), Default::default()), EventContext { sender_id: sender, ..EventContext::default() }, event);
        let (to_alice, mut alices) = opened();
        let (to_bob, mut bobs) = opened();
        let running = |id: ActorId, tx| (id, RunningActor { id, abort_handle: tokio::spawn(async {}).abort_handle(), task: TaskTracker::new(), inbox: Some(tx), leaving: None });
        let actors = HashMap::from([running(alice_id, to_alice), running(bob_id, to_bob)]);
        let rejected = async |rx: &mut InboxReceiver| {
            let mut rejections = Vec::new();
            while let Some(envelope) = rx.try_recv().await {
                if let ActorEvent::MessageRejected { rejected, message, reason } = envelope.event {
                    assert_eq!(envelope.context.sender_id, ActorId::System, "in the channel's name");
                    rejections.push((rejected, message, reason));
                }
            }
            rejections
        };
        channel.process_envelope(from(alice_id, ActorEvent::CompleteMessageGenerated(said.clone())), &actors).await;

        // What the channel does not let in, it tells its sender alone, and
        // why (issue 304).
        let above = Classification::general(Confidentiality::Confidential);
        let modified = |content: &str, classification: &Classification| ActorEvent::MessageModified(Message { content: Some(content.to_string()), classification: classification.clone(), ..said.clone() });
        let reclassified = |classification: &Classification| ActorEvent::MessageReclassified { message: said.id, classification: classification.clone() };
        let revoked = |message| ActorEvent::MessageRevoked { message, at: Utc::now() };
        channel.process_envelope(from(bob_id, modified("bob's", &Classification::CHANNEL_DEFAULT)), &actors).await;
        channel.process_envelope(from(bob_id, revoked(said.id)), &actors).await;
        channel.process_envelope(from(bob_id, reclassified(&Classification::PUBLIC)), &actors).await;
        let kind = |rejected: &str, reason| (rejected.to_string(), Some(said.id), reason);
        assert_eq!(rejected(&mut bobs).await, [kind("MessageModified", Rejection::Forbidden), kind("MessageRevoked", Rejection::Forbidden), kind("MessageReclassified", Rejection::Forbidden)], "not his");
        let never = MessageId::new();
        channel.process_envelope(from(alice_id, revoked(never)), &actors).await;
        channel.process_envelope(from(alice_id, modified("hello", &above)), &actors).await;
        channel.process_envelope(from(alice_id, reclassified(&above)), &actors).await;
        let secret = Message { id: MessageId::new(), ..Message::new(above.clone()) };
        channel.process_envelope(from(alice_id, ActorEvent::CompleteMessageGenerated(secret.clone())), &actors).await;
        assert_eq!(rejected(&mut alices).await, [
            ("MessageRevoked".to_string(), Some(never), Rejection::MessageNotFound),
            kind("MessageModified", Rejection::ClassificationRefused),
            kind("MessageReclassified", Rejection::ClassificationRefused),
            ("CompleteMessageGenerated".to_string(), Some(secret.id), Rejection::ClassificationRefused),
        ], "never said, and never above the channel - the flow, not her permission");
        assert!(rejected(&mut bobs).await.is_empty(), "nor is anyone else told");
        channel.process_envelope(from(alice_id, revoked(said.id)), &actors).await;
        channel.process_envelope(from(alice_id, modified("hello", &Classification::CHANNEL_DEFAULT)), &actors).await;
        assert_eq!(rejected(&mut alices).await, [kind("MessageModified", Rejection::MessageUnchangeable)], "taken back, it stays so");

        // A request to answer again names an agent taking part, of a
        // message as of a turn (issue 362): of one that left, its sender is
        // told, rather than it being dropped unseen.
        let gone = ActorId::Agent(AgentId::new());
        let its = Message { id: MessageId::new(), sender_id: gone, role: MessageRole::Assistant, content: Some("said before it left".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        channel.process_envelope(from(gone, ActorEvent::CompleteMessageGenerated(its.clone())), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::RetryRequested { target: RetryTarget::Message(its.id), author: gone }), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::RetryRequested { target: RetryTarget::Turn(TurnId::new()), author: gone }), &actors).await;
        assert_eq!(rejected(&mut alices).await, [
            ("RetryRequested".to_string(), Some(its.id), Rejection::Invalid),
            ("RetryRequested".to_string(), None, Rejection::Invalid),
        ], "an agent that left answers nothing again");

        // Her actor looks at what it can see at once, and asks nothing of the
        // policy: that is the channel's.
        let (hers, his) = (channel.user_actor(alice.id).unwrap(), channel.user_actor(bob.id).unwrap());
        let error = |result: Result<Uuid>| result.expect_err("refused").downcast::<ChannelError>().expect("a channel's error");
        his.reclassify_message(MessageId::new(), Classification::PUBLIC).await.expect_err("never said");
        assert!(matches!(error(hers.revoke_message(MessageId::new()).await), ChannelError::MessageNotFound { .. }), "never said");
        assert!(matches!(error(hers.modify_message(said.id, Some("hello".to_string()), None, Vec::new()).await), ChannelError::MessageRevoked { .. }), "taken back");
        assert!(matches!(error(hers.reclassify_message(said.id, Classification::PUBLIC).await), ChannelError::MessageRevoked { .. }), "with nothing left to release");
        assert!(matches!(error(his.request_retry(said.id).await), ChannelError::MessageRevoked { .. }));
        assert!(matches!(error(his.request_retry(MessageId::new()).await), ChannelError::MessageNotFound { .. }));
        assert!(matches!(error(his.request_retry_of_turn(alice_id, TurnId::new()).await), ChannelError::NotAParticipant { .. }), "no agent answers here");
    }

    #[tokio::test]
    async fn a_removal_is_announced_in_the_channel_in_the_name_of_who_removed_after_the_farewell() {
        let Stuck { channel, context, agent, helper, mut events, running } = stuck_in_a_turn().await;

        channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");

        // What a frontend needs to end the turn it shows: who is gone -
        // announced once the actor's task has ended, after the end of its
        // turn, which it tells itself (issue 262); nothing of it follows.
        let (removed, farewell_first) = time::timeout(Duration::from_secs(5), async {
            let mut farewell = false;
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == ActorId::Agent(agent) && matches!(envelope.event, ActorEvent::TurnGivenUp { .. }) {
                    farewell = true;
                }
                if let ActorEvent::ParticipantLeft { participant, .. } = envelope.event {
                    break ((envelope.context.sender_id, envelope.context.sender_name, participant), farewell);
                }
            }
        }).await.expect("the removal is announced");
        assert_eq!(removed, (context.caller_id, "alice".to_string(), ActorId::Agent(agent)));
        assert!(farewell_first, "the agent's farewell precedes the announcement");
        settled(&helper, ActorState::Idle).await;
        time::sleep(Duration::from_millis(50)).await;
        while let Ok(envelope) = events.try_recv() {
            assert_ne!(envelope.context.sender_id, ActorId::Agent(agent), "nothing of it follows the announcement: {}", envelope.describe());
        }
        running.abort();
    }

    #[tokio::test]
    async fn nothing_of_a_removed_participant_enters_the_channel_once_its_task_has_ended() {
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let mut events = channel.observe();
        let gone = ActorId::Agent(AgentId::new());
        let late = Envelope::new(SecurityContext::default(), EventContext::new(channel.session_id, channel.id, gone, "helper".to_string()), ActorEvent::TurnGivenUp { turn_id: TurnId::new(), reason: GivenUpReason::Failed { message: "too late".to_string() } });

        // Its actor no longer runs, nor is it leaving: it was aborted.
        channel.handle_actor_event(Some(late), &HashMap::new()).await;

        assert!(events.try_recv().is_err(), "what a removed participant sends is dropped");
    }

    /// An actor told to stop whose task has not ended: no inbox, and who
    /// removed it.
    fn leaving_actor(id: ActorId, remover: SecurityContext) -> RunningActor {
        let name = Name::new("helper").expect("a name");
        RunningActor { id, abort_handle: tokio::spawn(async {}).abort_handle(), task: TaskTracker::new(), inbox: None, leaving: Some((remover, name, Position::START)) }
    }

    #[tokio::test]
    async fn what_a_leaving_actor_sends_enters_the_channel_until_its_task_has_ended() {
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let mut events = channel.observe();
        let going = ActorId::Agent(AgentId::new());
        let turn_id = TurnId::new();
        let farewell = Envelope::new(SecurityContext::default().next(going, ActorId::None, ActorId::None), EventContext::new(channel.session_id, channel.id, going, "helper".to_string()), ActorEvent::TurnGivenUp { turn_id, reason: GivenUpReason::Left });

        // Told to stop, its task still runs: what it says is its own (issue 262).
        let running = HashMap::from([(going, leaving_actor(going, SecurityContext::default()))]);
        channel.handle_actor_event(Some(farewell), &running).await;

        let heard = events.try_recv().expect("the farewell enters the channel");
        assert!(matches!(heard.event, ActorEvent::TurnGivenUp { turn_id: id, .. } if id == turn_id));
        assert_eq!(heard.context.sender_id, going);
    }

    #[tokio::test]
    async fn a_removal_still_pending_is_announced_before_an_actor_starts_under_the_same_id() {
        let (channel, context, agent, _) = scripted_channel(Script::Hangs).await;
        let mut events = channel.observe();
        let participant = ActorId::Agent(agent);
        let joined = channel.actors.load().get(&participant).cloned().expect("the agent takes part");

        // Its old actor's task has ended - `add_participant` waited for it -
        // but `run` has not seen that yet when the new actor's start comes.
        let (event_tx, _event_rx) = mpsc::channel(10);
        let mut join_set = JoinSet::new();
        let mut running = HashMap::from([(participant, leaving_actor(participant, context.clone()))]);
        channel.apply_change(ActorChange::Start(joined, Some(Position::START), Some(context.clone())), &mut running, &event_tx, &mut join_set).await;

        let announced = events.try_recv().expect("the removal is announced");
        assert!(matches!(announced.event, ActorEvent::ParticipantLeft { participant: p, .. } if p == participant), "{}", announced.describe());
        assert_eq!(announced.context.sender_id, context.caller_id, "in the remover's name");
        let shared = events.try_recv().expect("what it is given of the history is told after it");
        assert!(matches!(shared.event, ActorEvent::HistoryShared { participant: p, from: Position::START, by, .. } if p == participant && by == context.caller_id), "{}", shared.describe());
        assert_eq!(shared.context.sender_id, ActorId::System, "in the system's name, for whom it names (issue 417)");
        let added = events.try_recv().expect("the join is announced after it");
        assert!(matches!(added.event, ActorEvent::ParticipantJoined { participant: p, .. } if p == participant), "{}", added.describe());
        assert!(running.get(&participant).is_some_and(|actor| actor.leaving.is_none()), "and then the new actor runs");
        join_set.abort_all();
    }

    /// What a `Listener` took in, in the order it took it in.
    #[derive(Debug, PartialEq)]
    enum TakenIn {
        Heard(&'static str, Option<Position>),
    }

    /// An actor that keeps where it stands, nothing yet, and tells what is
    /// delivered to it, with the position of its entry.
    #[derive(Debug)]
    struct Listener {
        id: ActorId,
        taken_in: mpsc::UnboundedSender<TakenIn>,
    }

    impl Listener {
        fn new(id: ActorId) -> (Arc<Self>, mpsc::UnboundedReceiver<TakenIn>) {
            let (taken_in, told) = mpsc::unbounded_channel();
            (Arc::new(Self { id, taken_in }), told)
        }
    }

    #[async_trait::async_trait]
    impl Actor for Listener {
        fn id(&self) -> ActorId { self.id }
        fn name(&self) -> Name { Name::new("listener").expect("a name") }
        fn event_filter(&self) -> fn(&Envelope) -> bool { |_| true }
        async fn heard(&self) -> Option<Heard> {
            Some(Heard::default())
        }
        async fn run(&self, mut rx: Box<dyn EventReceiver>, _tx: Box<dyn EventSender>) -> Result<()> {
            while let Some(envelope) = rx.recv().await {
                let _ = self.taken_in.send(TakenIn::Heard(envelope.event.kind(), envelope.context.position));
            }
            Ok(())
        }
        fn state(&self) -> ActorState { ActorState::Idle }
        fn stop(&self) {}
        fn leave(&self) {}
    }

    #[tokio::test]
    async fn a_newcomer_reads_what_it_was_given_then_its_join_then_what_is_said_from_there_on() {
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let said = || Envelope::new(
            context.clone(),
            EventContext { sender_id: ActorId::User(alice.id), ..EventContext::default() },
            ActorEvent::CompleteMessageGenerated(Message { id: MessageId::new(), sender_id: ActorId::User(alice.id), role: MessageRole::User, content: Some("hello".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) }),
        );
        let (event_tx, _event_rx) = mpsc::channel(10);
        let mut join_set = JoinSet::new();
        let mut running = HashMap::new();
        let start_all = async |running: &mut HashMap<ActorId, RunningActor>, join_set: &mut JoinSet<ActorId>| {
            while let Ok(change) = channel.changes_rx.lock().await.try_recv() {
                channel.apply_change(change, running, &event_tx, join_set).await;
            }
        };
        start_all(&mut running, &mut join_set).await;
        let (newcomer, mut taken_in) = Listener::new(ActorId::User(UserId::new()));
        let from = channel.state.lock().await.history.end();

        // Taken in, but `run` has not started it yet: what is said meanwhile
        // has no inbox to go to.
        channel.adopt(Arc::<Listener>::clone(&newcomer), ResolvedParticipant::approver(channel.session_id, channel.id), Some(from), Some(context.clone())).await;
        channel.enter(said(), &running).await;
        start_all(&mut running, &mut join_set).await;
        channel.enter(said(), &running).await;

        let mut told = Vec::new();
        while told.len() < 4 {
            told.push(time::timeout(Duration::from_secs(5), taken_in.recv()).await.expect("taken in").expect("the listener runs"));
        }
        join_set.abort_all();
        assert_eq!(told, [
            TakenIn::Heard("CompleteMessageGenerated", Some(from)),
            TakenIn::Heard("HistoryShared", Some(from.next())),
            TakenIn::Heard("ParticipantJoined", Some(from.after(2))),
            TakenIn::Heard("CompleteMessageGenerated", Some(from.after(3))),
        ], "what was said before it ran, read from the history, its join, and what is said from there on: nothing twice, nothing missed (issues 423, 223)");
    }

    #[tokio::test]
    async fn a_cancelled_channel_delivers_what_is_said_but_asks_nobody_until_it_is_asked_to_wake() {
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let bob = ActorId::Agent(AgentId::new());
        let channel = channel_moderated_by(&[&alice], Arc::new(AskBack(bob))).await;
        let (tx, mut rx) = opened();
        let abort_handle = tokio::spawn(async {}).abort_handle();
        let actors = HashMap::from([(bob, RunningActor { id: bob, abort_handle, task: TaskTracker::new(), inbox: Some(tx), leaving: None })]);
        let from_alice = |event| Envelope::new(
            context.clone(),
            EventContext { sender_id: ActorId::User(alice.id), ..EventContext::default() },
            event,
        );
        let said = || from_alice(ActorEvent::TurnFinished { turn_id: TurnId::new(), messages: vec![Message::new(Classification::CHANNEL_DEFAULT)] });
        let delivered = async |rx: &mut InboxReceiver| {
            let mut kinds = Vec::new();
            while let Some(envelope) = rx.try_recv().await { kinds.push(envelope.event.kind()); }
            kinds
        };

        channel.cancel(&context).await.expect("permitted");
        routed(&channel, &actors, said()).await;
        assert_eq!(delivered(&mut rx).await, ["TurnFinished"], "what was said still arrives, but nobody is asked");

        // A message alone does not wake it: its sender asks for that, as a
        // user's actor does with every message (issue 90).
        routed(&channel, &actors, from_alice(ActorEvent::WakeupRequested)).await;
        delivered(&mut rx).await; // this test's moderator forwards the request, too
        routed(&channel, &actors, said()).await;
        assert_eq!(delivered(&mut rx).await, ["TurnFinished", "ResponseRequested"], "woken, the channel asks again");
    }

    #[tokio::test]
    async fn cancelling_one_actor_ends_its_turn_alone_and_keeps_the_channel_answering() {
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let (cancelled, other) = (sleeper(), sleeper());
        let channel = channel_moderated_by(&[&alice], Arc::new(AskBack(other.id()))).await;
        for actor in [&cancelled, &other] {
            channel.adopt(Arc::<StandIn>::clone(actor), ResolvedParticipant::approver(channel.session_id, channel.id), None, None).await;
        }

        channel.cancel_actor(&context, cancelled.id()).await.expect("permitted");
        assert!(cancelled.cancelled(), "its turn is ended");
        assert!(!other.cancelled(), "the other's goes on");

        let said = Envelope::new(
            context.clone(),
            EventContext { sender_id: ActorId::User(alice.id), ..EventContext::default() },
            ActorEvent::TurnFinished { turn_id: TurnId::new(), messages: vec![Message::new(Classification::CHANNEL_DEFAULT)] },
        );
        routed(&channel, &HashMap::new(), said).await;
        assert!(other.woken(), "the channel is not quiet: the next message is answered");

        let stranger = ActorId::Agent(AgentId::new());
        let error = channel.cancel_actor(&context, stranger).await.expect_err("not in the channel");
        assert!(matches!(error.downcast_ref::<ChannelError>(), Some(ChannelError::NotAParticipant { .. })), "{error}");
        let error = channel.wakeup_actor(&context, stranger).await.expect_err("nor to be woken");
        assert!(matches!(error.downcast_ref::<ChannelError>(), Some(ChannelError::NotAParticipant { .. })), "the same error: {error}");
    }

    /// Issue 417: what enters the channel is recorded with its sender and
    /// whom the moderator delivered it to - everybody, as every channel
    /// delivers, or some alone.
    #[tokio::test]
    async fn what_enters_is_recorded_with_its_sender_and_whom_it_went_to() {
        let (alice, bob) = (user("alice"), user("bob"));
        let said = |text: &str| Envelope::new(
            SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default()),
            EventContext { sender_id: ActorId::User(alice.id), ..EventContext::default() },
            ActorEvent::CompleteMessageGenerated(Message { sender_id: ActorId::User(alice.id), role: MessageRole::User, content: Some(text.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) }),
        );
        let recorded = |channel: Arc<ChannelLoop>| async move {
            let state = channel.state.lock().await;
            let said = state.history.entries().find(|node| node.entry.entry.said().is_some()).map(|node| (node.entry.sender, node.entry.recipients.clone()));
            said
        };

        let everybody = channel_with(&[&alice, &bob], Arc::new(AllowAllPolicy::new())).await;
        everybody.enter(said("hello"), &HashMap::new()).await;
        assert_eq!(recorded(Arc::clone(&everybody)).await, Some((ActorId::User(alice.id), Recipients::members(true))), "as every channel delivers: to every member");

        let to_bob = channel_moderated_by(&[&alice, &bob], Arc::new(AskBack(ActorId::User(bob.id)))).await;
        to_bob.enter(said("psst"), &HashMap::new()).await;
        assert_eq!(recorded(Arc::clone(&to_bob)).await, Some((ActorId::User(alice.id), Recipients::only(vec![ActorId::User(bob.id)]))), "to whom the moderator named");
        let state = to_bob.state.lock().await;
        let asked = state.history.last().map(|node| (node.entry.entry.clone(), node.entry.sender, node.entry.recipients.clone()));
        assert!(matches!(asked, Some((HistoryEntry::ResponseRequested { by, reason: RequestReason::Mentioned, .. }, ActorId::System, ref recipients)) if by == ActorId::User(alice.id) && *recipients == Recipients::only(vec![ActorId::User(bob.id)])),
            "and whom it asked to respond, after it, in the system's name, and why: {asked:?}");
    }

    /// Issue 417: what the channel delivers carries the position of its
    /// entry - the event as it entered, and the request to respond the
    /// channel records after it -, so that a client tells the live event
    /// from the item it reads later.
    #[tokio::test]
    async fn what_is_delivered_carries_the_position_of_its_entry() {
        let (alice, bob) = (user("alice"), user("bob"));
        let channel = channel_moderated_by(&[&alice, &bob], Arc::new(AskBack(ActorId::User(bob.id)))).await;
        let (tx, mut rx) = opened();
        let his = ActorId::User(bob.id);
        let abort_handle = tokio::spawn(async {}).abort_handle();
        let running = HashMap::from([(his, RunningActor { id: his, abort_handle, task: TaskTracker::new(), inbox: Some(tx), leaving: None })]);
        let hers = ActorId::User(alice.id);
        let said = Message { sender_id: hers, role: MessageRole::User, content: Some("hello".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        channel.enter(Envelope::new(SecurityContext::default(), EventContext { sender_id: hers, ..EventContext::default() }, ActorEvent::CompleteMessageGenerated(said)), &running).await;

        let state = channel.state.lock().await;
        let at = |position: Option<Position>| position.and_then(|seq| state.history.at(seq)).map(|node| node.entry.entry.clone());
        let delivered = rx.try_recv().await.expect("delivered");
        assert!(matches!(at(delivered.context.position), Some(HistoryEntry::Said { .. })), "the message, at its entry: {:?}", delivered.context.position);
        let asked = rx.try_recv().await.expect("and he is asked");
        assert!(matches!(at(asked.context.position), Some(HistoryEntry::ResponseRequested { .. })), "the request, at its own: {:?}", asked.context.position);
        assert!(matches!(asked.event, ActorEvent::ResponseRequested { reason: RequestReason::Mentioned }), "for the reason the moderator gave: {:?}", asked.event);
        assert!(asked.context.position > delivered.context.position, "recorded after it");
    }

    /// Issue 417: what only its sender's owner sees enters the channel and is
    /// recorded with it, and reaches nobody else - neither the moderator,
    /// which decides on what everybody may see, nor a participant; a message
    /// of which nothing is left to show is recorded as gone to nobody.
    #[tokio::test]
    async fn what_only_its_owner_sees_is_recorded_and_given_to_nobody_else() {
        use caelumora_model::confidentiality::Visibility;
        use caelumora_model::tool::ToolVisibility;
        /// Asks nobody, and keeps what it was given.
        #[derive(Debug, Default)]
        struct Told(Mutex<Vec<Envelope>>);
        #[async_trait::async_trait]
        impl Moderator for Told {
            async fn route(&self, event: &Envelope) -> Routing {
                self.0.lock().unwrap().push(event.clone());
                Routing::default()
            }
            fn add_actor(&self, _: Arc<dyn Actor>) {}
            fn remove_actor(&self, _: ActorId) {}
        }
        let alice = user("alice");
        let told = Arc::new(Told::default());
        let channel = channel_moderated_by(&[&alice], Arc::<Told>::clone(&told)).await;
        let (tx, mut rx) = opened();
        let hers = ActorId::User(alice.id);
        let abort_handle = tokio::spawn(async {}).abort_handle();
        let running = HashMap::from([(hers, RunningActor { id: hers, abort_handle, task: TaskTracker::new(), inbox: Some(tx), leaving: None })]);
        let agent = ActorId::Agent(AgentId::new());
        let hidden = ToolCall { id: Some("1".to_string()), function_name: "look".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::HIDDEN, presentation: Default::default() };
        let thinking = Message { sender_id: agent, role: MessageRole::Assistant, reasoning: Some("weighing".to_string()), reasoning_visibility: Visibility::Owner, tool_calls: vec![hidden], ..Message::new(Classification::CHANNEL_DEFAULT) };
        let said = |message: Message| Envelope::new(SecurityContext::default(), EventContext { sender_id: agent, ..EventContext::default() }, ActorEvent::CompleteMessageGenerated(message));
        let parts = |event: &ActorEvent| match event {
            ActorEvent::CompleteMessageGenerated(m) => (m.content.clone(), m.reasoning.clone(), m.tool_calls.len()),
            other => panic!("expected a message, got {}", other.kind()),
        };
        let recorded = || async {
            let state = channel.state.lock().await;
            let last = state.history.last().expect("recorded").entry.clone();
            (last.entry.said().map(|m| (m.content.clone(), m.reasoning.clone(), m.tool_calls.len())), last.recipients)
        };

        channel.enter(said(Message { content: Some("found it".to_string()), ..thinking.clone() }), &running).await;
        assert_eq!(parts(&rx.try_recv().await.expect("delivered").event), (Some("found it".to_string()), None, 0), "alice is given what everybody may see");
        assert_eq!(told.0.lock().unwrap().iter().map(|e| parts(&e.event)).collect::<Vec<_>>(), [(Some("found it".to_string()), None, 0)], "and so is the moderator");
        assert_eq!(recorded().await, (Some((Some("found it".to_string()), Some("weighing".to_string()), 1)), Recipients::members(true)), "the channel records it whole");

        channel.enter(said(thinking), &running).await;
        assert!(rx.try_recv().await.is_none(), "nothing of it is left to show");
        assert_eq!(told.0.lock().unwrap().len(), 1, "nor to decide on");
        assert_eq!(recorded().await, (Some((None, Some("weighing".to_string()), 1)), Recipients::NOBODY), "recorded whole, as gone to nobody");
    }

    /// Issue 404: what others hear of a message is announced with the
    /// message once it is stored, as news of the channel; a step of a plan
    /// or a notice of the runtime is not.
    #[tokio::test]
    async fn what_others_hear_is_announced_once_it_is_stored() {
        use crate::session::testing::{drain, running_session_of_a_user, spec, test_runtime};
        use crate::session::SessionManager;
        use caelumora_model::channel::{Channel, SAID};
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let channel = session.create_channel(&ctx, spec("side", vec![])).await.unwrap();
        let owner = ctx.initiator_id;
        let said = |message: &Message| Envelope::new(ctx.clone(), EventContext { sender_id: owner, ..EventContext::default() }, ActorEvent::CompleteMessageGenerated(message.clone()));
        let heard = Message { id: MessageId::new(), role: MessageRole::User, sender_id: owner, content: Some("hello".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let step = Message { id: MessageId::new(), internal: true, ..heard.clone() };
        let notice = Message { id: MessageId::new(), role: MessageRole::System, ..heard.clone() };
        let mut rx = runtime.event_bus().subscribe_all();

        for message in [&heard, &step, &notice] {
            channel.append_history(&said(message), Audience::Everybody).await;
        }

        let announced: Vec<MessageId> = drain(&mut rx).iter()
            .filter_map(|event| event.about::<Channel>().filter(|((_, id), _)| *id == channel.id()))
            .filter_map(|(_, change)| change.data::<Message>(SAID))
            .map(|message| message.id)
            .collect();
        assert_eq!(announced, [heard.id], "the words others hear, with the message");
        assert_eq!(channel.history().await.len(), 3, "though every one is kept");
    }

    #[tokio::test]
    async fn a_message_carries_only_what_its_sender_uploaded_for_the_channel() {
        let world = World::new().await;
        let session = world.shared_session(&[], &[]).await;
        let main = Arc::clone(&session.channels()[0]);
        let view = world.manager.views(Viewer::new(World::as_user(world.alice))).session(session.id()).await.unwrap().channel(main.id()).await.unwrap();
        let attachments = world.runtime.attachment_manager();
        let upload = || attachments.create(&view, "a.txt", None, (1, file("a")));
        let (alice, bob) = (ActorId::User(world.alice), ActorId::User(world.bob));
        let sent = |sender: ActorId, event: ActorEvent| Envelope::new(
            SecurityContext::new(sender, session.id(), main.id()),
            EventContext { sender_id: sender, ..EventContext::default() },
            event,
        );
        let said = |sender: ActorId, attachments: Vec<ContentRef>| sent(sender, ActorEvent::CompleteMessageGenerated(
            Message { sender_id: sender, role: MessageRole::User, attachments, ..Message::new(main.classification()) },
        ));

        let hers = upload().await.unwrap();
        assert_eq!(main.admission(&said(bob, vec![hers.clone()])).await, Err(Rejection::Invalid), "not his");
        let in_her_name = sent(bob, ActorEvent::CompleteMessageGenerated(Message { sender_id: alice, role: MessageRole::User, attachments: vec![hers.clone()], ..Message::new(main.classification()) }));
        assert_eq!(main.admission(&in_her_name).await, Err(Rejection::Invalid), "not his for a message naming her");
        let call = ToolCall { id: None, function_name: "f".to_string(), arguments: serde_json::json!({}), visibility: Default::default(), presentation: Default::default() };
        let result = ToolCallResult { media: vec![hers.clone()], ..ToolCallResult::of(&call, Default::default(), "done".to_string(), main.classification()) };
        let with_media = sent(alice, ActorEvent::CompleteMessageGenerated(Message { sender_id: alice, role: MessageRole::Assistant, tool_results: vec![result], ..Message::new(main.classification()) }));
        assert_eq!(main.admission(&with_media).await, Err(Rejection::Invalid), "what a tool gave, not yet (step 5)");
        let elsewhere = ContentRef { key: format!("attachments/{}/{}/{}", session.id(), ChannelId::new(), hers.id), ..hers.clone() };
        assert_eq!(main.admission(&said(alice, vec![elsewhere])).await, Err(Rejection::Invalid), "only as it was uploaded");
        let mut many = Vec::new();
        for _ in 0..11 {
            many.push(upload().await.unwrap());
        }
        assert_eq!(main.admission(&said(alice, many.clone())).await, Err(Rejection::Invalid), "more than a message carries");
        assert_eq!(main.admission(&said(alice, many[..10].to_vec())).await, Ok(()), "and claimed so");
        assert_eq!(main.admission(&said(alice, many[..1].to_vec())).await, Err(Rejection::Invalid), "by one message only");

        let actor = main.user_actor(world.alice).unwrap();
        let mut watch = actor.watch();
        actor.send_message(None, None, vec![hers.id]).await.expect("an attachment alone is a message");
        let ActorEvent::CompleteMessageGenerated(message) = shown(&mut watch, "CompleteMessageGenerated").await else { unreachable!() };
        assert_eq!(message.attachments, slice::from_ref(&hers));
        assert_eq!(main.admission(&said(alice, vec![hers.clone()])).await, Err(Rejection::Invalid), "claimed by the message said");

        let modified = |attachments: Vec<ContentRef>| sent(alice, ActorEvent::MessageModified(Message { attachments, ..message.clone() }));
        let added = upload().await.unwrap();
        assert_eq!(main.admission(&modified(vec![hers.clone(), added.clone()])).await, Ok(()), "what it carries, and what she uploaded");
        let his = world.manager.views(Viewer::new(World::as_user(world.bob))).session(session.id()).await.unwrap().channel(main.id()).await.unwrap();
        let his = attachments.create(&his, "b.txt", None, (1, file("b"))).await.unwrap();
        assert_eq!(main.admission(&modified(vec![hers, his])).await, Err(Rejection::Invalid), "never another's upload");
    }
}
