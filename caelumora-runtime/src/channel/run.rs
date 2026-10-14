//! The run loop of a channel: starting and stopping its actors, admitting, routing, recording and
//! appending to the history what they send.

use crate::logging::envelope_span;
use crate::transport::{EventSender, TransportError};
use crate::transport::{FilteringEventSender, InMemoryEventReceiver, InMemoryEventSender, OriginEventSender};
use anyhow::{anyhow, Result};
use caelumora_model::actor::{ActorEvent, ActorId, Envelope, EventContext, Rejection, RetryTarget};
use caelumora_model::confidentiality::ConfidentialityQuestions;
use caelumora_model::message::Message;
use caelumora_model::message_history::HistoryEntry;
use caelumora_model::security::{ActionType, Enforcement, Reclassified, SecurityContext};
use tracing::{debug, info, trace, warn, Instrument};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinSet};
use tokio_util::task::TaskTracker;
use super::{ActorChange, ChannelActor, ChannelLoop, stopped};

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
    sender: Option<Box<dyn EventSender>>,
    /// On whose behalf it was removed, once it was told to stop (issue
    /// 262): its task still runs - the step it was in, its farewell - and
    /// `run` announces the removal in this name once the task has ended.
    leaving: Option<SecurityContext>,
}

impl ChannelLoop {
    pub async fn run(&self) -> Result<()> {
        let _guard = self.run_lock.try_lock()
            .map_err(|_| anyhow!("Already running"))?;

        info!("Channel {}/{} is starting", self.channel.name(), self.id);
        let mut changes_rx = self.changes_rx.lock().await;
        let (event_tx, mut event_rx) = mpsc::channel(10);
        let mut join_set = JoinSet::new();
        let mut running = self.spawn_actors(event_tx.clone(), &mut join_set);

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
                    if let Some(RunningActor { id, leaving: Some(remover), .. }) = ended.and_then(|id| running.remove(&id)) {
                        self.announce_removal(id, remover, &running).await;
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

    /// Starts or stops an actor as a participant joined or left (issue 46).
    async fn apply_change(&self, change: ActorChange, running: &mut HashMap<ActorId, RunningActor>, event_tx: &Sender<Envelope>, join_set: &mut JoinSet<ActorId>) {
        match change {
            ActorChange::Start(joined) => {
                // Joined before `run` started: already running. Removed
                // again before `run` got to start it: never started, so that
                // it cannot run next to whoever joins under its id after it
                // (issue 171).
                let id = joined.actor.id();
                let still_joined = self.actors.load().get(&id).is_some_and(|current| Arc::ptr_eq(&current.actor, &joined.actor));
                // Its old actor has ended - `add_participant` waited for
                // that (`left`) - but its task's end may not have been seen
                // yet: the removal is announced before anything of the new
                // actor (issue 262).
                if running.get(&id).is_some_and(|old| old.leaving.is_some()) {
                    if let Some(RunningActor { leaving: Some(remover), .. }) = running.remove(&id) {
                        self.announce_removal(id, remover, running).await;
                    }
                }
                if !running.contains_key(&id) && still_joined {
                    let handle = self.spawn_actor(&joined, event_tx.clone(), join_set);
                    running.insert(handle.id, handle);
                }
            }
            ActorChange::Stop(id, remover) => {
                // Dropping its inbox closes it; told to stop (`Actor::stop`),
                // the actor ends its task itself once its state is stored
                // consistent. Only if it does not - a tool that hangs - is it
                // aborted. It keeps running until then: what it sends is
                // taken (`handle_actor_event`), and the removal is announced
                // once its task has ended (issue 262). An actor never started
                // has nothing to say: announced at once.
                match running.get_mut(&id) {
                    Some(handle) => {
                        handle.sender = None;
                        handle.leaving = Some(remover);
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
                    None => self.announce_removal(id, remover, running).await,
                }
            }
        }
    }

    /// Announces that `participant` left, in the name of `remover`'s caller
    /// (`ParticipantRemoved`, issue 162) - between the events `run` routes,
    /// once the participant's task has ended: what it sent before is in the
    /// channel, nothing of it follows (issue 262).
    async fn announce_removal(&self, participant: ActorId, remover: SecurityContext, running: &HashMap<ActorId, RunningActor>) {
        let name = self.controllers.actor_name(remover.caller_id).await.unwrap_or_else(|e| {
            warn!("Cannot name {} for channel {}: {}", remover.caller_id, self.id, e);
            remover.caller_id.to_string()
        });
        let sender = EventContext::new(self.session_id, self.id, remover.caller_id, name);
        let removed = Envelope::new(remover, sender, ActorEvent::ParticipantRemoved { participant });
        self.process_envelope(removed, running).await;
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
            self.reject(&event, reason, running);
            return;
        }
        let event = self.resolve_mentions(event);
        self.record_receive_event(&event).await;
        self.append_history(&event).await;
        self.route_event(running, event).await;
    }

    /// Starts the tasks of the channel's actors as `run` begins.
    fn spawn_actors(&self, actor_output: Sender<Envelope>, join_set: &mut JoinSet<ActorId>) -> HashMap<ActorId, RunningActor> {
        self.actors.load().iter()
            .map(|(id, actor)| (*id, self.spawn_actor(actor, actor_output.clone(), join_set)))
            .collect()
    }

    fn spawn_actor(&self, channel_actor: &ChannelActor, actor_output: Sender<Envelope>, join_set: &mut JoinSet<ActorId>) -> RunningActor {
        let ChannelActor { actor, task, .. } = channel_actor.clone();
        let id = actor.id();
        let filter = actor.event_filter();
        let (tx, rx) = mpsc::channel(100);

        let actor_rx = Box::new(InMemoryEventReceiver::new(rx));
        // It sends only in its own name (issue 228).
        let actor_tx = Box::new(OriginEventSender::new(id, Box::new(InMemoryEventSender::new(actor_output))));

        // Tracked inside the `JoinSet`'s task: a task that is aborted counts
        // as ended, too.
        let abort_handle = join_set.spawn(task.track_future(async move {
            let result = actor.run(actor_rx, actor_tx).await;
            if let Err(e) = result {
                tracing::error!("Actor {} stopped with an error: {:#}", actor.id(), e)
            }
            id
        }));

        // Wrap the actor's input sender with its declared filter predicate.
        let filtered_sender = Box::new(FilteringEventSender::new(
            Box::new(InMemoryEventSender::new(tx)),
            filter,
        ));

        RunningActor { id, abort_handle, task, sender: Some(filtered_sender), leaving: None }
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
    ///   something into the channel -, each part of it, the message and its
    ///   tool results, as it is classified (`ConfidentialityPolicy::
    ///   may_publish`, issue 133): its sender misclassified what does not
    ///   flow in;
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
    ///   its own name (issue 284): nobody else has turns.
    async fn admission(&self, event: &Envelope) -> Result<(), Rejection> {
        let context = &event.security;
        let messages = event.event.messages();
        if !messages.is_empty() && context.caller_id != event.context.sender_id {
            return Err(Rejection::Invalid);
        }
        let classification = self.classification();
        let channel = self.security_attributes();
        let forbidden = |_| Rejection::Forbidden;
        match &event.event {
            ActorEvent::MessageModified(modified) => {
                let mut message = self.message(modified.id).await.ok_or(Rejection::MessageNotFound)?;
                self.security.require(context, ActionType::ModifyMessage { author: message.sender_id, channel: &channel }, forbidden).await?;
                message.modify(modified).then_some(()).ok_or(Rejection::MessageUnchangeable)
            }
            ActorEvent::MessageRevoked { message, at } => {
                let mut message = self.message(*message).await.ok_or(Rejection::MessageNotFound)?;
                self.security.require(context, ActionType::RevokeMessage { author: message.sender_id, channel: &channel }, forbidden).await?;
                message.revoke(*at).then_some(()).ok_or(Rejection::MessageUnchangeable)
            }
            ActorEvent::MessageReclassified { message, classification: new } => {
                let mut said = self.message(*message).await.ok_or(Rejection::MessageNotFound)?;
                let target = Reclassified::Message { author: said.sender_id, attributes: &channel };
                self.security.require(context, ActionType::Reclassify(target), forbidden).await?;
                self.confidentiality.require_publish(context, new, &classification, || Rejection::ClassificationRefused)?;
                said.reclassify(new).then_some(()).ok_or(Rejection::MessageUnchangeable)
            }
            ActorEvent::RetryRequested { target: RetryTarget::Message(message), author } => {
                let message = self.message(*message).await.ok_or(Rejection::MessageNotFound)?;
                self.security.require(context, ActionType::RetryMessage(&channel), forbidden).await?;
                if message.revoked.is_some() {
                    return Err(Rejection::MessageUnchangeable);
                }
                (message.sender_id == *author && self.agent_actor(*author).is_some()).then_some(()).ok_or(Rejection::Invalid)
            }
            // Whether the turn is still under way is decided as the request
            // is delivered, where the turn is ended (`route_event_impl`).
            ActorEvent::RetryRequested { target: RetryTarget::Turn(_), author } => {
                self.security.require(context, ActionType::RetryMessage(&channel), forbidden).await?;
                self.agent_actor(*author).is_some().then_some(()).ok_or(Rejection::Invalid)
            }
            ActorEvent::QuestionAnswered { .. } | ActorEvent::QuestionDeclined { .. } => {
                self.security.require(context, ActionType::AnswerQuestion(&channel), forbidden).await
            }
            ActorEvent::ApprovalGranted(response) | ActorEvent::ApprovalDenied(response) if event.context.sender_id.as_user_id().is_some() => {
                let agent = self.controllers.agent_attributes(response.agent_id).await.map_err(|_| Rejection::Invalid)?;
                self.security.require(context, ActionType::ApproveToolCall { agent: &agent, channel: &channel }, forbidden).await
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
                (event.context.sender_id.as_user_id().is_some() && *participant != event.context.sender_id && self.actor_by_id(*participant).is_some())
                    .then_some(()).ok_or(Rejection::Invalid)
            }
            ActorEvent::PlanStepStarted { .. }
            | ActorEvent::CommandExecuted { turn_id: Some(_), .. }
            | ActorEvent::ActivityStarted { .. }
            | ActorEvent::ActivityFinished { .. }
            | ActorEvent::TurnFinished { .. }
            | ActorEvent::TurnGivenUp { .. } => {
                let sender = event.context.sender_id;
                (sender.as_agent_id().is_some() && context.caller_id == sender).then_some(()).ok_or(Rejection::Invalid)
            }
            _ => Ok(()),
        }?;
        for message in messages {
            let parts = std::iter::once(&message.classification).chain(message.tool_results.iter().map(|r| &r.classification));
            for part in parts {
                self.confidentiality.require_publish(context, part, &classification, || Rejection::ClassificationRefused)?;
            }
        }
        Ok(())
    }

    /// Tells the sender of `event`, which the channel did not let in, why
    /// (`MessageRejected`, issue 304) - it alone, in the channel's name; not
    /// one that left, whose inbox is closed.
    fn reject(&self, event: &Envelope, reason: Rejection, running: &HashMap<ActorId, RunningActor>) {
        warn!("Dropped {}: {}", event.describe(), reason);
        let sender = event.context.sender_id;
        let Some(inbox) = running.get(&sender).and_then(|actor| actor.sender.as_ref()) else { return };
        let mut context = EventContext::new(self.session_id, self.id, ActorId::System, ActorId::System.to_string());
        context.span_id = event.context.span_id;
        context.correlation_id = event.context.event_id;
        let rejected = ActorEvent::MessageRejected { rejected: event.event.kind().to_string(), message: event.event.subject_message(), reason };
        let envelope = Envelope::new(SecurityContext::system(), context, rejected);
        if let Err(e) = inbox.try_send(envelope) {
            warn!("Could not tell {} that channel {} did not let in what it sent: {:?}", sender, self.id, e);
        }
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

    async fn route_event(&self, running: &HashMap<ActorId, RunningActor>, event: Envelope) {
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

        self.route_event_impl(running, &event).await;
        #[cfg(test)]
        let _ = self.observed.send(event);
    }

    async fn route_event_impl(&self, running: &HashMap<ActorId, RunningActor>, event: &Envelope) {
        if event.context.span_depth >= self.max_route_decisions {
            warn!("Too many routings, stopping further propagation");
            return
        }

        let span_id = event.context.span_id;
        let span_depth = event.context.span_depth + 1;
        let decision = self.moderator.route(event).await;
        let quiet = self.quiet.load(Ordering::SeqCst);

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
                .filter(|(target_id, route)| *target_id != event.context.sender_id && matches!(route.event, ActorEvent::ResponseRequested | ActorEvent::RetryRequested { .. }));
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
            // `ChainCutOff` to the agent whose chain it cut off (issue 200).
            if target_id == event.context.sender_id && route.1.context.sender_id == event.context.sender_id {
                continue;
            }
            // A cancelled channel stays quiet until it is woken: what was said
            // still reaches everyone, so that every transcript keeps up with
            // the history, but nobody is asked to answer it.
            if quiet && matches!(route.1.event, ActorEvent::ResponseRequested) {
                debug!(to = %target_id, "Not asking for a response: the channel is quiet after a cancel");
                continue;
            }

            let mut target_event = route.1;
            let sender_id = target_event.context.sender_id;
            let event_desc = target_event.describe();
            // An answer under way asked for again (issue 235): its turn is
            // ended before the request is delivered, so that the agent finds
            // it given up as it takes the request in - and only if it is
            // still the turn under way. A turn given up already is delivered
            // as it is, if it is the agent's last (issue 259). Otherwise
            // there is nothing to answer again, and the request is dropped:
            // its sender asked before, and its users see the answer end.
            if let ActorEvent::RetryRequested { target: RetryTarget::Turn(turn), .. } = &target_event.event {
                let interrupted = self.actor_by_id(target_id).is_some_and(|actor| actor.retry_turn(*turn));
                if !interrupted && !self.given_up_last(target_id, *turn).await {
                    warn!(to = %target_id, "Dropped {}: turn {} is neither under way nor the last given up", event_desc, turn);
                    continue;
                }
            }
            target_event.context.channel_id = self.id;
            target_event.context.session_id = self.session_id;
            target_event.context.span_id = span_id;
            target_event.context.span_depth = span_depth;
            // Not to an actor that is leaving: its inbox is closed.
            if let Some(sender) = running.get(&target_id).and_then(|actor| actor.sender.as_ref()) {
                debug!(to = %target_id, "Forwarding {}", event_desc);
                self.record_send_event(&target_event).await;
                if let Err(e) = sender.try_send(target_event) {
                    match e {
                        TransportError::Full => warn!("Queue full for actor {}", target_id),
                        _ => warn!("Could not forward event {} from={} to={}: {:?}", event_desc, sender_id, target_id, e)
                    }
                }
            }
        }
    }

    async fn append_history(&self, envelope: &Envelope) {
        let mut state = self.state.lock().await;
        let changed = match &envelope.event {
            // What is said is kept as it arrives, a turn's steps too, and
            // nothing said is unsaid (issue 258); where a step of its plan
            // begins is recorded before the step's messages (issue 263), a
            // turn's end after its messages.
            ActorEvent::CompleteMessageGenerated(message) | ActorEvent::IntermediateMessageGenerated(message) => {
                state.history.push(message.clone(), None);
                true
            }
            ActorEvent::TurnFinished { turn_id, .. } => {
                state.history.end_turn(HistoryEntry::TurnFinished { turn_id: *turn_id, actor: envelope.context.sender_id, at: envelope.timestamp });
                true
            }
            ActorEvent::TurnGivenUp { turn_id, reason } => {
                state.history.end_turn(HistoryEntry::TurnGivenUp { turn_id: *turn_id, actor: envelope.context.sender_id, reason: reason.clone(), at: envelope.timestamp });
                true
            }
            // A command an agent carried out is part of the record (issue
            // 269), before its turn's end; who asked is the initiator. One
            // the session ran addresses the system and is not.
            ActorEvent::CommandExecuted { turn_id: Some(turn_id), command, args, .. } => {
                state.history.record_command(HistoryEntry::CommandExecuted {
                    turn_id: *turn_id,
                    actor: envelope.context.sender_id,
                    issuer: envelope.security.initiator_id,
                    command: command.clone(),
                    args: args.clone(),
                    at: envelope.timestamp,
                });
                true
            }
            // What an agent did of its own accord is part of the record
            // (issue 284), with what came of it, before its turn's end.
            ActorEvent::ActivityFinished { turn_id, activity, output } => {
                state.history.record_activity(HistoryEntry::ActivityFinished {
                    turn_id: *turn_id,
                    actor: envelope.context.sender_id,
                    activity: activity.clone(),
                    output: output.clone(),
                    at: envelope.timestamp,
                });
                true
            }
            ActorEvent::PlanStepStarted { turn_id, step_index, total_steps, description } => {
                state.history.start_plan_step(HistoryEntry::PlanStepStarted {
                    turn_id: *turn_id,
                    actor: envelope.context.sender_id,
                    step_index: *step_index,
                    total_steps: *total_steps,
                    description: description.clone(),
                    at: envelope.timestamp,
                });
                true
            }
            // Admitted, so they apply (`admission`, issues 234, 304).
            ActorEvent::MessageModified(modified) => state.history.modify(modified.clone()),
            ActorEvent::MessageRevoked { message, at } => state.history.revoke(*message, *at),
            ActorEvent::MessageReclassified { message, classification } => state.history.reclassify(*message, classification.clone()),
            _ => false,
        };

        if changed {
            if let Err(e) = self.state_store.put_channel_state(self.id, &mut state).await {
                warn!("Cannot store state of channel {}: {}", self.id, e);
            }
        }
    }
    async fn record_send_event(&self, event: &Envelope) {
        let mut recorder = self.recorder.lock().await;
        recorder.record_send(event).await;
        drop(recorder);
    }
    async fn record_receive_event(&self, event: &Envelope) {
        let mut recorder = self.recorder.lock().await;
        recorder.record_receive(event).await;
        drop(recorder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::value::name::Name;
    use crate::actor::Actor;
    use crate::component::ComponentImpl;
    use crate::transport::EventSender;
    use crate::transport::InMemoryEventSender;
    use anyhow::Result;
    use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext, Rejection, RetryTarget};
    use caelumora_model::approval::Approver;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::agent::AgentId;
    use caelumora_model::message::{Message, MessageId, MessageRole};
    use caelumora_model::work::TurnId;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::user::UserId;
    use std::collections::HashMap;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio::task::JoinSet;
    use tokio_util::task::TaskTracker;
    
    use crate::channel::{ActorChange, ChannelError, Member};
    use caelumora_model::security::DummySecurityManager;
    use crate::channel::tests::*;

    #[tokio::test]
    async fn a_users_own_message_is_shown_with_its_mentions_resolved() {
        let (alice, bob) = (user("alice"), user("bob"));
        let channel = channel_with(&[&alice, &bob], Arc::new(DummySecurityManager::new())).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let mut following = actor.watch().events;

        actor.send_message("@bob, look".to_string(), None).await.expect("sent");

        let shown = following.try_recv().expect("shown at once, before the channel takes it in");
        let ActorEvent::CompleteMessageGenerated(message) = shown.event else { panic!("its message, got {}", shown.event.kind()) };
        assert_eq!(message.mentions, vec![ActorId::User(bob.id)], "as the channel resolves them (issue 220)");
    }

    /// Sends every event back to its sender, once as it is and once as a
    /// `ChainCutOff` of its own, in the name of the system.
    #[derive(Debug)]
    struct TellsTheSender;

    #[async_trait::async_trait]
    impl crate::moderator::Moderator for TellsTheSender {
        async fn route(&self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
            let mut notice = event.forward(ActorEvent::ChainCutOff { message: MessageId::new(), max_agent_hops: 0, not_asked: Vec::new() });
            notice.context.sender_id = ActorId::System;
            vec![(event.context.sender_id, event.clone()), (event.context.sender_id, notice)]
        }
        fn add_actor(&self, _: Arc<dyn Actor>) {}
        fn remove_actor(&self, _: ActorId) {}
    }

    #[tokio::test]
    async fn the_sender_gets_back_only_what_the_moderator_sends_in_another_name() {
        use caelumora_model::actor::EventContext;
        let alice = user("alice");
        let bob = ActorId::Agent(caelumora_model::agent::AgentId::new());
        let channel = channel_moderated_by(&[&alice], Arc::new(TellsTheSender)).await;
        let (tx, mut rx) = mpsc::channel(8);
        let abort_handle = tokio::spawn(async {}).abort_handle();
        let actors = HashMap::from([(bob, RunningActor { id: bob, abort_handle, task: TaskTracker::new(), sender: Some(Box::new(InMemoryEventSender::new(tx))), leaving: None })]);
        let said = Envelope::new(
            SecurityContext::default(),
            EventContext { sender_id: bob, ..EventContext::default() },
            ActorEvent::TurnFinished { turn_id: caelumora_model::work::TurnId::new(), messages: vec![Message::new(Classification::CHANNEL_DEFAULT)] },
        );

        channel.route_event(&actors, said).await;
        let mut kinds = Vec::new();
        while let Ok(envelope) = rx.try_recv() { kinds.push(envelope.event.kind()); }
        assert_eq!(kinds, ["ChainCutOff"], "its own message is not sent back, but what the system tells it is (issue 200)");
    }

    /// An agent that says one thing as it starts, as the answer to a request
    /// one hop into a chain between agents, and waits.
    #[derive(Debug)]
    struct SaysOnce(ActorId);

    #[async_trait::async_trait]
    impl Actor for SaysOnce {
        fn id(&self) -> ActorId { self.0 }
        fn kind(&self) -> String { "says_once".to_string() }
        fn name(&self) -> Name { Name::new("talker").expect("a name") }
        fn event_filter(&self) -> fn(&Envelope) -> bool { |_| true }
        async fn run(&self, mut rx: Box<dyn crate::transport::EventReceiver>, tx: Box<dyn EventSender>) -> Result<()> {
            use caelumora_model::actor::EventContext;
            let mut context = EventContext { sender_id: self.0, ..EventContext::default() };
            context.agent_hops = 1;
            let said = Message { sender_id: self.0, role: caelumora_model::message::MessageRole::Assistant, content: Some("and you?".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
            let _ = tx.send(Envelope::new(SecurityContext::default().next(self.0, ActorId::None, ActorId::None), context, ActorEvent::TurnFinished { turn_id: caelumora_model::work::TurnId::new(), messages: vec![said] })).await;
            while rx.recv().await.is_some() {}
            Ok(())
        }
        fn state(&self) -> ActorState { ActorState::Idle }
        fn wakeup(&self) {}
        fn stop(&self) {}
    }

    #[tokio::test]
    async fn a_user_following_the_channel_is_told_when_a_chain_is_cut_off() {
        use crate::moderator::policy_moderator::PolicyModerator;
        use caelumora_model::approval::{ApprovalStrategy, ApproverId};
        let alice = user("alice");
        let responders = ComponentImpl { id: "responders".to_string(), fail_policy: Default::default(), facet: Arc::new(crate::moderator::broadcast_policy::BroadcastPolicy) as Arc<dyn crate::moderator::ResponderPolicy> };
        let moderator = Arc::new(PolicyModerator::new(responders, 1));
        let channel = channel_moderated_by(&[&alice], Arc::<PolicyModerator>::clone(&moderator)).await;
        let talker = Arc::new(SaysOnce(ActorId::Agent(caelumora_model::agent::AgentId::new())));
        channel.adopt(Arc::<SaysOnce>::clone(&talker), Member::Approver(Approver { id: ApproverId::new(), strategy: ApprovalStrategy::Any })).await;
        let mut following = channel.user_actor(alice.id).unwrap().watch().events;
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

        let told = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let ActorEvent::ChainCutOff { max_agent_hops, .. } = following.recv().await.expect("the channel runs").event {
                    break max_agent_hops;
                }
            }
        }).await.expect("the web UI follows the user's actor, which is told (issues 200, 220)");
        assert_eq!(told, 1);
        running.abort();
    }

    /// An actor that says what it is given to say as it starts, as it is
    /// written - in whatever name - and waits.
    #[derive(Debug)]
    struct Says(ActorId, std::sync::Mutex<Vec<Envelope>>);

    #[async_trait::async_trait]
    impl Actor for Says {
        fn id(&self) -> ActorId { self.0 }
        fn kind(&self) -> String { "says".to_string() }
        fn name(&self) -> Name { Name::new("talker").expect("a name") }
        fn event_filter(&self) -> fn(&Envelope) -> bool { |_| true }
        async fn run(&self, mut rx: Box<dyn crate::transport::EventReceiver>, tx: Box<dyn EventSender>) -> Result<()> {
            let said = std::mem::take(&mut *self.1.lock().unwrap());
            for envelope in said {
                let _ = tx.send(envelope).await;
            }
            while rx.recv().await.is_some() {}
            Ok(())
        }
        fn state(&self) -> ActorState { ActorState::Idle }
        fn wakeup(&self) {}
        fn stop(&self) {}
    }

    #[tokio::test]
    async fn an_actor_cannot_speak_in_the_name_of_another() {
        use caelumora_model::approval::{ApprovalStrategy, ApproverId};
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(DummySecurityManager::new())).await;
        let talker = ActorId::Agent(caelumora_model::agent::AgentId::new());
        let her = ActorId::User(alice.id);
        let said = |sender: ActorId, caller: ActorId, author: ActorId, text: &str| {
            let message = Message { sender_id: author, role: caelumora_model::message::MessageRole::Assistant, content: Some(text.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
            Envelope::new(SecurityContext::default().next(caller, ActorId::None, ActorId::None), EventContext { sender_id: sender, ..EventContext::default() }, ActorEvent::CompleteMessageGenerated(message))
        };
        let envelopes = vec![
            said(her, talker, talker, "as alice's envelope"),
            said(talker, her, talker, "in alice's security context"),
            said(talker, talker, her, "as alice's message"),
            // On alice's behalf - she asked - but said by itself: as an agent answers.
            Envelope { security: SecurityContext::new(her, channel.session_id, channel.id).next(talker, her, her), ..said(talker, talker, talker, "its own") },
        ];
        let says = Arc::new(Says(talker, std::sync::Mutex::new(envelopes)));
        channel.adopt(Arc::<Says>::clone(&says), Member::Approver(caelumora_model::approval::Approver { id: ApproverId::new(), strategy: ApprovalStrategy::Any })).await;
        let mut events = channel.observe();
        let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

        let entered = tokio::time::timeout(Duration::from_secs(5), async {
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
    async fn what_is_recorded_of_a_turn_comes_from_an_agent_in_its_own_name() {
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(DummySecurityManager::new())).await;
        let agent = ActorId::Agent(caelumora_model::agent::AgentId::new());
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
            ActorEvent::TurnGivenUp { turn_id, reason: caelumora_model::turn::GivenUpReason::Stopped },
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
        use caelumora_model::actor::EventContext;
        use caelumora_model::approval::{ApprovalStrategy, ApproverId};
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let (asked, other) = (sleeper(), sleeper());
        let channel = channel_moderated_by(&[&alice], Arc::new(AskBack(asked.id))).await;
        for actor in [&asked, &other] {
            // Who it is does not matter to routing: the channel only looks it up.
            channel.adopt(Arc::<Sleeper>::clone(actor), Member::Approver(Approver { id: ApproverId::new(), strategy: ApprovalStrategy::Any })).await;
        }
        let said = || Envelope::new(
            context.clone(),
            EventContext { sender_id: ActorId::User(alice.id), ..EventContext::default() },
            ActorEvent::TurnFinished { turn_id: caelumora_model::work::TurnId::new(), messages: vec![Message::new(Classification::CHANNEL_DEFAULT)] },
        );

        channel.route_event(&HashMap::new(), said()).await;
        assert!(asked.woken.load(Ordering::SeqCst), "asked to respond, it is woken to do so");
        assert!(!other.woken.load(Ordering::SeqCst), "not addressed, it sleeps on");

        // A quiet channel asks nobody, and wakes nobody.
        asked.woken.store(false, Ordering::SeqCst);
        channel.cancel(&context).await.expect("permitted");
        channel.route_event(&HashMap::new(), said()).await;
        assert!(!asked.woken.load(Ordering::SeqCst), "nobody is asked in a quiet channel");
    }

    #[tokio::test]
    async fn a_channel_admits_only_what_may_flow_into_it_in_its_senders_name() {
        use caelumora_model::actor::EventContext;
        use caelumora_model::confidentiality::Confidentiality;
        use caelumora_model::security::OwnershipSecurityManager;
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(OwnershipSecurityManager::new())).await;
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
        let call = caelumora_model::tool::ToolCall { id: None, function_name: "f".to_string(), arguments: serde_json::json!({}), visibility: Default::default(), presentation: Default::default() };
        let result = caelumora_model::tool::ToolCallResult::of(&call, Default::default(), "secret".to_string(), Classification::general(Confidentiality::Confidential));
        let mut with_result = from(alice_id, alice_id, internal);
        if let ActorEvent::CompleteMessageGenerated(message) = &mut with_result.event {
            message.tool_results.push(result);
        }
        channel.process_envelope(with_result, &actors).await;
        assert_eq!(channel.history().await.len(), 1, "nor with a result above the channel");
    }

    #[tokio::test]
    async fn a_change_to_a_message_enters_the_channel_only_from_its_author_as_it_stands() {
        use caelumora_model::actor::EventContext;
        use caelumora_model::security::OwnershipSecurityManager;
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(OwnershipSecurityManager::new())).await;
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(UserId::new()));
        let from = |sender: ActorId, event| Envelope::new(
            SecurityContext::new(sender, Default::default(), Default::default()),
            EventContext { sender_id: sender, ..EventContext::default() },
            event,
        );
        let said = Message { id: MessageId::new(), sender_id: alice_id, role: caelumora_model::message::MessageRole::User, content: Some("helo".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let modified = |sender: ActorId, content: &str| Message { sender_id: sender, content: Some(content.to_string()), timestamp: chrono::Utc::now(), ..said.clone() };
        let revoked = || ActorEvent::MessageRevoked { message: said.id, at: chrono::Utc::now() };
        let actors = HashMap::new();
        channel.process_envelope(from(alice_id, ActorEvent::CompleteMessageGenerated(said.clone())), &actors).await;
        let entries = || async { channel.state.lock().await.history.all_entries().len() };

        channel.process_envelope(from(bob_id, ActorEvent::MessageModified(modified(bob_id, "bob's"))), &actors).await;
        channel.process_envelope(from(bob_id, revoked()), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::MessageModified(Message { id: MessageId::new(), ..modified(alice_id, "nothing said") })), &actors).await;
        assert_eq!(entries().await, 1, "neither by another nor of what was never said");

        channel.process_envelope(from(alice_id, ActorEvent::MessageModified(modified(alice_id, "hello"))), &actors).await;
        let standing = channel.message(said.id).await.expect("said");
        assert_eq!((standing.content.as_deref(), standing.modified.is_some()), (Some("hello"), true), "its author modifies it");

        // Reclassified as the policy says (issue 239): here, with nobody
        // owning the channel, by its author alone.
        let reclassified = |classification| ActorEvent::MessageReclassified { message: said.id, classification };
        let above = Classification::general(caelumora_model::confidentiality::Confidentiality::Confidential);
        channel.process_envelope(from(bob_id, reclassified(Classification::PUBLIC)), &actors).await;
        channel.process_envelope(from(alice_id, reclassified(above)), &actors).await;
        assert_eq!(entries().await, 2, "neither by another nor above the channel");
        channel.process_envelope(from(alice_id, reclassified(Classification::PUBLIC)), &actors).await;
        assert_eq!(channel.message(said.id).await.map(|m| m.classification), Some(Classification::PUBLIC), "its author releases it");

        channel.process_envelope(from(alice_id, revoked()), &actors).await;
        assert!(channel.message(said.id).await.is_some_and(|m| m.revoked.is_some() && m.content.is_none()), "and takes it back");
        channel.process_envelope(from(alice_id, revoked()), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::MessageModified(modified(alice_id, "again"))), &actors).await;
        channel.process_envelope(from(alice_id, reclassified(Classification::CHANNEL_DEFAULT)), &actors).await;
        assert_eq!(entries().await, 4, "nothing changes it once taken back");
        assert_eq!(channel.history().await.len(), 1, "it stays in its place");

        // Only an agent is asked to answer again, and only for its own message.
        let mut routed = channel.observe();
        channel.process_envelope(from(alice_id, ActorEvent::RetryRequested { target: RetryTarget::Message(said.id), author: alice_id }), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::RetryRequested { target: RetryTarget::Message(said.id), author: ActorId::Agent(caelumora_model::agent::AgentId::new()) }), &actors).await;
        // An answer under way only of an agent of the channel.
        channel.process_envelope(from(alice_id, ActorEvent::RetryRequested { target: RetryTarget::Turn(TurnId::new()), author: alice_id }), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::RetryRequested { target: RetryTarget::Turn(TurnId::new()), author: ActorId::Agent(caelumora_model::agent::AgentId::new()) }), &actors).await;
        // A nudge only of someone else in the channel (issue 278).
        channel.process_envelope(from(alice_id, ActorEvent::NudgeRequested { participant: alice_id }), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::NudgeRequested { participant: ActorId::Agent(caelumora_model::agent::AgentId::new()) }), &actors).await;
        assert!(routed.try_recv().is_err(), "neither a user's message nor one another said, nor a turn of someone who is no agent here, nor a nudge of oneself or of a stranger");
    }

    #[tokio::test]
    async fn a_user_changes_only_their_own_message_and_learns_why_not() {
        use caelumora_model::actor::EventContext;
        use caelumora_model::security::OwnershipSecurityManager;
        let (alice, bob) = (user("alice"), user("bob"));
        let channel = channel_with(&[&alice, &bob], Arc::new(OwnershipSecurityManager::new())).await;
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(bob.id));
        let said = Message { id: MessageId::new(), sender_id: alice_id, role: caelumora_model::message::MessageRole::User, content: Some("helo".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let from = |sender: ActorId, event| Envelope::new(SecurityContext::new(sender, Default::default(), Default::default()), EventContext { sender_id: sender, ..EventContext::default() }, event);
        let (to_alice, mut alices) = mpsc::channel(16);
        let (to_bob, mut bobs) = mpsc::channel(16);
        let running = |id: ActorId, tx| (id, RunningActor { id, abort_handle: tokio::spawn(async {}).abort_handle(), task: TaskTracker::new(), sender: Some(Box::new(InMemoryEventSender::new(tx))), leaving: None });
        let actors = HashMap::from([running(alice_id, to_alice), running(bob_id, to_bob)]);
        let rejected = |rx: &mut mpsc::Receiver<Envelope>| {
            let mut rejections = Vec::new();
            while let Ok(envelope) = rx.try_recv() {
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
        let above = Classification::general(caelumora_model::confidentiality::Confidentiality::Confidential);
        let modified = |content: &str, classification: &Classification| ActorEvent::MessageModified(Message { content: Some(content.to_string()), classification: classification.clone(), ..said.clone() });
        let reclassified = |classification: &Classification| ActorEvent::MessageReclassified { message: said.id, classification: classification.clone() };
        let revoked = |message| ActorEvent::MessageRevoked { message, at: chrono::Utc::now() };
        channel.process_envelope(from(bob_id, modified("bob's", &Classification::CHANNEL_DEFAULT)), &actors).await;
        channel.process_envelope(from(bob_id, revoked(said.id)), &actors).await;
        channel.process_envelope(from(bob_id, reclassified(&Classification::PUBLIC)), &actors).await;
        let kind = |rejected: &str, reason| (rejected.to_string(), Some(said.id), reason);
        assert_eq!(rejected(&mut bobs), [kind("MessageModified", Rejection::Forbidden), kind("MessageRevoked", Rejection::Forbidden), kind("MessageReclassified", Rejection::Forbidden)], "not his");
        let never = MessageId::new();
        channel.process_envelope(from(alice_id, revoked(never)), &actors).await;
        channel.process_envelope(from(alice_id, modified("hello", &above)), &actors).await;
        channel.process_envelope(from(alice_id, reclassified(&above)), &actors).await;
        let secret = Message { id: MessageId::new(), ..Message::new(above.clone()) };
        channel.process_envelope(from(alice_id, ActorEvent::CompleteMessageGenerated(secret.clone())), &actors).await;
        assert_eq!(rejected(&mut alices), [
            ("MessageRevoked".to_string(), Some(never), Rejection::MessageNotFound),
            kind("MessageModified", Rejection::ClassificationRefused),
            kind("MessageReclassified", Rejection::ClassificationRefused),
            ("CompleteMessageGenerated".to_string(), Some(secret.id), Rejection::ClassificationRefused),
        ], "never said, and never above the channel - the flow, not her permission");
        assert!(rejected(&mut bobs).is_empty(), "nor is anyone else told");
        channel.process_envelope(from(alice_id, revoked(said.id)), &actors).await;
        channel.process_envelope(from(alice_id, modified("hello", &Classification::CHANNEL_DEFAULT)), &actors).await;
        assert_eq!(rejected(&mut alices), [kind("MessageModified", Rejection::MessageUnchangeable)], "taken back, it stays so");

        // A request to answer again names an agent taking part, of a
        // message as of a turn (issue 362): of one that left, its sender is
        // told, rather than it being dropped unseen.
        let gone = ActorId::Agent(AgentId::new());
        let its = Message { id: MessageId::new(), sender_id: gone, role: MessageRole::Assistant, content: Some("said before it left".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        channel.process_envelope(from(gone, ActorEvent::CompleteMessageGenerated(its.clone())), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::RetryRequested { target: RetryTarget::Message(its.id), author: gone }), &actors).await;
        channel.process_envelope(from(alice_id, ActorEvent::RetryRequested { target: RetryTarget::Turn(TurnId::new()), author: gone }), &actors).await;
        assert_eq!(rejected(&mut alices), [
            ("RetryRequested".to_string(), Some(its.id), Rejection::Invalid),
            ("RetryRequested".to_string(), None, Rejection::Invalid),
        ], "an agent that left answers nothing again");

        // Her actor looks at what it can see at once, and asks nothing of the
        // policy: that is the channel's.
        let (hers, his) = (channel.user_actor(alice.id).unwrap(), channel.user_actor(bob.id).unwrap());
        let error = |result: anyhow::Result<uuid::Uuid>| result.expect_err("refused").downcast::<ChannelError>().expect("a channel's error");
        his.reclassify_message(MessageId::new(), Classification::PUBLIC).await.expect_err("never said");
        assert!(matches!(error(hers.revoke_message(MessageId::new()).await), ChannelError::MessageNotFound { .. }), "never said");
        assert!(matches!(error(hers.modify_message(said.id, "hello".to_string(), None).await), ChannelError::MessageRevoked { .. }), "taken back");
        assert!(matches!(error(hers.reclassify_message(said.id, Classification::PUBLIC).await), ChannelError::MessageRevoked { .. }), "with nothing left to release");
        assert!(matches!(error(his.request_retry(said.id).await), ChannelError::MessageRevoked { .. }));
        assert!(matches!(error(his.request_retry(MessageId::new()).await), ChannelError::MessageNotFound { .. }));
        assert!(matches!(error(his.request_retry_of_turn(alice_id, TurnId::new()).await), ChannelError::NotAParticipant { .. }), "no agent answers here");
    }

    #[tokio::test]
    async fn a_removal_is_announced_in_the_channel_in_the_name_of_who_removed_after_the_farewell() {
        let Stuck { channel, context, agent, helper, mut events, running } = stuck_in_a_model_call().await;

        channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");

        // What a frontend needs to end the turn it shows: who is gone -
        // announced once the actor's task has ended, after the end of its
        // turn, which it tells itself (issue 262); nothing of it follows.
        let (removed, farewell_first) = tokio::time::timeout(Duration::from_secs(5), async {
            let mut farewell = false;
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == ActorId::Agent(agent) && matches!(envelope.event, ActorEvent::TurnGivenUp { .. }) {
                    farewell = true;
                }
                if let ActorEvent::ParticipantRemoved { participant } = envelope.event {
                    break ((envelope.context.sender_id, envelope.context.sender_name, participant), farewell);
                }
            }
        }).await.expect("the removal is announced");
        assert_eq!(removed, (context.caller_id, "alice".to_string(), ActorId::Agent(agent)));
        assert!(farewell_first, "the agent's farewell precedes the announcement");
        settled(&helper, ActorState::Idle).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        while let Ok(envelope) = events.try_recv() {
            assert_ne!(envelope.context.sender_id, ActorId::Agent(agent), "nothing of it follows the announcement: {}", envelope.describe());
        }
        running.abort();
    }

    #[tokio::test]
    async fn nothing_of_a_removed_participant_enters_the_channel_once_its_task_has_ended() {
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(DummySecurityManager::new())).await;
        let mut events = channel.observe();
        let gone = ActorId::Agent(caelumora_model::agent::AgentId::new());
        let late = Envelope::new(SecurityContext::default(), EventContext::new(channel.session_id, channel.id, gone, "helper".to_string()), ActorEvent::TurnGivenUp { turn_id: caelumora_model::work::TurnId::new(), reason: caelumora_model::turn::GivenUpReason::Failed { message: "too late".to_string() } });

        // Its actor no longer runs, nor is it leaving: it was aborted.
        channel.handle_actor_event(Some(late), &HashMap::new()).await;

        assert!(events.try_recv().is_err(), "what a removed participant sends is dropped");
    }

    /// An actor told to stop whose task has not ended: no inbox, and who
    /// removed it.
    fn leaving_actor(id: ActorId, remover: SecurityContext) -> RunningActor {
        RunningActor { id, abort_handle: tokio::spawn(async {}).abort_handle(), task: TaskTracker::new(), sender: None, leaving: Some(remover) }
    }

    #[tokio::test]
    async fn what_a_leaving_actor_sends_enters_the_channel_until_its_task_has_ended() {
        let alice = user("alice");
        let channel = channel_with(&[&alice], Arc::new(DummySecurityManager::new())).await;
        let mut events = channel.observe();
        let going = ActorId::Agent(caelumora_model::agent::AgentId::new());
        let turn_id = caelumora_model::work::TurnId::new();
        let farewell = Envelope::new(SecurityContext::default().next(going, ActorId::None, ActorId::None), EventContext::new(channel.session_id, channel.id, going, "helper".to_string()), ActorEvent::TurnGivenUp { turn_id, reason: caelumora_model::turn::GivenUpReason::Stopped });

        // Told to stop, its task still runs: what it says is its own (issue 262).
        let running = HashMap::from([(going, leaving_actor(going, SecurityContext::default()))]);
        channel.handle_actor_event(Some(farewell), &running).await;

        let heard = events.try_recv().expect("the farewell enters the channel");
        assert!(matches!(heard.event, ActorEvent::TurnGivenUp { turn_id: id, .. } if id == turn_id));
        assert_eq!(heard.context.sender_id, going);
    }

    #[tokio::test]
    async fn a_removal_still_pending_is_announced_before_an_actor_starts_under_the_same_id() {
        let (channel, context, agent, _) = agent_channel(Arc::new(DummySecurityManager::new()), Arc::new(NeverAnswers), Default::default()).await;
        let mut events = channel.observe();
        let participant = ActorId::Agent(agent);
        let joined = channel.actors.load().get(&participant).cloned().expect("the agent takes part");

        // Its old actor's task has ended - `add_participant` waited for it -
        // but `run` has not seen that yet when the new actor's start comes.
        let (event_tx, _event_rx) = mpsc::channel(10);
        let mut join_set = JoinSet::new();
        let mut running = HashMap::from([(participant, leaving_actor(participant, context.clone()))]);
        channel.apply_change(ActorChange::Start(joined), &mut running, &event_tx, &mut join_set).await;

        let announced = events.try_recv().expect("the removal is announced");
        assert!(matches!(announced.event, ActorEvent::ParticipantRemoved { participant: p } if p == participant), "{}", announced.describe());
        assert_eq!(announced.context.sender_id, context.caller_id, "in the remover's name");
        assert!(running.get(&participant).is_some_and(|actor| actor.leaving.is_none()), "and then the new actor runs");
        join_set.abort_all();
    }

    #[tokio::test]
    async fn a_cancelled_channel_delivers_what_is_said_but_asks_nobody_until_it_is_asked_to_wake() {
        use caelumora_model::actor::EventContext;
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let bob = ActorId::Agent(caelumora_model::agent::AgentId::new());
        let channel = channel_moderated_by(&[&alice], Arc::new(AskBack(bob))).await;
        let (tx, mut rx) = mpsc::channel(8);
        let abort_handle = tokio::spawn(async {}).abort_handle();
        let actors = HashMap::from([(bob, RunningActor { id: bob, abort_handle, task: TaskTracker::new(), sender: Some(Box::new(InMemoryEventSender::new(tx))), leaving: None })]);
        let from_alice = |event| Envelope::new(
            context.clone(),
            EventContext { sender_id: ActorId::User(alice.id), ..EventContext::default() },
            event,
        );
        let said = || from_alice(ActorEvent::TurnFinished { turn_id: caelumora_model::work::TurnId::new(), messages: vec![Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)] });
        let delivered = |rx: &mut mpsc::Receiver<Envelope>| {
            let mut kinds = Vec::new();
            while let Ok(envelope) = rx.try_recv() { kinds.push(envelope.event.kind()); }
            kinds
        };

        channel.cancel(&context).await.expect("permitted");
        channel.route_event(&actors, said()).await;
        assert_eq!(delivered(&mut rx), ["TurnFinished"], "what was said still arrives, but nobody is asked");

        // A message alone does not wake it: its sender asks for that, as a
        // user's actor does with every message (issue 90).
        channel.route_event(&actors, from_alice(ActorEvent::WakeupRequested)).await;
        delivered(&mut rx); // this test's moderator forwards the request, too
        channel.route_event(&actors, said()).await;
        assert_eq!(delivered(&mut rx), ["TurnFinished", "ResponseRequested"], "woken, the channel asks again");
    }

    #[tokio::test]
    async fn cancelling_one_actor_ends_its_turn_alone_and_keeps_the_channel_answering() {
        use caelumora_model::actor::EventContext;
        use caelumora_model::approval::{ApprovalStrategy, ApproverId};
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let (cancelled, other) = (sleeper(), sleeper());
        let channel = channel_moderated_by(&[&alice], Arc::new(AskBack(other.id))).await;
        for actor in [&cancelled, &other] {
            channel.adopt(Arc::<Sleeper>::clone(actor), Member::Approver(Approver { id: ApproverId::new(), strategy: ApprovalStrategy::Any })).await;
        }

        channel.cancel_actor(&context, cancelled.id).await.expect("permitted");
        assert!(cancelled.cancelled.load(Ordering::SeqCst), "its turn is ended");
        assert!(!other.cancelled.load(Ordering::SeqCst), "the other's goes on");

        let said = Envelope::new(
            context.clone(),
            EventContext { sender_id: ActorId::User(alice.id), ..EventContext::default() },
            ActorEvent::TurnFinished { turn_id: caelumora_model::work::TurnId::new(), messages: vec![Message::new(Classification::CHANNEL_DEFAULT)] },
        );
        channel.route_event(&HashMap::new(), said).await;
        assert!(other.woken.load(Ordering::SeqCst), "the channel is not quiet: the next message is answered");

        let stranger = ActorId::Agent(caelumora_model::agent::AgentId::new());
        let error = channel.cancel_actor(&context, stranger).await.expect_err("not in the channel");
        assert!(matches!(error.downcast_ref::<ChannelError>(), Some(ChannelError::NotAParticipant { .. })), "{error}");
        let error = channel.wakeup_actor(&context, stranger).await.expect_err("nor to be woken");
        assert!(matches!(error.downcast_ref::<ChannelError>(), Some(ChannelError::NotAParticipant { .. })), "the same error: {error}");
    }
}
