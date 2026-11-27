//! The run loop of a channel: starting and stopping its actors, admitting, routing, recording and
//! appending to the history what they send.

use super::inbox::{Behind, Inbox, InboxReceiver, CAPACITY};
use super::{joined_in, stopped, ActorChange, ChannelActor, ChannelLoop, Request, ThreadChange};
use crate::actor::ChannelAnswering;
use crate::moderator::Routing;
use crate::transport::{InMemoryEventSender, OriginEventSender};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use caelumora_api::actor::Answering;
use caelumora_api::logging::envelope_span;
use caelumora_domain::actor::{ActorEvent, ActorId, Envelope, Rejection, RetryTarget};
use caelumora_domain::chain::Position;
use caelumora_domain::channel::{ChannelState, Circle, Said, ThreadId, ThreadState};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::history::{Audience, Heard, Reader, Recorded};
use caelumora_domain::message::Message;
use caelumora_domain::store::StoreResult;
use caelumora_security::confidentiality::ConfidentialityQuestions;
use caelumora_security::{ActionType, Enforcement, Reclassified, SecurityContext};
use std::collections::{BTreeSet, HashMap};
use std::convert::Infallible;
use std::iter;
use std::sync::{self, Arc, Weak};
use tokio::sync::mpsc;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;
use tokio::task::{AbortHandle, JoinSet};
use tokio_util::task::TaskTracker;
use tracing::{debug, error, info, trace, warn, Instrument};

/// How many envelopes the actors of a channel may have sent it that it has yet to take: whoever
/// sends one more waits until it takes them (issue 626).
const INPUT_CAPACITY: usize = 100;

/// The task of a running actor and its inbox. Owned by `run`, which keeps
/// them apart from `ChannelLoop::actors`: that table says who takes part and
/// is read from anywhere; this one is `run`'s, whose `JoinSet` owns the
/// tasks - so that they end with the channel's own task - and which starts and
/// stops them as `ActorChange`s tell it (issue 91).
pub(super) struct RunningActor {
    id: ActorId,
    abort_handle: AbortHandle,
    /// What to wait on for the task to end (`ChannelActor::task`).
    task: TaskTracker,
    /// Its inbox - none once it was told to stop: nothing is delivered to
    /// an actor that is leaving, though what it sends is still taken.
    inbox: Option<Inbox>,
    /// How it leaves, once it was told to stop (issue 262): its task still runs - the step it was
    /// in, its farewell - and `run` announces the removal once the task has ended.
    leaving: Option<Leaving>,
    /// Whom `run` answers once its task has ended, with every other actor it was asked to wait for
    /// with it (`ActorChange::End`): each answered as the last of them has ended.
    ended: Vec<Arc<Ended>>,
}

/// Whom `run` answers once every actor it was asked to wait for (`ActorChange::End`) has ended:
/// held by each of them while its task runs - what it sends until then is routed first -, and
/// answered as the last task ends, by itself or aborted after `STOP_GRACE` (`let_go`); at once if
/// none runs. Never answered if `run` ends first, as no change under way then is.
struct Ended(oneshot::Sender<()>);

impl Ended {
    /// Lets go of `ended`, held by an actor whose task has ended: whoever asked is answered once
    /// the last lets go of it.
    fn let_go(ended: Arc<Self>) {
        if let Some(Ended(carried_out)) = Arc::into_inner(ended) {
            let _ = carried_out.send(());
        }
    }
}

/// How an actor that was told to stop leaves the channel: on whose behalf it was removed, in whose
/// name `run` announces it (issue 262), what it was called as it left, the position from which on
/// nothing was delivered to it (issue 417), the threads it took part in, in each of which it is
/// announced (issues 557, 602), and whom `run` answers once it announced it - each who asked for
/// its leave while it was leaving (issue 593).
struct Leaving {
    remover: SecurityContext,
    name: String,
    until: Position,
    threads: BTreeSet<ThreadId>,
    carried_out: Vec<oneshot::Sender<()>>,
}

/// `first` and what was queued behind it as it was taken, to be handled one after the other (issue
/// 626): each envelope taken in by the last one taken of its sender if it is the next piece of the
/// same partial message (`Envelope::absorb`), so that the pieces sent while the channel was busy
/// are admitted, routed and delivered as one. Each sender's envelopes keep their order; between
/// senders, a piece moves ahead of what another sent before it, which it does not depend on. What
/// arrives while they are taken waits for the next round.
fn taken_together(first: Envelope, queue: &mut mpsc::Receiver<Envelope>) -> Vec<Envelope> {
    let mut taken = vec![first];
    for _ in 0..queue.len() {
        let Ok(next) = queue.try_recv() else { break };
        let sender = next.context.sender_id;
        let last = taken.iter_mut().rev().find(|envelope| envelope.context.sender_id == sender);
        if !last.is_some_and(|last| last.absorb(&next)) {
            taken.push(next);
        }
    }
    taken
}

/// Aborts the tasks `aborts` names unless those `tasks` track have ended by themselves within
/// `STOP_GRACE` (`stopped`), logging `warning` then: for actors told to leave or to stop, which
/// `run` keeps routing meanwhile.
fn abort_unless_stopped(tasks: Vec<TaskTracker>, aborts: Vec<AbortHandle>, warning: String) {
    tokio::spawn(async move {
        if !stopped(&tasks).await {
            warn!("{warning}");
            for abort in aborts {
                abort.abort();
            }
        }
    });
}

/// Logs that `run` stopped, however its task ends - aborted, as it is (issue 593).
struct EndOfRun<'a>(&'a ChannelLoop);

impl Drop for EndOfRun<'_> {
    fn drop(&mut self) {
        info!("Channel {}/{} stopped", self.0.channel.name(), self.0.id);
    }
}

impl ChannelLoop {
    /// Runs the channel - starts and ends its actors, routes what they say - until its task is
    /// aborted, as its session stops or it is deleted. Empty, it runs on, for whoever comes in
    /// (issue 593).
    ///
    /// # Errors
    /// If it ran already: it runs once.
    pub async fn run(&self) -> Result<Infallible> {
        let taken = self.requests_rx.lock().unwrap_or_else(sync::PoisonError::into_inner).take();
        let mut requests_rx = taken.ok_or_else(|| anyhow!("Channel {} ran already", self.id))?;
        info!("Channel {}/{} is starting", self.channel.name(), self.id);
        let _end = EndOfRun(self);
        let (event_tx, mut event_rx) = mpsc::channel(INPUT_CAPACITY);
        let mut join_set = JoinSet::new();
        let mut running = HashMap::new();
        // The actors taken in before it ran - as the channel was built, and
        // the founders of a channel just created (issue 420) - are started,
        // and who joined announced, before anything is routed: what one of
        // them says must come after the joins of all who hear it.
        while let Ok(request) = requests_rx.try_recv() {
            self.apply_change(request, &mut running, &event_tx, &mut join_set).await;
        }

        loop {
            tokio::select! {
                // Events before changes and ended tasks (issues 171, 262):
                // what a removed actor sent before it ended is routed before
                // its removal is announced, and before an actor that joins
                // under its id starts.
                biased;
                // Never closed: `run` keeps a sender for the actors joining later.
                Some(event) = event_rx.recv() => {
                    for event in taken_together(event, &mut event_rx) {
                        self.handle_actor_event(event, &running).await;
                    }
                }
                Some(request) = requests_rx.recv() => {
                    self.apply_change(request, &mut running, &event_tx, &mut join_set)
                        .await;
                }
                // Finished tasks are dropped as they end - a removed actor's with its removal
                // announced (issue 262). The last one gone, the channel runs on, empty (issue 593).
                Some(ended) = join_set.join_next_with_id(), if !join_set.is_empty() => {
                    let task = match &ended {
                        Ok((task, _)) => *task,
                        Err(e) => e.id(),
                    };
                    let ended = running.iter().find(|(_, actor)| actor.abort_handle.id() == task).map(|(id, _)| *id);
                    if let Some(actor) = ended.and_then(|id| running.remove(&id)) {
                        self.task_ended(actor, &running).await;
                    }
                }
            }
        }
    }

    /// Starts an actor taken in, as the channel was built or as someone joined, or stops one that
    /// left (issue 46); records a thread opened, or closes one (issue 557) - whoever closes it
    /// holds it, so that nobody is still leaving it; ends every actor (`end`); answers `request`
    /// once it carried its change out (issue 593).
    async fn apply_change(
        &self,
        request: Request,
        running: &mut HashMap<ActorId, RunningActor>,
        event_tx: &Sender<Envelope>,
        join_set: &mut JoinSet<ActorId>,
    ) {
        let Request { change, carried_out } = request;
        match change {
            ActorChange::Start(started, from, joined) => {
                // Removed again before `run` got to start it: never started,
                // so that it cannot run next to whoever joins under its id
                // after it (issue 171).
                let id = started.actor.id();
                let still_taken_in = self
                    .actors
                    .load()
                    .get(&id)
                    .is_some_and(|current| Arc::ptr_eq(&current.actor, &started.actor));
                // Its old actor has ended - whoever let it go waited for that, holding the
                // participant (`left`) - but its task's end may not have been seen yet: the
                // removal is announced before anything of the new actor (issue 262).
                if running.get(&id).is_some_and(|old| old.leaving.is_some()) {
                    if let Some(old) = running.remove(&id) {
                        self.task_ended(old, running).await;
                    }
                }
                // Who joined is announced before the newcomer runs, in each thread it takes part
                // in (issue 602), so that its join is recorded - and with it where its view of the
                // history begins - before it reads (issue 223); also if it was removed again
                // before it got to run: it was added, and its removal follows. Right before its
                // join in the thread it joined in, the part of the history it was given, to it
                // alone and in the system's name, if it was given anything before its join (issue
                // 417). An actor that starts again with its channel joins nothing.
                let end = self.state.lock().await.history.end();
                if let Some(adder) = joined.clone() {
                    let threads = BTreeSet::clone(&started.threads.load());
                    let joined_in = joined_in(self.id, &threads);
                    if let Some(from) = from.filter(|from| *from < end) {
                        let shared = ActorEvent::HistoryShared {
                            participant: id,
                            from,
                            by: adder.caller_id,
                            by_name: self.name_of(adder.caller_id).await,
                        };
                        let system = SecurityContext::system();
                        self.announce(shared, system, joined_in, running).await;
                    }
                    let name = started.actor.name().to_string();
                    let first = iter::once(joined_in);
                    for thread in first.chain(threads.into_iter().filter(|t| *t != joined_in)) {
                        let joined = ActorEvent::ParticipantJoined {
                            participant: id,
                            name: name.clone(),
                            into_channel: thread == joined_in,
                        };
                        self.announce(joined, adder.clone(), thread, running).await;
                    }
                }
                if still_taken_in {
                    let heard = Self::opening(
                        started.actor.heard().await,
                        joined.is_some().then(|| from.unwrap_or(end)),
                        end,
                    );
                    let handle =
                        self.spawn_actor(&started, heard, event_tx.clone(), join_set).await;
                    running.insert(handle.id, handle);
                }
                let _ = carried_out.send(());
            }
            ActorChange::Leave(id, name, remover, threads) => {
                // Nothing is delivered to it from here on (issue 417), in every thread its actor
                // takes part in now: one may have drawn it in while it was let go (issue 602).
                let until = self.state.lock().await.history.end();
                let threads = BTreeSet::clone(&threads.load());
                let carried_out = vec![carried_out];
                let leaving = Leaving { remover, name, until, threads, carried_out };
                self.take_leave(id, leaving, running).await;
            }
            ActorChange::Thread(id, name, thread, change, by) => {
                if self.change_thread(id, name, thread, change, by, running).await {
                    let _ = carried_out.send(());
                }
            }
            ActorChange::DrawIn(thread, drawn, by) => {
                self.draw_in(thread, drawn, &by, running).await;
                let _ = carried_out.send(());
            }
            ActorChange::End => self.wait_for_end(carried_out, running),
            // A thread opened or closed (issue 557) is of the whole channel: announced in its
            // line, and recorded among the threads open with its entry, which stores them.
            ActorChange::Open(thread, opening, by) => {
                let origin = opening.origin;
                let state = ThreadState { opening: Some(opening), closed: false, delegation: None };
                self.state.lock().await.threads.insert(thread, state);
                let opened = ActorEvent::ThreadOpened { thread, origin };
                self.announce(opened, by, ThreadId::line(self.id), running).await;
                let _ = carried_out.send(());
            }
            // Its delegate goes as it is closed (issue 557): it has nothing more to give, and what
            // was open is dropped. Answered either way; left open while someone takes part, whom
            // it drew in since closing it was asked (issue 602).
            ActorChange::Close(thread, by) => {
                if !self.runs_anyone_in(thread, &[]) {
                    self.let_delegate_go(thread, running).await;
                    self.close(thread, by, running).await;
                }
                let _ = carried_out.send(());
            }
            // Answered only if the brief was taken (issue 598): the delegating caller asks why not.
            ActorChange::Delegate(thread, delegation) => {
                if self.hand_to_delegate(thread, delegation).await {
                    let _ = carried_out.send(());
                }
            }
        }
    }

    /// Has `id` leave as `leaving` says (`ActorChange::Leave`). Dropping its inbox closes it; told
    /// to leave (`Actor::leave`), the actor ends its task itself once its state is stored
    /// consistent. Only if it does not - a tool that hangs - is it aborted. It keeps running until
    /// then: what it sends is taken (`handle_actor_event`), and the removal is announced once its
    /// task has ended (issue 262). The moderators of the threads it leaves ask it no more, also of
    /// one it was drawn into while it was let go (issue 602). An actor never started has nothing to
    /// say: announced at once.
    async fn take_leave(
        &self,
        id: ActorId,
        mut leaving: Leaving,
        running: &mut HashMap<ActorId, RunningActor>,
    ) {
        let Some(handle) = running.get_mut(&id) else {
            self.announce_removal(id, leaving, running).await;
            return;
        };
        for there in leaving.threads.iter().filter_map(|thread| self.thread(*thread)) {
            there.remove_actor(id);
        }
        handle.inbox = None;
        // Asked for again while it was leaving: announced once, as asked last, and answered to
        // both.
        if let Some(earlier) = handle.leaving.take() {
            leaving.carried_out.extend(earlier.carried_out);
        }
        handle.leaving = Some(leaving);
        abort_unless_stopped(
            vec![handle.task.clone()],
            vec![handle.abort_handle.clone()],
            format!("Actor {id} did not stop by itself after leaving channel {}: aborted", self.id),
        );
        info!("Actor {} is stopping: it left channel {}", id, self.id);
    }

    /// Has the running actor `id`, called `name`, take part in `thread`, or no longer, as `change`
    /// says, on behalf of `by` (issue 602): right where its join or its leave is announced there
    /// in the name of `by`'s caller - between the events `run` routes, so that what is delivered
    /// to it of the thread and its attendance there begin and end at one point. Whether it takes
    /// part there so now - nothing is announced for an actor that did already, drawn in
    /// meanwhile; `false` for an actor that ended meanwhile, or a thread closed.
    async fn change_thread(
        &self,
        id: ActorId,
        name: String,
        thread: ThreadId,
        change: ThreadChange,
        by: SecurityContext,
        running: &HashMap<ActorId, RunningActor>,
    ) -> bool {
        let until = self.state.lock().await.history.end();
        let Some(changed) = self.take_part(id, thread, change) else { return false };
        if changed {
            let changed = match change {
                ThreadChange::Joins => {
                    ActorEvent::ParticipantJoined { participant: id, name, into_channel: false }
                }
                ThreadChange::Leaves => ActorEvent::ParticipantLeft {
                    participant: id,
                    name,
                    until,
                    out_of_channel: false,
                },
            };
            self.announce(changed, by, thread, running).await;
        }
        true
    }

    /// Takes into `thread` whoever of `drawn` it draws in (issue 602), on behalf of `by`: where its
    /// circle is open, each whose actor runs in the channel - started and not leaving -, takes no
    /// part there yet, may take part in several threads (`Actor::takes_part_in_several_threads`)
    /// and takes part in a thread whose circle is open, its join announced in the name of `by`'s
    /// caller (`change_thread`). Asks nothing: what draws them in - something said there, its
    /// opening - was asked already.
    async fn draw_in(
        &self,
        thread: ThreadId,
        drawn: Vec<ActorId>,
        by: &SecurityContext,
        running: &HashMap<ActorId, RunningActor>,
    ) {
        if self.thread(thread).is_none_or(|there| there.circle() == Circle::Closed) {
            return;
        }
        for id in drawn {
            // One taken in whose start `run` has not taken in yet would be asked without an inbox.
            if running.get(&id).is_none_or(|handle| handle.leaving.is_some()) {
                continue;
            }
            let Some(drawn) = self.actors.load().get(&id).cloned() else { continue };
            // Who takes part in closed circles alone - a sub-agent - stays in them.
            let closed = |thread: &ThreadId| {
                self.thread(*thread).is_some_and(|there| there.circle() == Circle::Closed)
            };
            if !drawn.actor.takes_part_in_several_threads()
                || drawn.threads.load().iter().all(closed)
            {
                continue;
            }
            let name = drawn.actor.name().to_string();
            self.change_thread(id, name, thread, ThreadChange::Joins, by.clone(), running).await;
        }
    }

    /// Waits for every actor that runs to end (`ActorChange::End`), told by whoever asked
    /// (`ChannelLoop::end_actors`): each ends its task itself once its state is stored consistent,
    /// and only one that does not, as a tool that hangs, is aborted after `STOP_GRACE`. Until then
    /// what it sends is taken and routed (`handle_actor_event`), before its task's end is seen:
    /// `carried_out` is answered once the last has ended (`Ended`), at once if none runs.
    fn wait_for_end(
        &self,
        carried_out: oneshot::Sender<()>,
        running: &mut HashMap<ActorId, RunningActor>,
    ) {
        let ended = Arc::new(Ended(carried_out));
        let mut tasks = Vec::with_capacity(running.len());
        let mut aborts = Vec::with_capacity(running.len());
        for handle in running.values_mut() {
            handle.ended.push(Arc::clone(&ended));
            tasks.push(handle.task.clone());
            aborts.push(handle.abort_handle.clone());
        }
        Ended::let_go(ended);
        let warning =
            format!("Not every actor of channel {} ended by itself: the rest is aborted", self.id);
        abort_unless_stopped(tasks, aborts, warning);
    }

    /// Takes in that the task of `actor`, out of `running` now, has ended: its removal announced
    /// if it was leaving (issue 262), and whoever waits for it to end with others answered once
    /// the last has (`Ended`).
    async fn task_ended(&self, actor: RunningActor, running: &HashMap<ActorId, RunningActor>) {
        let RunningActor { id, leaving, ended, .. } = actor;
        if let Some(leaving) = leaving {
            self.announce_removal(id, leaving, running).await;
        }
        for ended in ended {
            Ended::let_go(ended);
        }
    }

    /// Closes `thread` on behalf of `by` (issue 557): recorded closed among the channel's threads
    /// with its entry, which stores them, no longer held, and announced in the line in its name.
    /// Whoever closes it holds it (`procedure::thread::Close`), as letting someone go from it does
    /// until they have left (issue 593): what they still sent until their task ended entered it,
    /// and their leave is recorded there.
    async fn close(
        &self,
        thread: ThreadId,
        by: SecurityContext,
        running: &HashMap<ActorId, RunningActor>,
    ) {
        if let Some(state) = self.state.lock().await.threads.get_mut(&thread) {
            state.closed = true;
        }
        self.threads.rcu(|threads| {
            let mut threads = HashMap::clone(threads);
            threads.remove(&thread);
            threads
        });
        let closed = ActorEvent::ThreadClosed { thread };
        self.announce(closed, by, ThreadId::line(self.id), running).await;
    }

    /// Announces that `participant` left as `leaving` says - called its name, in the name of its
    /// remover's caller (`ParticipantLeft`, issue 162), in each of its threads (issues 557, 602) -
    /// between the events `run` routes, once the participant's task has ended: what it sent before
    /// is in the channel, nothing of it follows (issue 262). Nothing was delivered to it from the
    /// position `until` on (issue 417). Whoever asked for the leave is answered then (issue 593).
    /// The leave told where it counts as joined (`joined_in`) is the leave of the channel
    /// (`ParticipantLeft::out_of_channel`, issue 603).
    async fn announce_removal(
        &self,
        participant: ActorId,
        leaving: Leaving,
        running: &HashMap<ActorId, RunningActor>,
    ) {
        let Leaving { remover, name, until, threads, carried_out } = leaving;
        let left_in = joined_in(self.id, &threads);
        for thread in threads {
            let removed = ActorEvent::ParticipantLeft {
                participant,
                name: name.clone(),
                until,
                out_of_channel: thread == left_in,
            };
            self.announce(removed, remover.clone(), thread, running).await;
        }
        for asked in carried_out {
            let _ = asked.send(());
        }
    }

    /// What `actor` is called, as the channel names whoever it announces
    /// something in the name of; its id if it cannot be named.
    pub(super) async fn name_of(&self, actor: ActorId) -> String {
        self.controllers.actor_name(actor).await.unwrap_or_else(|e| {
            warn!("Cannot name {} for channel {}: {}", actor, self.id, e);
            actor.to_string()
        })
    }

    /// Announces `event` in the channel in the name of `by`'s caller, in the thread `thread`: a
    /// change of who takes part (issue 408) in a thread the participant takes part in (issues 557,
    /// 602) - which makes its attendance there -, a thread opened or closed in the line. The
    /// channel's own word, which enters it without `admission` - that lets no actor say it -, is
    /// recorded in its history and told to whom it goes.
    async fn announce(
        &self,
        event: ActorEvent,
        by: SecurityContext,
        thread: ThreadId,
        running: &HashMap<ActorId, RunningActor>,
    ) {
        let name = self.name_of(by.caller_id).await;
        let envelope = Answering::in_channel(self.session_id, self.id, name, &by)
            .in_thread(thread)
            .envelope(event);
        debug!("Announcing {}", envelope.describe());
        self.record_and_route(envelope, &by, running).await;
    }

    async fn handle_actor_event(&self, event: Envelope, running: &HashMap<ActorId, RunningActor>) {
        // An actor that was removed still finishes the step it was in and ends its turn: what it
        // sends until its task has ended enters the channel as its own (issue 262); what arrives
        // after that - its task aborted - does not (issue 162). Its sender is the actor's own: its
        // `OriginEventSender` passes on nothing else (issue 228).
        if !running.contains_key(&event.context.sender_id) {
            debug!("Dropped {}: its sender left channel {}", event.describe(), self.id);
            return;
        }
        let span = envelope_span(&event.context, event.event.kind());
        self.process_envelope(event, running).instrument(span).await;
    }

    async fn process_envelope(&self, event: Envelope, running: &HashMap<ActorId, RunningActor>) {
        match event.event {
            // One per streamed chunk: far finer than per message.
            ActorEvent::PartialMessageGenerated(_)
            | ActorEvent::IntermediatePartialGenerated(_) => {
                trace!("Received {}", event.describe())
            }
            _ => debug!("Received {}", event.describe()),
        }
        // The full envelope carries message content: trace level only.
        trace!("Envelope: {:?}", event);
        let context = self.admitted_in(&event);
        if let Err(reason) = self.admission(&event, &context).await {
            self.follow_refused(&event).await;
            self.reject(&event, reason, running).await;
            return;
        }
        self.enter(event, &context, running).await;
    }

    /// Lets `event`, admitted, enter the channel in `context`, its sender's as the channel built it
    /// to admit the event: its mentions resolved, whoever it draws into its thread taken in there
    /// (issue 602) - of a message heard, whoever says it if of no turn, and whom it mentions by
    /// name (`draw_in`) -, then recorded and routed (`record_and_route`).
    async fn enter(
        &self,
        event: Envelope,
        context: &SecurityContext,
        running: &HashMap<ActorId, RunningActor>,
    ) {
        let event = self.resolve_mentions(event).await;
        // Their joins come before what draws them in, which they hear so - a message heard. A
        // message of a turn draws in no speaker: the turn runs where its agent takes part, or is
        // what remains of a turn cancelled as it was let go from the thread, which would draw it
        // back. Whom it mentions by name it draws in, not everyone a group handle addresses.
        if let ActorEvent::CompleteMessageGenerated(said)
        | ActorEvent::IntermediateMessageGenerated(said) = &event.event
        {
            if said.is_heard() {
                let speaker = said.turn_id.is_none().then_some(event.context.sender_id);
                let drawn = speaker.into_iter().chain(said.mentions.iter().copied()).collect();
                self.draw_in(event.context.thread_id, drawn, context, running).await;
            }
        }
        self.record_and_route(event, context, running).await;
    }

    /// Records `event`, admitted or the channel's own, in `context` - its sender's, as the channel
    /// built it to admit the event, or the one the channel speaks in (issue 516), which the
    /// moderator routes it by -, and routes it by the thread it happens in (issue 557), which
    /// admission made sure the channel has - dropped, unrecorded, if it has not. What its sender's
    /// owner alone sees is recorded with it, and given to nobody else (issue 417): the moderator
    /// decides on what everybody may see of it, and that is what is delivered; of an event with
    /// nothing left to show, nothing is, and its entry went to nobody.
    ///
    /// An answer under way asked for again (issue 235) has its turn ended
    /// first, so that the agent finds it given up as it takes the request
    /// in, and only if it is still the turn under way. A turn given up
    /// already is asked for as it is, if it is the agent's last (issue 259).
    /// Otherwise there is nothing to answer again - the turn ended since its
    /// admission, which asked whether it could be (issue 567) -, and the
    /// request is dropped, unrecorded: its users see the answer end, and
    /// what is not delivered is not recorded, so that the history never
    /// delivers it either (issue 223).
    async fn record_and_route(
        &self,
        event: Envelope,
        context: &SecurityContext,
        running: &HashMap<ActorId, RunningActor>,
    ) {
        let Some(thread) = self.thread(event.context.thread_id) else {
            warn!(
                "Dropped {}: the channel has no thread {}",
                event.describe(),
                event.context.thread_id
            );
            return;
        };
        if let ActorEvent::RetryRequested { target: RetryTarget::Turn(turn), author } = &event.event
        {
            let interrupted = self.actor(*author).is_some_and(|actor| actor.retry_turn(*turn));
            let thread = event.context.thread_id;
            if !interrupted && !self.given_up_last(*author, *turn, thread).await {
                warn!(to = %author, "Dropped {}: turn {} is neither under way nor the last given up", event.describe(), turn);
                return;
            }
        }
        let shown = event.published();
        let routing = match &shown {
            Some(shown) => Some(thread.route(shown, context).await),
            None => None,
        };
        let audience = routing
            .as_ref()
            .map_or_else(|| Audience::Only(Vec::new()), |routing| routing.audience.clone());
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
    async fn spawn_actor(
        &self,
        channel_actor: &ChannelActor,
        heard: Heard,
        actor_output: Sender<Envelope>,
        join_set: &mut JoinSet<ActorId>,
    ) -> RunningActor {
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
        let actor_tx =
            Box::new(OriginEventSender::new(id, Box::new(InMemoryEventSender::new(actor_output))));

        // Tracked inside the `JoinSet`'s task: a task that is aborted counts
        // as ended, too.
        let abort_handle = join_set.spawn(task.track_future(async move {
            let result = actor.run(actor_rx, actor_tx).await;
            if let Err(e) = result {
                error!("Actor {} stopped with an error: {:#}", actor.id(), e)
            }
            id
        }));

        RunningActor {
            id,
            abort_handle,
            task,
            inbox: Some(inbox),
            leaving: None,
            ended: Vec::new(),
        }
    }

    /// Stamps `envelope` as the channel delivers it, live or read from the
    /// history (issue 223): in the channel and its session, one step deeper
    /// in its span than what it was delivered of.
    fn stamp(&self, envelope: &mut Envelope) {
        envelope.context.channel_id = self.id;
        envelope.context.session_id = self.session_id;
        envelope.context.span_depth += 1;
    }

    /// The security context of `sender` as the channel knows it now, for the initiator
    /// `initiator` (issue 516): what its envelopes are admitted and routed in, live and read back
    /// from the history alike (issue 223) - never what the sender says of itself. A user's own; an
    /// agent's with its owner's rights and roles as the controllers hold them, limited by its
    /// personality (`ResolvedParticipant::context_in`); the approver's its own, answered for by the
    /// owner of its channel (issue 341); one without rights if the sender takes no part, or is
    /// none - the channel, the system. The initiator is the one the event names.
    fn security_context_of(&self, sender: ActorId, initiator: ActorId) -> SecurityContext {
        match self.actors.load().get(&sender) {
            Some(actor) => actor.participant.context_in(&self.channel.load(), initiator),
            None => SecurityContext {
                initiator_id: initiator,
                ..SecurityContext::new(ActorId::None, self.session_id, self.id)
            },
        }
    }

    /// The context of `participant` acting for itself, as the channel knows it now
    /// (`security_context_of`): what the approver asks in whether a user may approve (issue 567),
    /// as the channel admits what they send.
    pub(crate) fn context_of(&self, participant: ActorId) -> SecurityContext {
        self.security_context_of(participant, participant)
    }

    /// The context `event` is admitted in: its sender's, as the channel knows it now, for the
    /// initiator the event names (`security_context_of`).
    fn admitted_in(&self, event: &Envelope) -> SecurityContext {
        self.security_context_of(event.context.sender_id, event.context.initiator_id)
    }

    /// Nothing if `issuer` may ask the agent `agent` of this channel for its command `command`
    /// (issue 516); why not otherwise. The agent takes part and carries the command out, and the
    /// issuer may see it (`ShowAgentCommand`) and ask for it (`ExecuteAgentCommand`) - asked here,
    /// at the door, with the attributes of the agent and of the channel, never by the agent. A
    /// command the issuer may not see does not fit the channel as they see it; one they may see
    /// but not ask for is forbidden.
    async fn admit_command(
        &self,
        issuer: &SecurityContext,
        agent: ActorId,
        command: &str,
    ) -> Result<(), Rejection> {
        let (commands, agent) = self.commands_held(agent).await.ok_or(Rejection::Invalid)?;
        let command =
            commands.iter().find(|c| c.name.as_str() == command).ok_or(Rejection::Invalid)?;
        let channel = self.security_attributes();
        let policy = self.policies.access_policy();
        policy
            .require(
                issuer,
                ActionType::ShowAgentCommand { command, agent: &agent, channel: &channel },
                |_| Rejection::Invalid,
            )
            .await?;
        policy
            .require(
                issuer,
                ActionType::ExecuteAgentCommand { command, agent: &agent, channel: &channel },
                |_| Rejection::Forbidden,
            )
            .await
    }

    /// Nothing if `event` may enter the channel; why not otherwise (issue 304) - the channel's one
    /// decision of what enters it, made here, in order, against the channel as it is when the event
    /// is taken in, so that nothing changed between a check and the entry. Actors ask nothing for
    /// what they send; who sent what is not let in is told (`EventRejected`). Whether whoever
    /// sends it may act so is asked before the flow of what it carries. Decided:
    ///
    /// - first, it happens in a thread the channel has (issue 557), else it does not fit the
    ///   channel as it is (`Rejection::Invalid`);
    /// - a message, in a thread whose circle is closed by one taking part there alone (issue 602,
    ///   `Rejection::ClosedCircle`); by the actor that is acting - only who acts puts something
    ///   into the channel -, each part of it, the message, its attachments, its tool results and
    ///   what they gave and showed, as it is classified (`ConfidentialityPolicy::may_publish`,
    ///   issue 133): its sender misclassified what does not flow in; what a result gave or showed
    ///   lies in the channel's key space (issues 282, 512);
    /// - a change of a message (issue 234): the message is in its history, its author - the sender
    ///   the history names - may change it, and it can be changed so as it stands
    ///   (`Message::modify`, `Message::revoke`); a reclassification (issue 239) by its author or
    ///   the channel's owner (`Reclassify`), never above the channel;
    /// - a request to answer again (`RetryMessage`): of a message, not taken back, or of a turn, of
    ///   an agent taking part in the thread it is made in (issues 362, 602), that can be answered
    ///   again - under way, or its last turn given up with nothing after it, in that thread
    ///   (`retryable_turn`, issue 567);
    /// - an answer to an agent's questions (`AnswerQuestion`, issue 165) - whether its sender was
    ///   asked is the agent's to check;
    /// - a user's answer to an approval request (`ApprovalAnswered`), from that user, with their
    ///   part alone - which agent asked, and whether they may approve its calls (`ApproveToolCall`,
    ///   issues 14, 312), the approver asks, which holds the request (issue 567); a decision comes
    ///   from an approver alone;
    /// - what an actor did not take (`EventRefused`, issue 567), from that actor in its own name,
    ///   to someone else in the channel;
    /// - a command for an agent (issue 269) comes from a user, names an agent taking part in the
    ///   thread it is sent in (issue 602) that carries it out, and is one the issuer may see and
    ///   ask for (`admit_command`, issue 516); a nudge (issue 278) comes from a user and names
    ///   someone else taking part in the thread it is sent in;
    /// - something given to read (issue 557) comes from the delegate of a thread opened and gives
    ///   what its delegation says - the open brief into the thread, what came of it back where the
    ///   thread was opened -, in the delegating chain's name, to whom runs there; a text alone, as
    ///   it is classified (`admit_given`);
    /// - what the channel records of a turn besides its messages - where a step of its plan began,
    ///   a command or an activity it carried out, how it ended - and the start of an activity come
    ///   from an agent in its own name (issue 284): nobody else has turns; so does what an agent
    ///   was given to read and a compaction of its context (issue 417), what it was given
    ///   classified so that it may flow in;
    /// - who joined or left is the channel's own word (`announce`, issue 408), never an actor's;
    /// - last, once everything else is admitted, a message said or changed claims its attachments
    ///   (`AttachmentManager::claim`, issue 282): it carries what it carried before or what the
    ///   actor acting uploaded for the channel, never what lies elsewhere nor another's upload. A
    ///   message streamed, or repeated as its turn ends, claims nothing - it is not what the
    ///   history keeps.
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn admission(
        &self,
        event: &Envelope,
        context: &SecurityContext,
    ) -> Result<(), Rejection> {
        // In a thread of the channel, and one still open (issue 557).
        let thread = event.context.thread_id;
        let Some(there) = self.thread(thread) else {
            let state = self.state.lock().await;
            let closed = state.threads.get(&thread).is_some_and(|thread| thread.closed);
            return Err(if closed { Rejection::ThreadClosed } else { Rejection::Invalid });
        };
        let messages = event.event.messages();
        let classification = self.classification();
        let channel = self.security_attributes();
        let forbidden = |_| Rejection::Forbidden;
        // The message said or changed, with what it carried before: what claims attachments.
        let mut said = None;
        match &event.event {
            ActorEvent::CompleteMessageGenerated(message)
            | ActorEvent::IntermediateMessageGenerated(message) => {
                said = Some((message, Vec::new()));
                // One outside it runs and takes no part there; one leaving still says what it
                // says until its task has ended (issue 262).
                let actors = self.actors.load();
                let sender = actors.get(&event.context.sender_id);
                let outside = sender.is_some_and(|actor| !actor.takes_part_in(thread));
                (there.circle() == Circle::Open || !outside)
                    .then_some(())
                    .ok_or(Rejection::ClosedCircle)
            }
            ActorEvent::MessageModified(modified) => {
                let mut message =
                    self.message(modified.id).await.ok_or(Rejection::MessageNotFound)?;
                self.policies
                    .access_policy()
                    .require(
                        context,
                        ActionType::ModifyMessage { author: message.sender_id, channel: &channel },
                        forbidden,
                    )
                    .await?;
                let kept = message.attachments.clone();
                message.modify(modified).then_some(()).ok_or(Rejection::MessageUnchangeable)?;
                said = Some((modified, kept));
                Ok(())
            }
            ActorEvent::MessageRevoked { message, at } => {
                let mut message = self.message(*message).await.ok_or(Rejection::MessageNotFound)?;
                self.policies
                    .access_policy()
                    .require(
                        context,
                        ActionType::RevokeMessage { author: message.sender_id, channel: &channel },
                        forbidden,
                    )
                    .await?;
                message.revoke(*at).then_some(()).ok_or(Rejection::MessageUnchangeable)
            }
            ActorEvent::MessageReclassified { message, classification: new } => {
                let mut said = self.message(*message).await.ok_or(Rejection::MessageNotFound)?;
                let target = Reclassified::Message { author: said.sender_id, attributes: &channel };
                self.policies
                    .access_policy()
                    .require(context, ActionType::Reclassify(target), forbidden)
                    .await?;
                self.policies.confidentiality().require_publish(
                    context,
                    new,
                    &classification,
                    || Rejection::ClassificationRefused,
                )?;
                said.reclassify(new).then_some(()).ok_or(Rejection::MessageUnchangeable)
            }
            ActorEvent::RetryRequested { target: RetryTarget::Message(message), author } => {
                let message = self.message(*message).await.ok_or(Rejection::MessageNotFound)?;
                self.policies
                    .access_policy()
                    .require(context, ActionType::RetryMessage(&channel), forbidden)
                    .await?;
                if message.revoked.is_some() {
                    return Err(Rejection::MessageUnchangeable);
                }
                let there = self.runs_in(*author, event.context.thread_id);
                (message.sender_id == *author && self.agent_actor(*author).is_some() && there)
                    .then_some(())
                    .ok_or(Rejection::Invalid)
            }
            // Whether the turn can be answered again is asked here (issue 567); whether it is still
            // under way as the request is delivered, where the turn is ended (`enter`).
            ActorEvent::RetryRequested { target: RetryTarget::Turn(turn), author } => {
                self.policies
                    .access_policy()
                    .require(context, ActionType::RetryMessage(&channel), forbidden)
                    .await?;
                // Where the turn runs or ran, which the request names (issue 602).
                let there = self.retryable_turn(*author, *turn).await;
                let thread = event.context.thread_id;
                let in_its_thread = there == Some(thread) && self.runs_in(*author, thread);
                (self.agent_actor(*author).is_some() && in_its_thread)
                    .then_some(())
                    .ok_or(Rejection::Invalid)
            }
            ActorEvent::QuestionAnswered { .. } | ActorEvent::QuestionDeclined { .. } => {
                self.policies
                    .access_policy()
                    .require(context, ActionType::AnswerQuestion(&channel), forbidden)
                    .await
            }
            // A person's answer says their part alone (issue 419): who decided is the approver's to
            // say, from their answers - and which agent asked, and whether they may approve its
            // calls, which the approver asks, holding the request (issue 567).
            ActorEvent::ApprovalAnswered { decision, .. } => {
                let sender = event.context.sender_id;
                (sender.as_user_id().is_some() && decision.by == sender)
                    .then_some(())
                    .ok_or(Rejection::Invalid)
            }
            ActorEvent::ApprovalGranted(_)
            | ActorEvent::ApprovalDenied(_)
            | ActorEvent::ApprovalTimedOut(_) => {
                matches!(event.context.sender_id, ActorId::Approver(_))
                    .then_some(())
                    .ok_or(Rejection::Invalid)
            }
            // What an actor did not take it tells whom it was of, someone else in the channel.
            ActorEvent::EventRefused { to, .. } => (*to != event.context.sender_id
                && self.runs(*to))
            .then_some(())
            .ok_or(Rejection::Invalid),
            // Of an agent that takes part in the thread it is issued in (issue 602).
            ActorEvent::CommandRequested { agent, command, .. } => {
                event.context.sender_id.as_user_id().ok_or(Rejection::Invalid)?;
                self.admit_command(context, *agent, command).await?;
                self.runs_in(*agent, event.context.thread_id)
                    .then_some(())
                    .ok_or(Rejection::Invalid)
            }
            // It asks nothing of the policy beyond what a message does - an
            // `@`-mention asks the same -, of someone who takes part in the thread it names (issue
            // 602), whose moderator routes it.
            ActorEvent::NudgeRequested { participant } => {
                (event.context.sender_id.as_user_id().is_some()
                    && *participant != event.context.sender_id
                    && self.runs_in(*participant, event.context.thread_id))
                .then_some(())
                .ok_or(Rejection::Invalid)
            }
            ActorEvent::GivenToRead { recipient, message } => {
                self.admit_given(event, *recipient, message, context, &classification).await
            }
            ActorEvent::MessageGiven { message } => {
                event.context.sender_id.as_agent_id().ok_or(Rejection::Invalid)?;
                self.policies.confidentiality().require_publish(
                    context,
                    &message.classification,
                    &classification,
                    || Rejection::ClassificationRefused,
                )
            }
            ActorEvent::PlanStepStarted { .. }
            | ActorEvent::MessageWithdrawn { .. }
            | ActorEvent::ContextCompacted { .. }
            | ActorEvent::CommandExecuted { turn_id: Some(_), .. }
            | ActorEvent::ActivityStarted { .. }
            | ActorEvent::ActivityFinished { .. }
            | ActorEvent::TurnFinished { .. }
            | ActorEvent::TurnGivenUp { .. } => {
                event.context.sender_id.as_agent_id().map(drop).ok_or(Rejection::Invalid)
            }
            event if event.is_announcement() => Err(Rejection::Invalid),
            _ => Ok(()),
        }?;
        for message in messages {
            self.require_parts(context, message, &classification)?;
        }
        match said {
            Some((message, kept)) => self
                .attachments
                .claim(context.caller_id, self.id, &message.attachments, &kept)
                .then_some(())
                .ok_or(Rejection::Invalid),
            None => Ok(()),
        }
    }

    /// Nothing if `event`, sent in `context`, may give `recipient` `message` to read (issue 557);
    /// why not otherwise. It comes from the delegate of a thread opened - no thread delegates into
    /// the line - and gives what that thread's delegation says (`Delegation`): into the thread, the
    /// open brief to the recipient, unless the thread is quiet; back into the thread it was opened
    /// at, what came of the open brief - answered, refused, its recipient gone -, of the recipient
    /// to the giver; in the name of the chain that delegated (its initiator), to whom runs there;
    /// and the message is a text - no attachment, which only a message said claims, no tool call or
    /// result -, classified so that it may enter the channel (`require_parts`).
    async fn admit_given(
        &self,
        event: &Envelope,
        recipient: ActorId,
        message: &Message,
        context: &SecurityContext,
        classification: &Classification,
    ) -> Result<(), Rejection> {
        let thread = event.context.thread_id;
        let ActorId::Delegate(delegated) = event.context.sender_id else {
            return Err(Rejection::Invalid);
        };
        let Some(origin) = self.origin_of(delegated) else {
            return Err(Rejection::Invalid);
        };
        let as_delegated = {
            let state = self.state.lock().await;
            let held = state.threads.get(&delegated).and_then(|held| held.delegation.as_ref());
            held.is_some_and(|delegation| {
                // A brief into a quiet thread would ask nobody, not even once it is woken: refused,
                // its delegate gives back that the recipient could not be asked (issue 557).
                let gives = if event.is_brief() {
                    delegation.gives_brief(recipient, message) && !self.is_quiet(thread)
                } else {
                    thread == origin.thread && delegation.gives_back(recipient, message)
                };
                gives && event.context.initiator_id == delegation.initiator
            })
        };
        let running_there = self.runs_in(recipient, thread);
        let text = message.attachments.is_empty()
            && message.tool_calls.is_empty()
            && message.tool_results.is_empty();
        if !(as_delegated && running_there && text) {
            return Err(Rejection::Invalid);
        }
        self.require_parts(context, message, classification)
    }

    /// Nothing if every part of `message` - the message, its attachments, its tool results and what
    /// they gave and showed - may enter the channel, classified `classification`, in `context`
    /// (issue 133); why not otherwise. What a result showed its model is one of the channel's
    /// attachments (issue 282), what it gave was kept for the channel (issue 512): both lie in its
    /// key space.
    fn require_parts(
        &self,
        context: &SecurityContext,
        message: &Message,
        classification: &Classification,
    ) -> Result<(), Rejection> {
        let mut seen =
            message.tool_results.iter().flat_map(|result| result.files.iter().chain(&result.shown));
        if !seen.all(|content| self.attachments.lies_in(self.id, content)) {
            return Err(Rejection::Invalid);
        }
        let carried = message.carried().map(|content| &content.classification);
        let parts = iter::once(&message.classification)
            .chain(carried)
            .chain(message.tool_results.iter().map(|r| &r.classification));
        for part in parts {
            self.policies.confidentiality().require_publish(
                context,
                part,
                classification,
                || Rejection::ClassificationRefused,
            )?;
        }
        Ok(())
    }

    /// Tells the sender of `event`, which the channel did not let in, why (`EventRejected`, issue
    /// 304) - it alone, in the channel's name, in the thread of `event` (issue 557); not one that
    /// left, whose inbox is closed. The notice is recorded, to the sender alone, so that it is
    /// shown after a reload (issue 417); what was rejected entered nothing and is not. Of an event
    /// naming a thread the channel does not have, the notice is recorded in that thread, and
    /// reaches its sender all the same: by name.
    async fn reject(
        &self,
        event: &Envelope,
        reason: Rejection,
        running: &HashMap<ActorId, RunningActor>,
    ) {
        warn!("Dropped {}: {}", event.describe(), reason);
        let sender = event.context.sender_id;
        let Some(inbox) = running.get(&sender).and_then(|actor| actor.inbox.as_ref()) else {
            return;
        };
        let mut envelope = Answering::in_channel(
            self.session_id,
            self.id,
            ActorId::System.to_string(),
            &SecurityContext::system(),
        )
        .replying_to(&event.context)
        .envelope(event.rejection(reason));
        envelope.context.position =
            self.append_history(&envelope, Audience::Only(vec![sender])).await;
        inbox.deliver(envelope);
    }

    /// Resolves the `@`-mentions in every complete message carried by `event` and stores them
    /// with it (`Message::resolve_mentions`): whom it mentions by name, and whom it addresses - a
    /// group handle those taking part in the thread it is said in (`taking_part_in`, issue 594):
    /// the event's, and of a modification, which happens in the whole channel, the thread the
    /// message was said in. Done here, once, as the message enters the channel, so that every
    /// frontend, the history, the moderator and the thread it draws into all see the same
    /// resolution (issues 29, 602).
    async fn resolve_mentions(&self, mut event: Envelope) -> Envelope {
        let directory = self.mentions();
        let thread = event.context.thread_id;
        match &mut event.event {
            ActorEvent::CompleteMessageGenerated(m)
            | ActorEvent::IntermediateMessageGenerated(m) => {
                m.resolve_mentions(&directory, &self.taking_part_in(thread));
            }
            ActorEvent::MessageModified(m) => {
                let said_in = self.said_in(m.id).await.map(|(_, thread)| thread);
                let present = said_in.map(|thread| self.taking_part_in(thread));
                m.resolve_mentions(&directory, &present.unwrap_or_default());
            }
            ActorEvent::TurnFinished { messages, .. } => {
                let present = self.taking_part_in(thread);
                messages.iter_mut().for_each(|m| m.resolve_mentions(&directory, &present));
            }
            _ => {}
        }
        event
    }

    async fn route_event(
        &self,
        running: &HashMap<ActorId, RunningActor>,
        event: Envelope,
        routing: Option<Routing>,
        position: Option<Position>,
    ) {
        // Asked for by an actor - a user's actor asks with every message it
        // sends (issue 90); the channel does not decide by itself.
        if matches!(event.event, ActorEvent::WakeupRequested) {
            self.wake(Some(event.context.thread_id));
        }
        // A nudge is deliberate, as `wakeup_actor` is (issue 278), and so is a request to answer
        // again (issue 567): the thread it names is quiet no longer (issues 557, 602), so that the
        // request is delivered, and whom it asks is woken with it, alone (`route_event_impl`).
        if matches!(
            event.event,
            ActorEvent::NudgeRequested { .. } | ActorEvent::RetryRequested { .. }
        ) {
            self.end_quiet(event.context.thread_id);
        }

        if let Some(routing) = routing {
            self.route_event_impl(running, &event, routing, position).await;
        }
        #[cfg(any(test, feature = "test-support"))]
        let _ = self.observed.send(event);
    }

    async fn route_event_impl(
        &self,
        running: &HashMap<ActorId, RunningActor>,
        event: &Envelope,
        routing: Routing,
        position: Option<Position>,
    ) {
        // Whether its thread is quiet after a cancel (issue 557). A brief was let in only into a
        // thread that was not, and asks its recipient as decided then: a cancel meanwhile would
        // leave it given and never asked, its giver waiting for nothing.
        let thread = event.context.thread_id;
        let quiet = !event.is_brief() && self.is_quiet(thread);
        let decision = self.deliveries(event, routing, position, quiet).await;

        // Whom the moderator asks to respond is woken first, before anything
        // of the decision is delivered: a sleeping agent ignores a
        // `ResponseRequested`, and one that took in the message still asleep
        // would ignore the request that follows it. Since the policies never
        // choose a sleeping agent (`actor_candidates`), only the agents a
        // message mentions are woken this way (issue 45). A quiet thread
        // asks nobody, and so wakes nobody.
        if !quiet {
            let actors = self.actors.load();
            let asked = decision.iter().filter(|(target_id, route)| {
                *target_id != event.context.sender_id
                    && matches!(
                        route.event,
                        ActorEvent::ResponseRequested { .. } | ActorEvent::RetryRequested { .. }
                    )
            });
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
            if target_id == event.context.sender_id
                && route.1.context.sender_id == event.context.sender_id
                && !event.event.is_announcement()
            {
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
    /// event's context, with the hops it counted - unless its thread is
    /// `quiet` after a cancel: what was said still reaches everyone, so that
    /// every transcript keeps up with the history, but nobody is asked to
    /// answer it. Each notice and request is recorded, in the system's name,
    /// to whom it goes, before it is stamped and delivered: the history keeps
    /// it as it entered, to be stamped again as it is read from there. Whom
    /// none of it goes back to is `route_event_impl`'s to see to.
    async fn deliveries(
        &self,
        event: &Envelope,
        routing: Routing,
        position: Option<Position>,
        quiet: bool,
    ) -> Vec<(ActorId, Envelope)> {
        let Routing { audience, ask, reason, agent_hops, notices } = routing;
        let deliver_to = match audience {
            Audience::Everybody => {
                // Of what happens in a thread, an agent is delivered what happens in its own
                // (issue 557).
                let (recipients, happened_in) = (event.recipients(), event.thread());
                self.actors
                    .load()
                    .iter()
                    .filter(|(id, actor)| {
                        recipients.goes_to(**id, &actor.threads.load(), happened_in)
                    })
                    .map(|(id, _)| *id)
                    .collect()
            }
            Audience::Only(only) => only,
        };
        let mut delivered = event.clone();
        delivered.context.position = position;
        self.stamp(&mut delivered);
        let mut deliveries: Vec<(ActorId, Envelope)> =
            deliver_to.into_iter().map(|id| (id, delivered.clone())).collect();
        for (recipients, notice) in notices {
            let mut told = event.forward(notice.into());
            told.context.sender_id = ActorId::System;
            told.context.sender_name = ActorId::System.fixed_name().unwrap_or_default().to_string();
            told.context.position =
                self.append_history(&told, Audience::Only(recipients.clone())).await;
            self.stamp(&mut told);
            deliveries.extend(recipients.into_iter().map(|id| (id, told.clone())));
        }
        if quiet {
            if !ask.is_empty() {
                debug!("Not asking for a response: the thread is quiet after a cancel");
            }
        } else if !ask.is_empty() {
            let mut request = event.forward(ActorEvent::ResponseRequested { reason });
            request.context.agent_hops = agent_hops;
            request.context.position =
                match Recorded::of(&request, ActorId::System, Audience::Only(ask.clone())) {
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
    /// Stored at once; what others hear of a message in the line, or of one
    /// mentioning a user in another thread, as everybody may see it, is
    /// announced as news once it is stored (issues 404, 557): what a frontend
    /// shows of the channel. Returns the
    /// position of its entry, which what is delivered of it carries; `None`
    /// if nothing was recorded.
    async fn append_history(&self, envelope: &Envelope, audience: Audience) -> Option<Position> {
        self.record(Recorded::of(envelope, envelope.context.sender_id, audience)?).await
    }

    /// Records `recorded` in the channel's history and stores it (`append_history`); returns its
    /// position, or `None` if it was not recorded - a change of a message that changes nothing.
    /// What a delegate gave is followed in its delegation (`Delegation::follow`), stored with it,
    /// and so is what its giver did with what came back (issue 598): its reading, then its turn's
    /// end or its leave, which let the delegation go - announced once stored
    /// (`channel::DELEGATION_LET_GO`).
    async fn record(&self, recorded: Recorded) -> Option<Position> {
        let (thread, sender) = (recorded.context.thread_id, recorded.sender);
        let in_line = recorded.thread().is_none_or(|of| of == ThreadId::line(self.id));
        let said = recorded
            .entry
            .published()
            .and_then(|entry| entry.said().cloned())
            .filter(|message| (in_line || message.addresses_a_user()) && message.is_heard())
            .map(|message| Said { message, thread });
        let entry = recorded.entry.clone();
        let mut state = self.state.lock().await;
        if !state.history.enter(recorded) {
            return None;
        }
        let position = state.history.end().previous();
        // What a delegate gave, and what its giver did with what came back, followed in the
        // delegation with its entry, stored together, and its delegate in step with it (issues
        // 557, 598).
        let delegated = position.and_then(|at| state.follow_given(&entry, at));
        let read = state.follow_read(sender, &entry);
        let let_go = state.follow_giver(&entry, thread);
        for followed in delegated.into_iter().chain(read).chain(let_go.iter().copied()) {
            if let Some(delegation) = state.delegation(followed) {
                self.sync_delegate(followed, delegation);
            }
        }
        match self.store_state(&mut state).await {
            Ok(()) => {
                if let Some(message) = said {
                    self.channel.announce_said(&message);
                }
                for thread in let_go {
                    self.channel.announce_delegation_let_go(thread);
                }
            }
            Err(e) => warn!("Cannot store state of channel {}: {}", self.id, e),
        }
        position
    }

    /// Follows the channel refusing `event`, if a delegate gave something with it (issue 557,
    /// `ChannelState::follow_refused`): stored, and its delegate in step with it - ahead of the
    /// rejection it is told.
    async fn follow_refused(&self, event: &Envelope) {
        let Some(entry) = event.entry() else { return };
        let mut state = self.state.lock().await;
        let Some(thread) = state.follow_refused(&entry) else { return };
        if let Err(e) = self.store_state(&mut state).await {
            warn!("Cannot store the delegation into thread {}: {}", thread, e);
        }
        if let Some(delegation) = state.delegation(thread) {
            self.sync_delegate(thread, delegation);
        }
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
/// name; a command asked of the actor admitted again, with the issuer's
/// rights as they are now (issue 516), a turn's end with its answer, stamped
/// as what is delivered live.
#[async_trait]
impl Behind for ChannelLoop {
    async fn recorded_for(
        &self,
        actor: ActorId,
        from: Position,
    ) -> Result<(Vec<Envelope>, Position)> {
        let end = self.state.lock().await.history.end();
        let to = from.after(CAPACITY as u64).min(end);
        if from >= to {
            return Ok((Vec::new(), from));
        }
        let reader = Reader::Participant(actor);
        let mut delivered = Vec::new();
        for node in self.history_store.entries(self.id, &reader, from..to).await? {
            let Some(mut envelope) = node.entry.envelope(node.seq) else { continue };
            if envelope.context.sender_id == actor && !envelope.event.is_announcement() {
                continue;
            }
            // A command asked of this agent is admitted again, with the
            // issuer's rights as they are now (issue 516): whoever lost a
            // role meanwhile has no command carried out that the role allowed.
            if let ActorEvent::CommandRequested { agent, command, .. } = &envelope.event {
                let issuer = self.admitted_in(&envelope);
                if *agent == actor && self.admit_command(&issuer, actor, command).await.is_err() {
                    debug!("Left out {}: not admitted again", envelope.describe());
                    continue;
                }
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
    use crate::actor::user::ChannelWatch;
    use crate::actor::Shown;
    use crate::channel::tests::*;
    use crate::channel::{
        require_opening, ActorChange, ChannelError, Ending, ResolvedParticipant, ThreadSpec,
    };
    use crate::moderator::broadcast_policy::BroadcastPolicy;
    use crate::moderator::{Moderator, Notice, ResponderPolicy, Routing};
    use crate::testing::Script;
    use crate::testing::StandIn;
    use crate::testing::{file, shown, World};
    use anyhow::Result;
    use arc_swap::ArcSwap;
    use bytes::Bytes;
    use caelumora_api::actor::Actor;
    use caelumora_api::component::ComponentImpl;
    use caelumora_api::error::AttachmentError;
    use caelumora_api::transport::{EventReceiver, EventSender};
    use caelumora_domain::actor::{
        ActorEvent, ActorId, ActorState, Envelope, EventContext, Rejection, RequestReason,
        RetryTarget, SpanId,
    };
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::channel::{
        Brief, ChannelId, Delegation, Opening, ThreadId, ThreadOrigin, Word,
    };
    use caelumora_domain::confidentiality::{Classification, Confidentiality};
    use caelumora_domain::content::ContentRef;
    use caelumora_domain::history::{HistoryEntry, Recipients};
    use caelumora_domain::message::{Message, MessageId, MessageRole};
    use caelumora_domain::tool::testing::make_tool_call;
    use caelumora_domain::tool::{ToolCall, ToolCallResult, ToolContent};
    use caelumora_domain::turn::GivenUpReason;
    use caelumora_domain::user::testing::make_user;
    use caelumora_domain::user::{User, UserId};
    use caelumora_domain::value::name::Name;
    use caelumora_domain::work::TurnId;
    use caelumora_security::SecurityContext;
    use caelumora_security::{AccessPolicy, AllowAllPolicy};
    use chrono::Utc;
    use std::collections::VecDeque;
    use std::slice;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::task::JoinSet;
    use tokio::time;

    impl ChannelLoop {
        /// `event` admitted in the context the channel builds for its sender, as it is live.
        async fn admit(&self, event: &Envelope) -> Result<(), Rejection> {
            self.admission(event, &self.admitted_in(event)).await
        }

        /// `event` let in, in the context the channel builds for its sender, as it is live.
        async fn let_in(&self, event: Envelope, running: &HashMap<ActorId, RunningActor>) {
            let context = self.admitted_in(&event);
            self.enter(event, &context, running).await;
        }
    }

    /// An inbox the test reads what the channel delivers from, with nothing
    /// behind it.
    fn opened() -> (Inbox, InboxReceiver) {
        Inbox::open(
            ActorId::None,
            |_| true,
            Heard::default(),
            Position::START,
            Weak::<ChannelLoop>::new(),
        )
    }

    /// Routes `event` in `channel` to `running` as the channel does once it
    /// recorded it: decided by the moderator of its thread, then delivered.
    async fn routed(
        channel: &ChannelLoop,
        running: &HashMap<ActorId, RunningActor>,
        event: Envelope,
    ) {
        let thread = channel.thread(event.context.thread_id).expect("its thread");
        let routing = thread.route(&event, &channel.admitted_in(&event)).await;
        channel.route_event(running, event, Some(routing), None).await;
    }

    #[tokio::test]
    async fn a_users_own_message_is_shown_with_its_mentions_resolved() {
        let (alice, bob) = (make_user("alice"), make_user("bob"));
        let channel = channel_with(&[&alice, &bob], Arc::new(AllowAllPolicy::new())).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let mut following = actor.watch();
        let running = tokio::spawn({
            let channel = Arc::clone(&channel);
            async move { channel.run().await }
        });

        actor
            .send_message(None, Some("@bob, look".to_string()), None, Vec::new())
            .await
            .expect("sent");

        let shown = time::timeout(Duration::from_secs(5), following.recv())
            .await
            .expect("shown by its actor, as it forwards it to the channel")
            .unwrap();
        running.abort();
        let ActorEvent::CompleteMessageGenerated(message) = shown.event else {
            panic!("its message, got {}", shown.event.kind())
        };
        assert_eq!(
            message.mentions,
            vec![ActorId::User(bob.id)],
            "as the channel resolves them (issue 220)"
        );
    }

    /// Sends every event back to its sender, once as it is and once as a
    /// `ChainCutOff` of its own, in the name of the system.
    #[derive(Debug)]
    struct TellsTheSender;

    #[async_trait::async_trait]
    impl Moderator for TellsTheSender {
        async fn route(&self, event: &Envelope, _context: &SecurityContext) -> Routing {
            let sender = event.context.sender_id;
            let notice = Notice::ChainCutOff {
                message: MessageId::new(),
                max_agent_hops: 0,
                not_asked: Vec::new(),
            };
            Routing {
                audience: Audience::Only(vec![sender]),
                notices: vec![(vec![sender], notice)],
                ..Routing::default()
            }
        }
        fn add_actor(&self, _: Arc<dyn Actor>) {}
        fn remove_actor(&self, _: ActorId) {}
    }

    #[tokio::test]
    async fn the_sender_gets_back_only_what_the_moderator_sends_in_another_name() {
        let alice = make_user("alice");
        let bob = ActorId::Agent(AgentId::new());
        let channel = channel_moderated_by(&[&alice], Arc::new(TellsTheSender)).await;
        let (tx, mut rx) = opened();
        let abort_handle = tokio::spawn(async {}).abort_handle();
        let actors = HashMap::from([(
            bob,
            RunningActor {
                id: bob,
                abort_handle,
                task: TaskTracker::new(),
                inbox: Some(tx),
                leaving: None,
                ended: Vec::new(),
            },
        )]);
        let said = Envelope::new(
            EventContext { sender_id: bob, ..channel.event_context() },
            ActorEvent::TurnFinished {
                turn_id: TurnId::new(),
                messages: vec![Message::new(Classification::CHANNEL_DEFAULT)],
            },
        );

        routed(&channel, &actors, said).await;
        let mut kinds = Vec::new();
        while let Some(envelope) = rx.try_recv().await {
            kinds.push(envelope.event.kind());
        }
        assert_eq!(
            kinds,
            ["ChainCutOff"],
            "its own message is not sent back, but what the system tells it is (issue 200)"
        );
    }

    #[tokio::test]
    async fn a_user_following_the_channel_is_told_when_a_chain_is_cut_off() {
        use crate::moderator::policy_moderator::{PolicyModerator, DEFAULT_CONTEXT_MESSAGES};
        let alice = make_user("alice");
        let responders = ComponentImpl {
            id: "responders".to_string(),
            fail_policy: Default::default(),
            facet: Arc::new(BroadcastPolicy) as Arc<dyn ResponderPolicy>,
        };
        let moderator = Arc::new(PolicyModerator::new(responders, 1, DEFAULT_CONTEXT_MESSAGES));
        let channel =
            channel_moderated_by(&[&alice], Arc::<PolicyModerator>::clone(&moderator)).await;
        // One hop into a chain between agents, as the answer to a request.
        let talker = ActorId::Agent(AgentId::new());
        let context = EventContext { sender_id: talker, agent_hops: 1, ..channel.event_context() };
        let said = Message {
            sender_id: talker,
            role: MessageRole::Assistant,
            content: Some("and you?".to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let answer = Envelope::new(
            context,
            ActorEvent::TurnFinished { turn_id: TurnId::new(), messages: vec![said] },
        );
        channel
            .adopt(
                Forger::sending(talker, vec![answer]),
                ResolvedParticipant::approver(channel.session_id, channel.id),
                in_thread_alone(ThreadId::line(channel.id)),
                None,
                None,
            )
            .await;
        let mut following = channel.user_actor(alice.id).unwrap().watch();
        let running = tokio::spawn({
            let channel = Arc::clone(&channel);
            async move { channel.run().await }
        });

        let told = time::timeout(Duration::from_secs(5), async {
            loop {
                if let ActorEvent::ChainCutOff { max_agent_hops, .. } =
                    following.recv().await.expect("the channel runs").event
                {
                    break max_agent_hops;
                }
            }
        })
        .await
        .expect("the web UI follows the user's actor, which is told (issues 200, 220)");
        assert_eq!(told, 1);
        running.abort();
    }

    /// The context the channel admits an envelope in is its sender's as the
    /// channel knows it (issue 516): a user's own; an agent's with its
    /// owner's rights, for whoever asked; one without rights for whoever
    /// takes no part. Never what the sender says of itself: an envelope
    /// carries none.
    #[tokio::test]
    async fn an_envelope_is_admitted_in_the_context_the_channel_builds_for_its_sender() {
        let (channel, _, agent, alice) = scripted_channel(Script::Hangs).await;
        let (her, its) = (ActorId::User(alice), ActorId::Agent(agent));
        let hers = channel.security_context_of(her, her);
        assert_eq!(
            (hers.caller_id, hers.owner_id, hers.principal_id, hers.initiator_id),
            (her, her, her, her),
            "her own"
        );
        assert_eq!((hers.session_id, hers.channel_id), (channel.session_id, channel.id));
        let its_own = channel.security_context_of(its, her);
        assert_eq!(
            (its_own.caller_id, its_own.owner_id, its_own.principal_id, its_own.initiator_id),
            (its, her, her, her),
            "the agent's, with its owner's rights, on her word"
        );
        let stranger = ActorId::User(UserId::new());
        let nobody = channel.security_context_of(stranger, stranger);
        assert_eq!(nobody.caller_id, ActorId::None, "takes no part: no rights");
    }

    /// A command asked of an agent is admitted again as the agent reads it
    /// back from the history (issue 516): one it may not be asked for any
    /// more - here, one it does not carry out - is left out, and what else
    /// went to it is delivered.
    #[tokio::test]
    async fn a_command_read_back_from_the_history_is_admitted_again_for_its_agent() {
        let (channel, _, agent, alice) = scripted_channel(Script::Hangs).await;
        let (her, its) = (ActorId::User(alice), ActorId::Agent(agent));
        let from_her = |event| {
            Envelope::new(EventContext { sender_id: her, ..channel.event_context() }, event)
        };
        let command = ActorEvent::CommandRequested {
            command: "mark".to_string(),
            args: String::new(),
            agent: its,
        };
        let said = ActorEvent::CompleteMessageGenerated(Message {
            sender_id: her,
            role: MessageRole::User,
            content: Some("hello".to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        });
        let running = HashMap::new();
        channel.let_in(from_her(command), &running).await;
        channel.let_in(from_her(said), &running).await;

        let (delivered, _) = channel.recorded_for(its, Position::START).await.unwrap();
        let kinds: Vec<_> = delivered.iter().map(|envelope| envelope.event.kind()).collect();
        assert!(
            kinds.contains(&"CompleteMessageGenerated") && !kinds.contains(&"CommandRequested"),
            "the message delivered, the command it does not carry out left out: {kinds:?}"
        );
    }

    #[tokio::test]
    async fn an_actor_cannot_speak_in_the_name_of_another() {
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let talker = ActorId::Agent(AgentId::new());
        let her = ActorId::User(alice.id);
        let said = |sender: ActorId, author: ActorId, text: &str| {
            let message = Message {
                sender_id: author,
                role: MessageRole::Assistant,
                content: Some(text.to_string()),
                ..Message::new(Classification::CHANNEL_DEFAULT)
            };
            Envelope::new(
                EventContext { sender_id: sender, ..channel.event_context() },
                ActorEvent::CompleteMessageGenerated(message),
            )
        };
        let envelopes = vec![
            said(her, talker, "as alice's envelope"),
            said(talker, her, "as alice's message"),
            // On alice's behalf - she asked - but said by itself: as an agent answers.
            Envelope {
                context: EventContext {
                    sender_id: talker,
                    initiator_id: her,
                    ..channel.event_context()
                },
                ..said(talker, talker, "its own")
            },
        ];
        channel
            .adopt(
                Forger::sending(talker, envelopes),
                ResolvedParticipant::approver(channel.session_id, channel.id),
                in_thread_alone(ThreadId::line(channel.id)),
                None,
                None,
            )
            .await;
        let mut events = channel.observe();
        let running = tokio::spawn({
            let channel = Arc::clone(&channel);
            async move { channel.run().await }
        });

        let entered = time::timeout(Duration::from_secs(5), async {
            loop {
                if let ActorEvent::CompleteMessageGenerated(message) =
                    events.recv().await.expect("the channel runs").event
                {
                    break message.content.into_iter().collect::<Vec<_>>();
                }
            }
        })
        .await
        .expect("what it said in its own name enters");
        assert_eq!(entered, ["its own"], "nothing it said in another's name entered before it");
        let history: Vec<_> =
            channel.history().await.iter().filter_map(|m| m.content.clone()).collect();
        assert_eq!(history, ["its own"], "nor the history");
        running.abort();
    }

    #[tokio::test]
    async fn who_joined_or_left_is_the_channel_s_word_alone() {
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let her = ActorId::User(alice.id);
        let stranger = ActorId::User(UserId::new());
        let from_her = |event| {
            Envelope::new(EventContext { sender_id: her, ..channel.event_context() }, event)
        };
        let (to_her, mut hers) = opened();
        let actors = HashMap::from([(
            her,
            RunningActor {
                id: her,
                abort_handle: tokio::spawn(async {}).abort_handle(),
                task: TaskTracker::new(),
                inbox: Some(to_her),
                leaving: None,
                ended: Vec::new(),
            },
        )]);

        // In her own name, but no actor's to say (issue 408).
        channel
            .process_envelope(
                from_her(ActorEvent::ParticipantJoined {
                    participant: stranger,
                    name: "mallory".to_string(),
                    into_channel: false,
                }),
                &actors,
            )
            .await;
        channel
            .process_envelope(
                from_her(ActorEvent::ParticipantLeft {
                    participant: her,
                    name: "alice".to_string(),
                    until: Position::new(0),
                    out_of_channel: false,
                }),
                &actors,
            )
            .await;
        // Nor a thread opened or closed (issue 557).
        let line = ThreadId::line(channel.id);
        let origin = ThreadOrigin { thread: line, message: MessageId::new() };
        let thread = ThreadId::new();
        channel
            .process_envelope(from_her(ActorEvent::ThreadOpened { thread, origin }), &actors)
            .await;
        channel
            .process_envelope(from_her(ActorEvent::ThreadClosed { thread: line }), &actors)
            .await;

        let recorded = channel.history_entries().await;
        assert!(
            matches!(
                recorded.as_slice(),
                [
                    HistoryEntry::ParticipantJoined { .. },
                    HistoryEntry::EventRejected { .. },
                    HistoryEntry::EventRejected { .. },
                    HistoryEntry::EventRejected { .. },
                    HistoryEntry::EventRejected { .. }
                ]
            ),
            "after her own join, nothing of it is recorded, only that it was not let in (issue 417): \
             {recorded:?}"
        );
        let to_her_alone = channel
            .reading_history(|history| {
                history.entries().skip(1).all(|node| {
                    node.entry.sender == ActorId::System
                        && node.entry.recipients == Recipients::only(vec![her])
                })
            })
            .await;
        assert!(to_her_alone, "told to her alone, in the system's name");
        let mut rejected = Vec::new();
        while let Some(envelope) = hers.try_recv().await {
            if let ActorEvent::EventRejected { rejected: kind, reason, .. } = envelope.event {
                rejected.push((kind, reason));
            }
        }
        assert_eq!(
            rejected,
            [
                ("ParticipantJoined".to_string(), Rejection::Invalid),
                ("ParticipantLeft".to_string(), Rejection::Invalid),
                ("ThreadOpened".to_string(), Rejection::Invalid),
                ("ThreadClosed".to_string(), Rejection::Invalid)
            ],
            "and she is told so"
        );
    }

    /// Issues 419, 567: a person's answer to an approval says their own part alone - who decided is
    /// the approver's to say -, so one that says someone else's is not let in, before anything is
    /// asked of it; nor is a decision from anyone but an approver.
    #[tokio::test]
    async fn an_answer_to_an_approval_says_its_senders_part_alone() {
        use caelumora_domain::approval::{ApprovalDecision, ApprovalVerdict};
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let (her, bob) = (ActorId::User(alice.id), ActorId::User(UserId::new()));
        let from_her = |event| {
            Envelope::new(EventContext { sender_id: her, ..channel.event_context() }, event)
        };
        let (to_her, mut hers) = opened();
        let actors = HashMap::from([(
            her,
            RunningActor {
                id: her,
                abort_handle: tokio::spawn(async {}).abort_handle(),
                task: TaskTracker::new(),
                inbox: Some(to_her),
                leaving: None,
                ended: Vec::new(),
            },
        )]);
        let part = |by: ActorId| ApprovalDecision {
            by,
            name: String::new(),
            by_rule: None,
            at: Utc::now(),
        };
        let answered = |decision| ActorEvent::ApprovalAnswered {
            id: Default::default(),
            approved: true,
            decision,
        };
        // A decision is the approver's alone.
        let decided = ActorEvent::ApprovalGranted(ApprovalVerdict {
            decisions: vec![part(her)],
            ..ApprovalVerdict::new(Default::default(), AgentId::new())
        });
        for event in [answered(part(bob)), decided] {
            channel.process_envelope(from_her(event), &actors).await;
        }

        let mut rejected = Vec::new();
        while let Some(envelope) = hers.try_recv().await {
            if let ActorEvent::EventRejected { rejected: kind, reason, .. } = envelope.event {
                rejected.push((kind, reason));
            }
        }
        assert_eq!(
            rejected,
            [
                ("ApprovalAnswered".to_string(), Rejection::Invalid),
                ("ApprovalGranted".to_string(), Rejection::Invalid)
            ],
            "someone else's answer, or a decision of hers: not let in"
        );
    }

    /// Issue 567: an answer to an approval comes from a user; what an actor did not take goes to
    /// someone else taking part - not back to itself, nor to someone who takes no part.
    #[tokio::test]
    async fn an_answer_comes_from_a_user_and_a_refusal_goes_to_someone_else_taking_part() {
        use caelumora_domain::approval::ApprovalDecision;
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let her = ActorId::User(alice.id);
        let (agent, stranger) = (ActorId::Agent(AgentId::new()), ActorId::User(UserId::new()));
        let running = |id, inbox| RunningActor {
            id,
            abort_handle: tokio::spawn(async {}).abort_handle(),
            task: TaskTracker::new(),
            inbox: Some(inbox),
            leaving: None,
            ended: Vec::new(),
        };
        let ((to_her, mut hers), (to_agent, mut agents)) = (opened(), opened());
        let actors =
            HashMap::from([(her, running(her, to_her)), (agent, running(agent, to_agent))]);
        let sent = |sender: ActorId, event: ActorEvent| {
            Envelope::new(EventContext { sender_id: sender, ..channel.event_context() }, event)
        };
        let refused = |to| ActorEvent::EventRefused {
            to,
            refused: "MessageModified".to_string(),
            message: None,
            reason: Rejection::Invalid,
        };
        let decision =
            ApprovalDecision { by: agent, name: String::new(), by_rule: None, at: Utc::now() };
        let answered =
            ActorEvent::ApprovalAnswered { id: Default::default(), approved: true, decision };

        channel.process_envelope(sent(agent, answered), &actors).await;
        for to in [agent, stranger] {
            channel.process_envelope(sent(agent, refused(to)), &actors).await;
        }
        channel.process_envelope(sent(agent, refused(her)), &actors).await;

        let mut rejected = Vec::new();
        while let Some(envelope) = agents.try_recv().await {
            if let ActorEvent::EventRejected { rejected: kind, reason, .. } = envelope.event {
                rejected.push((kind, reason));
            }
        }
        assert_eq!(
            rejected,
            [
                ("ApprovalAnswered".to_string(), Rejection::Invalid),
                ("EventRefused".to_string(), Rejection::Invalid),
                ("EventRefused".to_string(), Rejection::Invalid)
            ],
            "an agent's answer, a refusal to itself or to a stranger: not let in"
        );
        let mut told = Vec::new();
        while let Some(envelope) = hers.try_recv().await {
            told.push(envelope.event.kind());
        }
        assert_eq!(told, ["EventRefused"], "a refusal to her, let in and delivered to her");
    }

    #[tokio::test]
    async fn what_is_recorded_of_a_turn_comes_from_an_agent_in_its_own_name() {
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let agent = ActorId::Agent(AgentId::new());
        let her = ActorId::User(alice.id);
        let sent = |sender: ActorId, event: ActorEvent| {
            Envelope::new(EventContext { sender_id: sender, ..channel.event_context() }, event)
        };
        let turn_id = TurnId::new();
        let activity = "compacting its context".to_string();
        let of_a_turn = [
            ActorEvent::PlanStepStarted {
                turn_id,
                step_index: 0,
                total_steps: 1,
                description: "first".to_string(),
            },
            ActorEvent::CommandExecuted {
                command: "compact".to_string(),
                args: String::new(),
                output: "done".to_string(),
                to: None,
                turn_id: Some(turn_id),
            },
            ActorEvent::ActivityStarted { turn_id, activity: activity.clone() },
            ActorEvent::ActivityFinished { turn_id, activity, output: "done".to_string() },
            ActorEvent::TurnFinished { turn_id, messages: Vec::new() },
            ActorEvent::TurnGivenUp { turn_id, reason: GivenUpReason::Left },
            // A prompt part its turn no longer gives (issue 537).
            ActorEvent::MessageWithdrawn { message: MessageId::new(), at: Utc::now() },
        ];
        for event in of_a_turn {
            let kind = event.kind();
            assert_eq!(
                channel.admit(&sent(agent, event.clone())).await,
                Ok(()),
                "{kind} of an agent's turn, in its own name"
            );
            assert_eq!(
                channel.admit(&sent(her, event)).await,
                Err(Rejection::Invalid),
                "{kind}: a user has no turns"
            );
        }
        let of_the_session = ActorEvent::CommandExecuted {
            command: "agent wakeup".to_string(),
            args: String::new(),
            output: "Woke.".to_string(),
            to: None,
            turn_id: None,
        };
        assert_eq!(
            channel.admit(&sent(her, of_the_session)).await,
            Ok(()),
            "a command of the session is run by a user, in no turn"
        );
    }

    #[tokio::test]
    async fn the_channel_wakes_only_whom_the_moderator_asks_to_respond() {
        let alice = make_user("alice");
        let context =
            SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let (asked, other) = (sleeper(), sleeper());
        let channel = channel_moderated_by(&[&alice], Arc::new(AskBack(asked.id()))).await;
        for actor in [&asked, &other] {
            // Who it is does not matter to routing: the channel only looks it up.
            channel
                .adopt(
                    Arc::<StandIn>::clone(actor),
                    ResolvedParticipant::approver(channel.session_id, channel.id),
                    in_thread_alone(ThreadId::line(channel.id)),
                    None,
                    None,
                )
                .await;
        }
        let said = || {
            Envelope::new(
                EventContext { sender_id: ActorId::User(alice.id), ..channel.event_context() },
                ActorEvent::TurnFinished {
                    turn_id: TurnId::new(),
                    messages: vec![Message::new(Classification::CHANNEL_DEFAULT)],
                },
            )
        };

        routed(&channel, &HashMap::new(), said()).await;
        assert!(asked.woken(), "asked to respond, it is woken to do so");
        assert!(!other.woken(), "not addressed, it sleeps on");

        // A quiet channel asks nobody, and wakes nobody.
        asked.sleep();
        channel.cancel(&context, None).await.expect("permitted");
        routed(&channel, &HashMap::new(), said()).await;
        assert!(!asked.woken(), "nobody is asked in a quiet channel");
    }

    #[tokio::test]
    async fn a_channel_admits_only_what_may_flow_into_it_in_its_senders_name() {
        use caelumora_security::OwnershipPolicy;
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(OwnershipPolicy::new())).await;
        let alice_id = ActorId::User(alice.id);
        let from = |sender: ActorId, classification: Classification| {
            Envelope::new(
                EventContext { sender_id: sender, ..channel.event_context() },
                ActorEvent::CompleteMessageGenerated(Message::new(classification)),
            )
        };
        let internal = Classification::general(Confidentiality::Internal);
        let actors = HashMap::new();
        channel.process_envelope(from(alice_id, internal.clone()), &actors).await;
        channel
            .process_envelope(
                from(alice_id, Classification::general(Confidentiality::Confidential)),
                &actors,
            )
            .await;
        assert_eq!(channel.history().await.len(), 1, "not above the channel");

        // A tool result is a part of its own.
        let call = ToolCall { id: None, arguments: serde_json::json!({}), ..make_tool_call("f") };
        let result = ToolCallResult::of(
            &call,
            Default::default(),
            "secret".to_string(),
            Classification::general(Confidentiality::Confidential),
        );
        let mut with_result = from(alice_id, internal);
        if let ActorEvent::CompleteMessageGenerated(message) = &mut with_result.event {
            message.tool_results.push(result);
        }
        channel.process_envelope(with_result, &actors).await;
        assert_eq!(channel.history().await.len(), 1, "nor with a result above the channel");
    }

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn a_change_to_a_message_enters_the_channel_only_from_its_author_as_it_stands() {
        use caelumora_security::OwnershipPolicy;
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(OwnershipPolicy::new())).await;
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(UserId::new()));
        let from = |sender: ActorId, event| {
            Envelope::new(EventContext { sender_id: sender, ..channel.event_context() }, event)
        };
        let said = Message {
            id: MessageId::new(),
            sender_id: alice_id,
            role: MessageRole::User,
            content: Some("helo".to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let modified = |sender: ActorId, content: &str| Message {
            sender_id: sender,
            content: Some(content.to_string()),
            timestamp: Utc::now(),
            ..said.clone()
        };
        let revoked = || ActorEvent::MessageRevoked { message: said.id, at: Utc::now() };
        let actors = HashMap::new();
        channel
            .process_envelope(
                from(alice_id, ActorEvent::CompleteMessageGenerated(said.clone())),
                &actors,
            )
            .await;
        let entries = || async { channel.reading_history(|history| history.entries().len()).await };

        channel
            .process_envelope(
                from(bob_id, ActorEvent::MessageModified(modified(bob_id, "bob's"))),
                &actors,
            )
            .await;
        channel.process_envelope(from(bob_id, revoked()), &actors).await;
        channel
            .process_envelope(
                from(
                    alice_id,
                    ActorEvent::MessageModified(Message {
                        id: MessageId::new(),
                        ..modified(alice_id, "nothing said")
                    }),
                ),
                &actors,
            )
            .await;
        assert_eq!(
            entries().await,
            2,
            "her join and what she said: neither a change by another nor of what was never said"
        );

        channel
            .process_envelope(
                from(alice_id, ActorEvent::MessageModified(modified(alice_id, "hello"))),
                &actors,
            )
            .await;
        let standing = channel.message(said.id).await.expect("said");
        assert_eq!(
            (standing.content.as_deref(), standing.modified.is_some()),
            (Some("hello"), true),
            "its author modifies it"
        );

        // Reclassified as the policy says (issue 239): here, with nobody
        // owning the channel, by its author alone.
        let reclassified =
            |classification| ActorEvent::MessageReclassified { message: said.id, classification };
        let above = Classification::general(Confidentiality::Confidential);
        channel.process_envelope(from(bob_id, reclassified(Classification::PUBLIC)), &actors).await;
        channel.process_envelope(from(alice_id, reclassified(above)), &actors).await;
        assert_eq!(entries().await, 3, "neither by another nor above the channel");
        channel
            .process_envelope(from(alice_id, reclassified(Classification::PUBLIC)), &actors)
            .await;
        assert_eq!(
            channel.message(said.id).await.map(|m| m.classification),
            Some(Classification::PUBLIC),
            "its author releases it"
        );

        channel.process_envelope(from(alice_id, revoked()), &actors).await;
        assert!(
            channel
                .message(said.id)
                .await
                .is_some_and(|m| m.revoked.is_some() && m.content.is_none()),
            "and takes it back"
        );
        channel.process_envelope(from(alice_id, revoked()), &actors).await;
        channel
            .process_envelope(
                from(alice_id, ActorEvent::MessageModified(modified(alice_id, "again"))),
                &actors,
            )
            .await;
        channel
            .process_envelope(
                from(alice_id, reclassified(Classification::CHANNEL_DEFAULT)),
                &actors,
            )
            .await;
        assert_eq!(entries().await, 5, "nothing changes it once taken back");
        assert_eq!(channel.history().await.len(), 1, "it stays in its place");

        // Only an agent is asked to answer again, and only for its own message.
        let mut routed = channel.observe();
        channel
            .process_envelope(
                from(
                    alice_id,
                    ActorEvent::RetryRequested {
                        target: RetryTarget::Message(said.id),
                        author: alice_id,
                    },
                ),
                &actors,
            )
            .await;
        channel
            .process_envelope(
                from(
                    alice_id,
                    ActorEvent::RetryRequested {
                        target: RetryTarget::Message(said.id),
                        author: ActorId::Agent(AgentId::new()),
                    },
                ),
                &actors,
            )
            .await;
        // An answer under way only of an agent of the channel.
        channel
            .process_envelope(
                from(
                    alice_id,
                    ActorEvent::RetryRequested {
                        target: RetryTarget::Turn(TurnId::new()),
                        author: alice_id,
                    },
                ),
                &actors,
            )
            .await;
        channel
            .process_envelope(
                from(
                    alice_id,
                    ActorEvent::RetryRequested {
                        target: RetryTarget::Turn(TurnId::new()),
                        author: ActorId::Agent(AgentId::new()),
                    },
                ),
                &actors,
            )
            .await;
        // A nudge only of someone else in the channel (issue 278).
        channel
            .process_envelope(
                from(alice_id, ActorEvent::NudgeRequested { participant: alice_id }),
                &actors,
            )
            .await;
        channel
            .process_envelope(
                from(
                    alice_id,
                    ActorEvent::NudgeRequested { participant: ActorId::Agent(AgentId::new()) },
                ),
                &actors,
            )
            .await;
        assert!(routed.try_recv().is_err(), "neither a user's message nor one another said, nor a turn of someone who is no agent here, nor a nudge of oneself or of a stranger");
    }

    #[tokio::test]
    async fn the_channel_keeps_the_end_of_its_history_and_reads_the_rest_from_the_store() {
        use caelumora_domain::history::WINDOW;
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let alice_id = ActorId::User(alice.id);
        let from = |event| {
            Envelope::new(EventContext { sender_id: alice_id, ..channel.event_context() }, event)
        };
        let said = |n: usize| Message {
            id: MessageId::new(),
            sender_id: alice_id,
            role: MessageRole::User,
            content: Some(format!("message {n}")),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let (to_alice, _alices) = opened();
        let actors = HashMap::from([(
            alice_id,
            RunningActor {
                id: alice_id,
                abort_handle: tokio::spawn(async {}).abort_handle(),
                task: TaskTracker::new(),
                inbox: Some(to_alice),
                leaving: None,
                ended: Vec::new(),
            },
        )]);
        let first = said(0);
        channel
            .process_envelope(from(ActorEvent::CompleteMessageGenerated(first.clone())), &actors)
            .await;
        for n in 1..250 {
            channel
                .process_envelope(from(ActorEvent::CompleteMessageGenerated(said(n))), &actors)
                .await;
        }
        let (base, end, digest) = channel
            .reading_history(|history| (history.base(), history.end(), history.digest()))
            .await;
        assert_eq!(
            (base, end),
            (end.before(WINDOW), Position::new(251)),
            "the channel holds the end of its history (issue 379), after her join"
        );
        assert_eq!(channel.visible_messages(alice_id).await.len(), 250, "the store all of it");

        // A message before the window is changed as any: the check reads the store.
        channel
            .process_envelope(
                from(ActorEvent::MessageModified(Message {
                    content: Some("changed".to_string()),
                    ..first.clone()
                })),
                &actors,
            )
            .await;
        assert_eq!(
            channel.message(first.id).await.and_then(|m| m.content).as_deref(),
            Some("changed")
        );

        let loaded = channel.state_store.get_channel_state(channel.id).await.unwrap();
        assert!(loaded.history.base() > Position::START, "loaded from where its window began");
        assert_ne!(loaded.history.digest(), digest, "with the change after it");
        assert_eq!(loaded.history.end(), Position::new(252));
    }

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn a_user_changes_only_their_own_message_and_learns_why_not() {
        use caelumora_security::OwnershipPolicy;
        let (alice, bob) = (make_user("alice"), make_user("bob"));
        let channel = channel_with(&[&alice, &bob], Arc::new(OwnershipPolicy::new())).await;
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(bob.id));
        let said = Message {
            id: MessageId::new(),
            sender_id: alice_id,
            role: MessageRole::User,
            content: Some("helo".to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let from = |sender: ActorId, event| {
            Envelope::new(EventContext { sender_id: sender, ..channel.event_context() }, event)
        };
        let (to_alice, mut alices) = opened();
        let (to_bob, mut bobs) = opened();
        let running = |id: ActorId, tx| {
            (
                id,
                RunningActor {
                    id,
                    abort_handle: tokio::spawn(async {}).abort_handle(),
                    task: TaskTracker::new(),
                    inbox: Some(tx),
                    leaving: None,
                    ended: Vec::new(),
                },
            )
        };
        let actors = HashMap::from([running(alice_id, to_alice), running(bob_id, to_bob)]);
        let rejected = async |rx: &mut InboxReceiver| {
            let mut rejections = Vec::new();
            while let Some(envelope) = rx.try_recv().await {
                if let ActorEvent::EventRejected { rejected, message, reason } = envelope.event {
                    assert_eq!(
                        envelope.context.sender_id,
                        ActorId::System,
                        "in the channel's name"
                    );
                    rejections.push((rejected, message, reason));
                }
            }
            rejections
        };
        channel
            .process_envelope(
                from(alice_id, ActorEvent::CompleteMessageGenerated(said.clone())),
                &actors,
            )
            .await;

        // What the channel does not let in, it tells its sender alone, and
        // why (issue 304).
        let above = Classification::general(Confidentiality::Confidential);
        let modified = |content: &str, classification: &Classification| {
            ActorEvent::MessageModified(Message {
                content: Some(content.to_string()),
                classification: classification.clone(),
                ..said.clone()
            })
        };
        let reclassified = |classification: &Classification| ActorEvent::MessageReclassified {
            message: said.id,
            classification: classification.clone(),
        };
        let revoked = |message| ActorEvent::MessageRevoked { message, at: Utc::now() };
        channel
            .process_envelope(
                from(bob_id, modified("bob's", &Classification::CHANNEL_DEFAULT)),
                &actors,
            )
            .await;
        channel.process_envelope(from(bob_id, revoked(said.id)), &actors).await;
        channel
            .process_envelope(from(bob_id, reclassified(&Classification::PUBLIC)), &actors)
            .await;
        let kind = |rejected: &str, reason| (rejected.to_string(), Some(said.id), reason);
        assert_eq!(
            rejected(&mut bobs).await,
            [
                kind("MessageModified", Rejection::Forbidden),
                kind("MessageRevoked", Rejection::Forbidden),
                kind("MessageReclassified", Rejection::Forbidden)
            ],
            "not his"
        );
        let never = MessageId::new();
        channel.process_envelope(from(alice_id, revoked(never)), &actors).await;
        channel.process_envelope(from(alice_id, modified("hello", &above)), &actors).await;
        channel.process_envelope(from(alice_id, reclassified(&above)), &actors).await;
        let secret = Message { id: MessageId::new(), ..Message::new(above.clone()) };
        channel
            .process_envelope(
                from(alice_id, ActorEvent::CompleteMessageGenerated(secret.clone())),
                &actors,
            )
            .await;
        assert_eq!(
            rejected(&mut alices).await,
            [
                ("MessageRevoked".to_string(), Some(never), Rejection::MessageNotFound),
                kind("MessageModified", Rejection::ClassificationRefused),
                kind("MessageReclassified", Rejection::ClassificationRefused),
                (
                    "CompleteMessageGenerated".to_string(),
                    Some(secret.id),
                    Rejection::ClassificationRefused
                ),
            ],
            "never said, and never above the channel - the flow, not her permission"
        );
        assert!(rejected(&mut bobs).await.is_empty(), "nor is anyone else told");
        channel.process_envelope(from(alice_id, revoked(said.id)), &actors).await;
        channel
            .process_envelope(
                from(alice_id, modified("hello", &Classification::CHANNEL_DEFAULT)),
                &actors,
            )
            .await;
        for event in [
            revoked(said.id),
            reclassified(&Classification::PUBLIC),
            ActorEvent::RetryRequested { target: RetryTarget::Message(said.id), author: alice_id },
        ] {
            channel.process_envelope(from(alice_id, event), &actors).await;
        }
        assert_eq!(
            rejected(&mut alices).await,
            [
                kind("MessageModified", Rejection::MessageUnchangeable),
                kind("MessageRevoked", Rejection::MessageUnchangeable),
                kind("MessageReclassified", Rejection::MessageUnchangeable),
                kind("RetryRequested", Rejection::MessageUnchangeable),
            ],
            "taken back, it stays so - the door's to tell, not her actor's (issue 567)"
        );

        // A request to answer again names an agent taking part, of a
        // message as of a turn (issue 362): of one that left, its sender is
        // told, rather than it being dropped unseen.
        let gone = ActorId::Agent(AgentId::new());
        let its = Message {
            id: MessageId::new(),
            sender_id: gone,
            role: MessageRole::Assistant,
            content: Some("said before it left".to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        channel
            .process_envelope(
                from(gone, ActorEvent::CompleteMessageGenerated(its.clone())),
                &actors,
            )
            .await;
        channel
            .process_envelope(
                from(
                    alice_id,
                    ActorEvent::RetryRequested {
                        target: RetryTarget::Message(its.id),
                        author: gone,
                    },
                ),
                &actors,
            )
            .await;
        channel
            .process_envelope(
                from(
                    alice_id,
                    ActorEvent::RetryRequested {
                        target: RetryTarget::Turn(TurnId::new()),
                        author: gone,
                    },
                ),
                &actors,
            )
            .await;
        assert_eq!(
            rejected(&mut alices).await,
            [
                ("RetryRequested".to_string(), Some(its.id), Rejection::Invalid),
                ("RetryRequested".to_string(), None, Rejection::Invalid),
            ],
            "an agent that left answers nothing again"
        );

        // Her actor reads only what it needs to make a modification or to address a request to
        // answer again - a message not there it cannot -, and leaves whether it may or can be done
        // to the channel, as above (issue 567): a message taken back is sent all the same.
        let hers = channel.user_actor(alice.id).unwrap();
        let his = channel.user_actor(bob.id).unwrap();
        let error = |result: Result<SpanId>| {
            result.expect_err("refused").downcast::<ChannelError>().expect("a channel's error")
        };
        assert!(
            matches!(
                error(hers.modify_message(MessageId::new(), None, None, Vec::new()).await),
                ChannelError::MessageNotFound { .. }
            ),
            "nothing to modify"
        );
        assert!(
            matches!(
                error(his.request_retry(MessageId::new()).await),
                ChannelError::MessageNotFound { .. }
            ),
            "nobody to ask"
        );
        hers.modify_message(said.id, Some("hello".to_string()), None, Vec::new())
            .await
            .expect("taken back, for the channel to reject");
        hers.revoke_message(MessageId::new()).await.expect("never said, for the channel to reject");
        hers.reclassify_message(said.id, Classification::PUBLIC)
            .await
            .expect("taken back, for the channel to reject");
        his.request_retry(said.id).await.expect("taken back, for the channel to reject");
    }

    #[tokio::test]
    async fn a_removal_is_announced_in_the_channel_in_the_name_of_who_removed_after_the_farewell() {
        let Stuck { channel, context, agent, helper, mut events, running } =
            stuck_in_a_turn().await;

        channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");

        // What a frontend needs to end the turn it shows: who is gone -
        // announced once the actor's task has ended, after the end of its
        // turn, which it tells itself (issue 262); nothing of it follows.
        let (removed, farewell_first) = time::timeout(Duration::from_secs(5), async {
            let mut farewell = false;
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == ActorId::Agent(agent)
                    && matches!(envelope.event, ActorEvent::TurnGivenUp { .. })
                {
                    farewell = true;
                }
                if let ActorEvent::ParticipantLeft { participant, .. } = envelope.event {
                    break (
                        (envelope.context.sender_id, envelope.context.sender_name, participant),
                        farewell,
                    );
                }
            }
        })
        .await
        .expect("the removal is announced");
        assert_eq!(removed, (context.caller_id, "alice".to_string(), ActorId::Agent(agent)));
        assert!(farewell_first, "the agent's farewell precedes the announcement");
        settled(&helper, ActorState::Idle).await;
        time::sleep(Duration::from_millis(50)).await;
        while let Ok(envelope) = events.try_recv() {
            assert_ne!(
                envelope.context.sender_id,
                ActorId::Agent(agent),
                "nothing of it follows the announcement: {}",
                envelope.describe()
            );
        }
        running.abort();
    }

    #[tokio::test]
    async fn nothing_of_a_removed_participant_enters_the_channel_once_its_task_has_ended() {
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let mut events = channel.observe();
        let gone = ActorId::Agent(AgentId::new());
        let late = Envelope::new(
            EventContext::new(channel.session_id, channel.id, gone, "helper".to_string()),
            ActorEvent::TurnGivenUp {
                turn_id: TurnId::new(),
                reason: GivenUpReason::Failed { message: "too late".to_string() },
            },
        );

        // Its actor no longer runs, nor is it leaving: it was aborted.
        channel.handle_actor_event(late, &HashMap::new()).await;

        assert!(events.try_recv().is_err(), "what a removed participant sends is dropped");
    }

    /// Issue 593: waiting for the actors of a channel to end is answered once every one of them
    /// that runs has ended - as `run` sees its task end -, and at once where none runs.
    #[tokio::test]
    async fn ending_the_actors_is_answered_once_the_last_has_ended() {
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let (event_tx, _event_rx) = mpsc::channel(10);
        let mut join_set = JoinSet::new();
        let mut running = HashMap::new();
        carry_out_requests(&channel, &mut running, &event_tx, &mut join_set).await;
        let hers = ActorId::User(alice.id);
        assert!(running.contains_key(&hers), "her actor runs");

        let ended = tokio::spawn(channel.request(ActorChange::End).carried_out());
        carry_out_requests(&channel, &mut running, &event_tx, &mut join_set).await;
        time::sleep(Duration::from_millis(20)).await;
        assert!(!ended.is_finished(), "not while it runs");
        channel.actor(hers).expect("hers").stop();
        let task = time::timeout(Duration::from_secs(1), join_set.join_next_with_id()).await;
        let (task, _) = task.expect("told to stop, her task ends").unwrap().unwrap();
        assert_eq!(running[&hers].abort_handle.id(), task, "hers");
        assert!(!ended.is_finished(), "not before `run` has seen it end");
        let actor = running.remove(&hers).expect("hers");
        channel.task_ended(actor, &running).await;
        assert!(ended.await.unwrap(), "once it has");

        let ended = tokio::spawn(channel.request(ActorChange::End).carried_out());
        carry_out_requests(&channel, &mut running, &event_tx, &mut join_set).await;
        assert!(ended.await.unwrap(), "at once where none runs");
    }

    /// Issue 104: the actors of a channel that never ran - taken out of its session before it
    /// started - are told to leave all the same, so that whoever follows one learns it ended.
    #[tokio::test]
    async fn the_actors_of_a_channel_that_never_ran_are_told_to_leave() {
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let actors = channel.runtime.kernel().actor_manager();
        let mut following = ChannelWatch::of(&actors.outbox(channel.id, ActorId::User(alice.id)));
        channel.end_actors(Ending::Leave).await;
        channel.never_to_run();
        let shown = time::timeout(Duration::from_secs(1), following.next()).await;
        assert!(matches!(shown, Ok(Ok(Shown::Ended))), "her actor ended");
    }

    /// Issue 593: what an actor sends as its channel is deleted - the turn it gives up - is in the
    /// channel by the time ending its actors returns, before the channel is stopped.
    #[tokio::test]
    async fn what_actors_send_as_they_end_is_in_the_channel_once_they_have_ended() {
        let Stuck { channel, agent, mut events, running, .. } = stuck_in_a_turn().await;
        channel.end_actors(Ending::Leave).await;
        running.abort();
        let given_up = iter::from_fn(|| events.try_recv().ok()).any(|envelope| {
            envelope.context.sender_id == ActorId::Agent(agent)
                && matches!(envelope.event, ActorEvent::TurnGivenUp { .. })
        });
        assert!(given_up, "the turn it gave up entered the channel");
    }

    /// An actor told to stop whose task has not ended: no inbox, who removed it, and the threads
    /// it took part in.
    fn leaving_actor(
        id: ActorId,
        remover: SecurityContext,
        threads: BTreeSet<ThreadId>,
    ) -> RunningActor {
        let name = "helper".to_string();
        RunningActor {
            id,
            abort_handle: tokio::spawn(async {}).abort_handle(),
            task: TaskTracker::new(),
            inbox: None,
            leaving: Some(Leaving {
                remover,
                name,
                until: Position::START,
                threads,
                carried_out: Vec::new(),
            }),
            ended: Vec::new(),
        }
    }

    /// Carries out what `channel` was handed so far, as `run` does with what it receives, and
    /// gives the receiver back.
    async fn carry_out_requests(
        channel: &ChannelLoop,
        running: &mut HashMap<ActorId, RunningActor>,
        event_tx: &Sender<Envelope>,
        join_set: &mut JoinSet<ActorId>,
    ) {
        let taken = channel.requests_rx.lock().unwrap().take();
        let mut requests = taken.expect("not taken by `run`");
        while let Ok(request) = requests.try_recv() {
            channel.apply_change(request, running, event_tx, join_set).await;
        }
        *channel.requests_rx.lock().unwrap() = Some(requests);
    }

    /// Issue 593: `run` answers a leave of one without an actor, an opening, a brief - one it drops
    /// as not taken (issue 598) - and a closing at once.
    #[tokio::test]
    async fn run_answers_at_once_what_it_carries_out_at_once() {
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let system = SecurityContext::system();
        let (event_tx, _event_rx) = mpsc::channel(10);
        let mut join_set = JoinSet::new();
        let line = ThreadId::line(channel.id);
        let gone = ActorId::Agent(AgentId::new());
        let in_line = Arc::new(ArcSwap::from_pointee(BTreeSet::from([line])));
        let leave = ActorChange::Leave(gone, "gone".to_string(), system.clone(), in_line);
        let left = channel.request(leave);
        let origin = ThreadOrigin { thread: line, message: MessageId::new() };
        let thread = ThreadId::new();
        let opening =
            Opening { origin, by: system.caller_id, circle: Circle::Closed, owned: Vec::new() };
        let opened = channel.request(ActorChange::Open(thread, opening, system.clone()));
        let briefs = VecDeque::new();
        let delegation = Delegation {
            giver: gone,
            recipient: gone,
            initiator: gone,
            briefs,
            word: Word::NotDue,
        };
        let delegated = channel.request(ActorChange::Delegate(thread, delegation));
        let mut running = HashMap::new();
        carry_out_requests(&channel, &mut running, &event_tx, &mut join_set).await;
        assert!(left.carried_out().await, "a leave without an actor, announced at once");
        assert!(opened.carried_out().await, "an opening, announced");
        assert!(
            !delegated.carried_out().await,
            "a brief dropped for a thread the channel holds not: answered at once as not taken"
        );

        let close = channel.request(ActorChange::Close(thread, system.clone()));
        carry_out_requests(&channel, &mut running, &event_tx, &mut join_set).await;
        assert!(close.carried_out().await, "a closing, announced");
    }

    /// Issue 602: a leave is announced in every thread the participant took part in - the first
    /// of them as the leave of the channel (issue 603).
    #[tokio::test]
    async fn a_leave_is_announced_in_every_thread_taken_part_in() {
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let mut events = channel.observe();
        let (event_tx, _event_rx) = mpsc::channel(10);
        let mut join_set = JoinSet::new();
        let (line, thread) = (ThreadId::line(channel.id), ThreadId::new());
        let system = SecurityContext::system();
        let origin = ThreadOrigin { thread: line, message: MessageId::new() };
        channel.open_thread(&system, thread, &ThreadSpec::new(origin)).await.expect("opened");
        let gone = ActorId::Agent(AgentId::new());
        let both = Arc::new(ArcSwap::from_pointee(BTreeSet::from([line, thread])));
        let left = channel.request(ActorChange::Leave(gone, "gone".to_string(), system, both));
        carry_out_requests(&channel, &mut HashMap::new(), &event_tx, &mut join_set).await;
        assert!(left.carried_out().await, "announced");

        let mut announced = BTreeSet::new();
        let mut of_channel = 0;
        while let Ok(event) = events.try_recv() {
            if let ActorEvent::ParticipantLeft { participant, out_of_channel, .. } = event.event {
                assert_eq!(participant, gone, "who left");
                announced.insert(event.context.thread_id);
                of_channel += usize::from(out_of_channel);
            }
        }
        assert_eq!(announced, BTreeSet::from([line, thread]), "in each");
        assert_eq!(of_channel, 1, "one of them the leave of the channel");
    }

    /// Issue 602: an actor drawn into a thread while it was let go - after the channel took it out
    /// of its actors, before `run` took the leave in - leaves that thread as well: `run` announces
    /// the leave in every thread the actor takes part in as it takes the leave in.
    #[tokio::test]
    async fn a_leave_is_announced_in_a_thread_drawn_into_while_letting_go() {
        let (channel, context, agent, _) = scripted_channel(Script::Answers("ok".into())).await;
        let helper = ActorId::Agent(agent);
        let origin = ThreadOrigin { thread: ThreadId::line(channel.id), message: MessageId::new() };
        let spec = ThreadSpec::new(origin);
        let thread = channel.open_thread_by_procedure(&context, spec).await.expect("opened");
        let threads = Arc::clone(&channel.actors.load().get(&helper).expect("it runs").threads);
        let mut events = channel.observe();
        channel.remove_actor(helper, &context).await;
        // As `take_part` would, had a thread drawn it in right then.
        threads.rcu(|taking_part| {
            let mut taking_part = BTreeSet::clone(taking_part);
            taking_part.insert(thread);
            taking_part
        });
        let running = tokio::spawn({
            let channel = Arc::clone(&channel);
            async move { channel.run().await }
        });

        time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("open");
                let left = matches!(
                    envelope.event,
                    ActorEvent::ParticipantLeft { participant, .. } if participant == helper
                );
                if left && envelope.context.thread_id == thread {
                    break;
                }
            }
        })
        .await
        .expect("its leave announced in the thread it was drawn into");
        running.abort();
    }

    /// Issue 593: `run` runs on, empty, until it is aborted; then it has run once, and a request
    /// handed in after is answered with nothing, at once.
    #[tokio::test]
    async fn run_runs_once_until_aborted_and_what_is_handed_in_after_is_answered_with_nothing() {
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let running = tokio::spawn({
            let channel = Arc::clone(&channel);
            async move { channel.run().await }
        });
        channel.end_actors(Ending::Stop).await;
        time::sleep(Duration::from_millis(50)).await;
        assert!(!running.is_finished(), "empty, it runs on");
        running.abort();
        assert!(running.await.is_err_and(|e| e.is_cancelled()), "until it is aborted");

        let line = ThreadId::line(channel.id);
        let gone = ActorId::Agent(AgentId::new());
        let late = channel.request(ActorChange::Leave(
            gone,
            "gone".to_string(),
            SecurityContext::system(),
            Arc::new(ArcSwap::from_pointee(BTreeSet::from([line]))),
        ));
        let answered = time::timeout(Duration::from_secs(1), late.carried_out()).await;
        assert!(!answered.expect("answered at once"), "with nothing");
        assert!(channel.run().await.is_err(), "it runs once");
    }

    /// Issue 593: a leave asked for again while the actor is leaving - one let go while it rests -
    /// is announced once and answered to both, once its task has ended.
    #[tokio::test]
    async fn a_leave_asked_again_while_leaving_is_answered_to_both_once_announced() {
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let system = SecurityContext::system();
        let line = ThreadId::line(channel.id);
        let (event_tx, _event_rx) = mpsc::channel(10);
        let mut join_set = JoinSet::new();
        let helper = ActorId::Agent(AgentId::new());
        let in_line = BTreeSet::from([line]);
        let leaving = leaving_actor(helper, system.clone(), in_line.clone());
        let mut running = HashMap::from([(helper, leaving)]);

        let threads = Arc::new(ArcSwap::from_pointee(in_line.clone()));
        let leave = || {
            ActorChange::Leave(helper, "helper".to_string(), system.clone(), Arc::clone(&threads))
        };
        let left = tokio::spawn(channel.request(leave()).carried_out());
        let again = tokio::spawn(channel.request(leave()).carried_out());
        carry_out_requests(&channel, &mut running, &event_tx, &mut join_set).await;
        time::sleep(Duration::from_millis(20)).await;
        assert!(!left.is_finished() && !again.is_finished(), "not while it is still leaving");

        let Some(RunningActor { leaving: Some(leaving), .. }) = running.remove(&helper) else {
            panic!("leaving");
        };
        assert_eq!(leaving.carried_out.len(), 2, "both asked");
        channel.announce_removal(helper, leaving, &running).await;
        assert!(left.await.unwrap() && again.await.unwrap(), "both answered once announced");
        let entries = channel.history_entries().await;
        let left = entries.iter().filter(|e| matches!(e, HistoryEntry::ParticipantLeft { .. }));
        assert_eq!(left.count(), 1, "announced once: {entries:?}");
    }

    #[tokio::test]
    async fn what_a_leaving_actor_sends_enters_the_channel_until_its_task_has_ended() {
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let mut events = channel.observe();
        let going = ActorId::Agent(AgentId::new());
        let turn_id = TurnId::new();
        let farewell = Envelope::new(
            EventContext::new(channel.session_id, channel.id, going, "helper".to_string()),
            ActorEvent::TurnGivenUp { turn_id, reason: GivenUpReason::Left },
        );

        // Told to stop, its task still runs: what it says is its own (issue 262).
        let in_line = BTreeSet::from([ThreadId::line(channel.id)]);
        let leaving = leaving_actor(going, SecurityContext::default(), in_line);
        let running = HashMap::from([(going, leaving)]);
        channel.handle_actor_event(farewell, &running).await;

        let heard = events.try_recv().expect("the farewell enters the channel");
        assert!(
            matches!(heard.event, ActorEvent::TurnGivenUp { turn_id: id, .. } if id == turn_id)
        );
        assert_eq!(heard.context.sender_id, going);
    }

    #[tokio::test]
    async fn a_removal_still_pending_is_announced_before_an_actor_starts_under_the_same_id() {
        let (channel, context, agent, _) = scripted_channel(Script::Hangs).await;
        let mut events = channel.observe();
        let participant = ActorId::Agent(agent);
        let joined =
            channel.actors.load().get(&participant).cloned().expect("the agent takes part");

        // Its old actor's task has ended - `add_participant` waited for it -
        // but `run` has not seen that yet when the new actor's start comes.
        let (event_tx, _event_rx) = mpsc::channel(10);
        let mut join_set = JoinSet::new();
        let (left, old_leave) = oneshot::channel();
        let threads = BTreeSet::clone(&joined.threads.load());
        let mut old = leaving_actor(participant, context.clone(), threads);
        if let Some(leaving) = &mut old.leaving {
            leaving.carried_out.push(left);
        }
        let mut running = HashMap::from([(participant, old)]);
        let change = ActorChange::Start(joined, Some(Position::START), Some(context.clone()));
        let (carried_out, started) = oneshot::channel();
        channel
            .apply_change(Request { change, carried_out }, &mut running, &event_tx, &mut join_set)
            .await;
        assert!(old_leave.await.is_ok(), "the old actor's leave answered (issue 593)");
        assert!(started.await.is_ok(), "and the start");

        let announced = events.try_recv().expect("the removal is announced");
        assert!(
            matches!(announced.event, ActorEvent::ParticipantLeft { participant: p, .. } if p == participant),
            "{}",
            announced.describe()
        );
        assert_eq!(announced.context.sender_id, context.caller_id, "in the remover's name");
        let shared = events.try_recv().expect("what it is given of the history is told after it");
        assert!(
            matches!(shared.event, ActorEvent::HistoryShared { participant: p, from: Position::START, by, .. } if p == participant && by == context.caller_id),
            "{}",
            shared.describe()
        );
        assert_eq!(
            shared.context.sender_id,
            ActorId::System,
            "in the system's name, for whom it names (issue 417)"
        );
        let added = events.try_recv().expect("the join is announced after it");
        assert!(
            matches!(added.event, ActorEvent::ParticipantJoined { participant: p, .. } if p == participant),
            "{}",
            added.describe()
        );
        assert!(
            running.get(&participant).is_some_and(|actor| actor.leaving.is_none()),
            "and then the new actor runs"
        );
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
        fn id(&self) -> ActorId {
            self.id
        }
        fn name(&self) -> Name {
            Name::new("listener").expect("a name")
        }
        fn event_filter(&self) -> fn(&Envelope) -> bool {
            |_| true
        }
        async fn heard(&self) -> Option<Heard> {
            Some(Heard::default())
        }
        async fn run(
            &self,
            mut rx: Box<dyn EventReceiver>,
            _tx: Box<dyn EventSender>,
        ) -> Result<()> {
            while let Some(envelope) = rx.recv().await {
                let _ = self
                    .taken_in
                    .send(TakenIn::Heard(envelope.event.kind(), envelope.context.position));
            }
            Ok(())
        }
        fn state(&self) -> ActorState {
            ActorState::Idle
        }
        fn stop(&self) {}
        fn leave(&self) {}
    }

    #[tokio::test]
    async fn a_newcomer_reads_what_it_was_given_then_its_join_then_what_is_said_from_there_on() {
        let alice = make_user("alice");
        let context =
            SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let said = || {
            Envelope::new(
                EventContext { sender_id: ActorId::User(alice.id), ..channel.event_context() },
                ActorEvent::CompleteMessageGenerated(Message {
                    id: MessageId::new(),
                    sender_id: ActorId::User(alice.id),
                    role: MessageRole::User,
                    content: Some("hello".to_string()),
                    ..Message::new(Classification::CHANNEL_DEFAULT)
                }),
            )
        };
        let (event_tx, _event_rx) = mpsc::channel(10);
        let mut join_set = JoinSet::new();
        let mut running = HashMap::new();
        let start_all = async |running: &mut HashMap<ActorId, RunningActor>,
                               join_set: &mut JoinSet<ActorId>| {
            carry_out_requests(&channel, running, &event_tx, join_set).await;
        };
        start_all(&mut running, &mut join_set).await;
        let (newcomer, mut taken_in) = Listener::new(ActorId::User(UserId::new()));
        let from = channel.reading_history(|history| history.end()).await;

        // Taken in, but `run` has not started it yet: what is said meanwhile
        // has no inbox to go to.
        channel
            .adopt(
                Arc::<Listener>::clone(&newcomer),
                ResolvedParticipant::approver(channel.session_id, channel.id),
                in_thread_alone(ThreadId::line(channel.id)),
                Some(from),
                Some(context.clone()),
            )
            .await;
        channel.let_in(said(), &running).await;
        start_all(&mut running, &mut join_set).await;
        channel.let_in(said(), &running).await;

        let mut told = Vec::new();
        while told.len() < 4 {
            told.push(
                time::timeout(Duration::from_secs(5), taken_in.recv())
                    .await
                    .expect("taken in")
                    .expect("the listener runs"),
            );
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
        let alice = make_user("alice");
        let context =
            SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let bob = ActorId::Agent(AgentId::new());
        let channel = channel_moderated_by(&[&alice], Arc::new(AskBack(bob))).await;
        let (tx, mut rx) = opened();
        let abort_handle = tokio::spawn(async {}).abort_handle();
        let actors = HashMap::from([(
            bob,
            RunningActor {
                id: bob,
                abort_handle,
                task: TaskTracker::new(),
                inbox: Some(tx),
                leaving: None,
                ended: Vec::new(),
            },
        )]);
        let from_alice = |event| {
            Envelope::new(
                EventContext { sender_id: ActorId::User(alice.id), ..channel.event_context() },
                event,
            )
        };
        let said = || {
            from_alice(ActorEvent::TurnFinished {
                turn_id: TurnId::new(),
                messages: vec![Message::new(Classification::CHANNEL_DEFAULT)],
            })
        };
        let delivered = async |rx: &mut InboxReceiver| {
            let mut kinds = Vec::new();
            while let Some(envelope) = rx.try_recv().await {
                kinds.push(envelope.event.kind());
            }
            kinds
        };

        channel.cancel(&context, None).await.expect("permitted");
        routed(&channel, &actors, said()).await;
        assert_eq!(
            delivered(&mut rx).await,
            ["TurnFinished"],
            "what was said still arrives, but nobody is asked"
        );

        // A message alone does not wake it: its sender asks for that, as a
        // user's actor does with every message (issue 90).
        routed(&channel, &actors, from_alice(ActorEvent::WakeupRequested)).await;
        delivered(&mut rx).await; // this test's moderator forwards the request, too
        routed(&channel, &actors, said()).await;
        assert_eq!(
            delivered(&mut rx).await,
            ["TurnFinished", "ResponseRequested"],
            "woken, the channel asks again"
        );
    }

    #[tokio::test]
    async fn cancelling_one_actor_ends_its_turn_alone_and_keeps_the_channel_answering() {
        let alice = make_user("alice");
        let context =
            SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let (cancelled, other) = (sleeper(), sleeper());
        let channel = channel_moderated_by(&[&alice], Arc::new(AskBack(other.id()))).await;
        for actor in [&cancelled, &other] {
            channel
                .adopt(
                    Arc::<StandIn>::clone(actor),
                    ResolvedParticipant::approver(channel.session_id, channel.id),
                    in_thread_alone(ThreadId::line(channel.id)),
                    None,
                    None,
                )
                .await;
        }

        channel.cancel_actor(&context, cancelled.id()).await.expect("permitted");
        assert!(cancelled.cancelled(), "its turn is ended");
        assert!(!other.cancelled(), "the other's goes on");

        let said = Envelope::new(
            EventContext { sender_id: ActorId::User(alice.id), ..channel.event_context() },
            ActorEvent::TurnFinished {
                turn_id: TurnId::new(),
                messages: vec![Message::new(Classification::CHANNEL_DEFAULT)],
            },
        );
        routed(&channel, &HashMap::new(), said).await;
        assert!(other.woken(), "the channel is not quiet: the next message is answered");

        let stranger = ActorId::Agent(AgentId::new());
        let error = channel.cancel_actor(&context, stranger).await.expect_err("not in the channel");
        assert!(
            matches!(
                error.downcast_ref::<ChannelError>(),
                Some(ChannelError::NotAParticipant { .. })
            ),
            "{error}"
        );
        let error = channel.wakeup_actor(&context, stranger).await.expect_err("nor to be woken");
        assert!(
            matches!(
                error.downcast_ref::<ChannelError>(),
                Some(ChannelError::NotAParticipant { .. })
            ),
            "the same error: {error}"
        );
    }

    /// Issue 557: of what happens in a thread, an agent is delivered what happens in its own, a
    /// user what happens in any.
    #[tokio::test]
    async fn an_agent_is_delivered_what_happens_in_its_thread_and_a_user_what_happens_in_any() {
        let (channel, _, agent, alice) = scripted_channel(Script::Hangs).await;
        let (agent, alice) = (ActorId::Agent(agent), ActorId::User(alice));
        let said_in = |thread_id| {
            Envelope::new(
                EventContext { sender_id: alice, thread_id, ..channel.event_context() },
                ActorEvent::CompleteMessageGenerated(Message {
                    sender_id: alice,
                    role: MessageRole::User,
                    content: Some("hello".to_string()),
                    ..Message::new(Classification::CHANNEL_DEFAULT)
                }),
            )
        };
        let in_line = said_in(ThreadId::line(channel.id));
        let in_line: Vec<ActorId> = channel
            .deliveries(&in_line, Routing::default(), None, false)
            .await
            .into_iter()
            .map(|(to, _)| to)
            .collect();
        assert!(in_line.contains(&agent), "an agent, what is said in its thread");
        let elsewhere = said_in(ThreadId::new());
        let elsewhere: Vec<ActorId> = channel
            .deliveries(&elsewhere, Routing::default(), None, false)
            .await
            .into_iter()
            .map(|(to, _)| to)
            .collect();
        assert!(!elsewhere.contains(&agent), "an agent, nothing of another thread");
        assert!(elsewhere.contains(&alice), "a user, what is said in any thread");
    }

    /// Issue 557: what enters the channel is routed in the thread it happens in - its line - and
    /// recorded there; what names a thread the channel does not have is not let in, and its
    /// sender is told why, in the thread it named.
    #[tokio::test]
    async fn an_event_enters_in_its_thread_and_one_of_no_thread_of_the_channel_is_not_let_in() {
        let alice = make_user("alice");
        let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
        let alice_id = ActorId::User(alice.id);
        let said = |thread_id: ThreadId, text: &str| {
            Envelope::new(
                EventContext { sender_id: alice_id, thread_id, ..channel.event_context() },
                ActorEvent::CompleteMessageGenerated(Message {
                    sender_id: alice_id,
                    role: MessageRole::User,
                    content: Some(text.to_string()),
                    ..Message::new(Classification::CHANNEL_DEFAULT)
                }),
            )
        };
        let (to_alice, mut alices) = opened();
        let running = RunningActor {
            id: alice_id,
            abort_handle: tokio::spawn(async {}).abort_handle(),
            task: TaskTracker::new(),
            inbox: Some(to_alice),
            leaving: None,
            ended: Vec::new(),
        };
        let actors = HashMap::from([(alice_id, running)]);
        let (line, elsewhere) = (ThreadId::line(channel.id), ThreadId::new());
        channel.process_envelope(said(line, "in the line"), &actors).await;
        channel.process_envelope(said(elsewhere, "elsewhere"), &actors).await;

        let mut rejections = Vec::new();
        while let Some(envelope) = alices.try_recv().await {
            if let ActorEvent::EventRejected { reason, .. } = envelope.event {
                rejections.push((reason, envelope.context.thread_id));
            }
        }
        assert_eq!(rejections, [(Rejection::Invalid, elsewhere)], "told why, in the thread named");
        let recorded = channel
            .reading_history(|history| {
                history
                    .entries()
                    .filter_map(|node| match &node.entry.entry {
                        HistoryEntry::Said { message } => {
                            Some((message.content.clone(), node.entry.context.thread_id))
                        }
                        HistoryEntry::EventRejected { .. } => {
                            Some((None, node.entry.context.thread_id))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
            .await;
        assert_eq!(
            recorded,
            [(Some("in the line".to_string()), line), (None, elsewhere)],
            "what is said in the line, recorded there, then the rejection of the other, in its thread"
        );
    }

    /// Issue 557: what is said in a thread opened enters there; once it is closed - by `run`, as it
    /// records it - nothing more is let in, its sender told why. An agent opens a thread at
    /// another thread, a person at the line alone.
    #[tokio::test]
    async fn a_thread_takes_in_what_is_said_until_it_is_closed() {
        let alice = make_user("alice");
        let access: Arc<dyn AccessPolicy> = Arc::new(AllowAllPolicy::new());
        let channel = channel_with(&[&alice], Arc::clone(&access)).await;
        let alice_id = ActorId::User(alice.id);
        let system = SecurityContext::system();
        let line = ThreadId::line(channel.id);
        let origin = ThreadOrigin { thread: line, message: MessageId::new() };
        let thread = channel
            .open_thread_by_procedure(&system, ThreadSpec::new(origin))
            .await
            .expect("opened");
        // What `run` does with the changes the channel hands it.
        let (event_tx, _event_rx) = mpsc::channel(10);
        let applied = async || {
            let mut running = HashMap::new();
            let mut join_set = JoinSet::new();
            carry_out_requests(&channel, &mut running, &event_tx, &mut join_set).await;
        };
        applied().await;
        let said = |text: &str| {
            Envelope::new(
                EventContext { sender_id: alice_id, thread_id: thread, ..channel.event_context() },
                ActorEvent::CompleteMessageGenerated(Message {
                    sender_id: alice_id,
                    role: MessageRole::User,
                    content: Some(text.to_string()),
                    ..Message::new(Classification::CHANNEL_DEFAULT)
                }),
            )
        };
        assert_eq!(channel.admit(&said("while open")).await, Ok(()), "let in while it is open");
        let agent =
            SecurityContext::new(ActorId::Agent(AgentId::new()), Default::default(), channel.id);
        let deeper = ThreadOrigin { thread, message: MessageId::new() };
        let opening = async |context: &SecurityContext| {
            let open = channel.is_open(deeper.thread);
            require_opening(access.as_ref(), context, &channel.channel(), deeper, open).await
        };
        opening(&agent).await.expect("an agent opens one at a thread");
        let person = SecurityContext::new(alice_id, Default::default(), channel.id);
        let refused = opening(&person).await.expect_err("not at the line");
        assert!(
            matches!(
                refused.downcast_ref::<ChannelError>(),
                Some(ChannelError::NotAtTheLine { .. })
            ),
            "{refused}"
        );

        // Closing waits until `run` closed it, which the test carries out by hand.
        let closing = tokio::spawn({
            let (channel, system) = (Arc::clone(&channel), system.clone());
            async move { channel.close_thread_by_procedure(&system, thread).await }
        });
        time::sleep(Duration::from_millis(20)).await;
        assert!(!closing.is_finished(), "it waits until `run` closed it");
        assert_eq!(channel.admit(&said("being closed")).await, Ok(()), "until `run` closes it");
        applied().await;
        assert!(closing.await.unwrap().expect("closed"), "by its opener");
        assert!(!channel.close_thread_by_procedure(&system, thread).await.unwrap(), "closed once");
        assert_eq!(
            channel.admit(&said("once closed")).await,
            Err(Rejection::ThreadClosed),
            "nothing more once it is closed"
        );
        assert!(
            !channel.close_thread_by_procedure(&system, line).await.unwrap(),
            "and the line never"
        );
    }

    /// A channel of alice and bob with a thread at its line delegated into by bob, to alice, in
    /// her chain, its brief given, whose delegate is forged (`ChannelLoop::forge_delegate`): what
    /// the delegate gives back is admitted as the delegation says (issue 557).
    async fn with_a_delegate(alice: &User, bob: &User) -> (Arc<ChannelLoop>, ThreadId) {
        let channel = channel_with(&[alice, bob], Arc::new(AllowAllPolicy::new())).await;
        let (hers, his) = (ActorId::User(alice.id), ActorId::User(bob.id));
        let message = Message {
            sender_id: his,
            content: Some("look this up".to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let brief = Brief { message, given_at: Some(Position::START) };
        let briefs = VecDeque::from([brief]);
        let delegation =
            Delegation { giver: his, recipient: hers, initiator: hers, briefs, word: Word::NotDue };
        let thread = channel.forge_delegate(delegation, |_| Vec::new()).await;
        (channel, thread)
    }

    /// `sender` gives `recipient` in `thread_id` a message of `giver`, classified
    /// `classification`, to read, in the chain of `giver` (issue 557).
    fn given(
        channel: &ChannelLoop,
        sender: ActorId,
        thread_id: ThreadId,
        recipient: ActorId,
        giver: ActorId,
        classification: Classification,
    ) -> Envelope {
        let context = EventContext {
            sender_id: sender,
            initiator_id: giver,
            thread_id,
            ..channel.event_context()
        };
        let message = Message {
            sender_id: giver,
            role: MessageRole::User,
            content: Some("found it".to_string()),
            ..Message::new(classification)
        };
        Envelope::new(context, ActorEvent::GivenToRead { recipient, message })
    }

    /// Issue 557: what a delegate gives is admitted as its thread's delegation says - what came of
    /// a brief given, back where the thread was opened, of its recipient to its giver, in the
    /// delegating chain's name -, from the delegate of a thread opened alone, to whom runs there,
    /// as a text of a classification the channel may hold.
    #[tokio::test]
    async fn a_delegate_gives_what_its_delegation_says_alone() {
        let (alice, bob) = (make_user("alice"), make_user("bob"));
        let (channel, thread) = with_a_delegate(&alice, &bob).await;
        let hers = ActorId::User(alice.id);
        let his = ActorId::User(bob.id);
        let line = ThreadId::line(channel.id);
        let delegate = ActorId::Delegate(thread);
        let given = |sender, thread_id, recipient, giver, classification| {
            given(&channel, sender, thread_id, recipient, giver, classification)
        };
        let fits = Classification::CHANNEL_DEFAULT;
        let back = given(delegate, line, his, hers, fits.clone());
        assert_eq!(channel.admit(&back).await, Ok(()), "back where the thread was opened");
        let refused = [
            (given(hers, line, his, hers, fits.clone()), "from one who is no delegate"),
            (
                given(ActorId::Delegate(ThreadId::new()), line, his, hers, fits.clone()),
                "from the delegate of a thread not opened at it",
            ),
            (given(ActorId::Delegate(line), line, his, hers, fits.clone()), "from the line's"),
            (given(delegate, thread, his, hers, fits.clone()), "into its thread but the brief"),
            (given(delegate, thread, delegate, hers, fits.clone()), "to itself"),
            (given(delegate, line, hers, hers, fits.clone()), "back to another than its giver"),
            (
                given(delegate, line, his, ActorId::Agent(AgentId::new()), fits.clone()),
                "of another than its recipient",
            ),
            (
                Envelope {
                    context: EventContext { initiator_id: his, ..back.context.clone() },
                    ..back.clone()
                },
                "in another chain than the one that delegated",
            ),
            (
                Envelope {
                    event: ActorEvent::GivenToRead {
                        recipient: his,
                        message: Message {
                            sender_id: hers,
                            tool_calls: vec![make_tool_call("calculator")],
                            ..Message::new(fits.clone())
                        },
                    },
                    ..back.clone()
                },
                "but a text",
            ),
        ];
        for (envelope, why) in refused {
            assert_eq!(channel.admit(&envelope).await, Err(Rejection::Invalid), "not {why}");
        }
        let above = Classification::general(Confidentiality::StrictlyConfidential);
        assert_eq!(
            channel.admit(&given(delegate, line, his, hers, above)).await,
            Err(Rejection::ClassificationRefused),
            "nor what may not flow in"
        );
    }

    /// Issue 557: what is given to read is recorded to its recipient by name and to the users, and
    /// the recipient alone is asked to respond, for being given it.
    #[tokio::test]
    async fn something_given_to_read_asks_its_recipient_alone() {
        let (alice, bob) = (make_user("alice"), make_user("bob"));
        let (channel, thread) = with_a_delegate(&alice, &bob).await;
        let hers = ActorId::User(alice.id);
        let his = ActorId::User(bob.id);
        let delegate = ActorId::Delegate(thread);
        let (to_alice, mut alices) = opened();
        let (to_bob, mut bobs) = opened();
        let running = |id, inbox| RunningActor {
            id,
            abort_handle: tokio::spawn(async {}).abort_handle(),
            task: TaskTracker::new(),
            inbox: Some(inbox),
            leaving: None,
            ended: Vec::new(),
        };
        let actors = HashMap::from([(hers, running(hers, to_alice)), (his, running(his, to_bob))]);
        let line = ThreadId::line(channel.id);
        let back = given(&channel, delegate, line, his, hers, Classification::CHANNEL_DEFAULT);
        channel.process_envelope(back, &actors).await;
        let kinds = async |inbox: &mut InboxReceiver| {
            let mut kinds = Vec::new();
            while let Some(envelope) = inbox.try_recv().await {
                kinds.push(
                    envelope.event.request_reason().map_or(envelope.event.kind(), |_| "asked"),
                );
            }
            kinds
        };
        assert_eq!(kinds(&mut bobs).await, ["GivenToRead", "asked"], "given it, then asked");
        assert_eq!(kinds(&mut alices).await, ["GivenToRead"], "shown it, and not asked");
        let recorded = channel
            .reading_history(|history| {
                history
                    .entries()
                    .map(|node| (node.entry.entry.clone(), node.entry.recipients.clone()))
                    .collect::<Vec<_>>()
            })
            .await;
        let to_him_and_the_users =
            Recipients { named: vec![his], ..Recipients::participants(false) };
        assert!(
            matches!(
                recorded.as_slice(),
                [
                    ..,
                    (HistoryEntry::GivenToRead { by, .. }, to_both),
                    (
                        HistoryEntry::ResponseRequested { reason: RequestReason::Given, .. },
                        to_him
                    ),
                ] if *by == delegate
                    && *to_both == to_him_and_the_users
                    && *to_him == Recipients::only(vec![his])
            ),
            "recorded to him by name and to the users, his request to him: {recorded:?}"
        );
    }

    /// Issue 417: what enters the channel is recorded with its sender and
    /// whom the moderator delivered it to - everybody, as every channel
    /// delivers, or some alone.
    #[tokio::test]
    async fn what_enters_is_recorded_with_its_sender_and_whom_it_went_to() {
        let (alice, bob) = (make_user("alice"), make_user("bob"));
        let said = |channel: &ChannelLoop, text: &str| {
            Envelope::new(
                EventContext { sender_id: ActorId::User(alice.id), ..channel.event_context() },
                ActorEvent::CompleteMessageGenerated(Message {
                    sender_id: ActorId::User(alice.id),
                    role: MessageRole::User,
                    content: Some(text.to_string()),
                    ..Message::new(Classification::CHANNEL_DEFAULT)
                }),
            )
        };
        let recorded = |channel: Arc<ChannelLoop>| async move {
            channel
                .reading_history(|history| {
                    history
                        .entries()
                        .find(|node| node.entry.entry.said().is_some())
                        .map(|node| (node.entry.sender, node.entry.recipients.clone()))
                })
                .await
        };

        let everybody = channel_with(&[&alice, &bob], Arc::new(AllowAllPolicy::new())).await;
        everybody.let_in(said(&everybody, "hello"), &HashMap::new()).await;
        assert_eq!(
            recorded(Arc::clone(&everybody)).await,
            Some((ActorId::User(alice.id), Recipients::participants(true))),
            "as every channel delivers: to every participant"
        );

        let to_bob =
            channel_moderated_by(&[&alice, &bob], Arc::new(AskBack(ActorId::User(bob.id)))).await;
        to_bob.let_in(said(&to_bob, "psst"), &HashMap::new()).await;
        assert_eq!(
            recorded(Arc::clone(&to_bob)).await,
            Some((ActorId::User(alice.id), Recipients::only(vec![ActorId::User(bob.id)]))),
            "to whom the moderator named"
        );
        let asked = to_bob
            .reading_history(|history| {
                history.last().map(|node| {
                    (node.entry.entry.clone(), node.entry.sender, node.entry.recipients.clone())
                })
            })
            .await;
        assert!(
            matches!(asked, Some((HistoryEntry::ResponseRequested { by, reason: RequestReason::Mentioned, .. }, ActorId::System, ref recipients)) if by == ActorId::User(alice.id) && *recipients == Recipients::only(vec![ActorId::User(bob.id)])),
            "and whom it asked to respond, after it, in the system's name, and why: {asked:?}"
        );
    }

    /// Issue 417: what the channel delivers carries the position of its
    /// entry - the event as it entered, and the request to respond the
    /// channel records after it -, so that a client tells the live event
    /// from the item it reads later.
    #[tokio::test]
    async fn what_is_delivered_carries_the_position_of_its_entry() {
        let (alice, bob) = (make_user("alice"), make_user("bob"));
        let channel =
            channel_moderated_by(&[&alice, &bob], Arc::new(AskBack(ActorId::User(bob.id)))).await;
        let (tx, mut rx) = opened();
        let his = ActorId::User(bob.id);
        let abort_handle = tokio::spawn(async {}).abort_handle();
        let running = HashMap::from([(
            his,
            RunningActor {
                id: his,
                abort_handle,
                task: TaskTracker::new(),
                inbox: Some(tx),
                leaving: None,
                ended: Vec::new(),
            },
        )]);
        let hers = ActorId::User(alice.id);
        let said = Message {
            sender_id: hers,
            role: MessageRole::User,
            content: Some("hello".to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        channel
            .let_in(
                Envelope::new(
                    EventContext { sender_id: hers, ..channel.event_context() },
                    ActorEvent::CompleteMessageGenerated(said),
                ),
                &running,
            )
            .await;

        let delivered = rx.try_recv().await.expect("delivered");
        let asked = rx.try_recv().await.expect("and he is asked");
        let (said_at, asked_at) = channel
            .reading_history(|history| {
                let at = |position: Option<Position>| {
                    position
                        .and_then(|seq| history.from(seq).next())
                        .map(|node| node.entry.entry.clone())
                };
                (at(delivered.context.position), at(asked.context.position))
            })
            .await;
        assert!(
            matches!(said_at, Some(HistoryEntry::Said { .. })),
            "the message, at its entry: {:?}",
            delivered.context.position
        );
        assert!(
            matches!(asked_at, Some(HistoryEntry::ResponseRequested { .. })),
            "the request, at its own: {:?}",
            asked.context.position
        );
        assert!(
            matches!(
                asked.event,
                ActorEvent::ResponseRequested { reason: RequestReason::Mentioned }
            ),
            "for the reason the moderator gave: {:?}",
            asked.event
        );
        assert!(asked.context.position > delivered.context.position, "recorded after it");
    }

    /// Issue 417: what only its sender's owner sees enters the channel and is
    /// recorded with it, and reaches nobody else - neither the moderator,
    /// which decides on what everybody may see, nor a participant; a message
    /// of which nothing is left to show is recorded as gone to nobody.
    #[tokio::test]
    async fn what_only_its_owner_sees_is_recorded_and_given_to_nobody_else() {
        use caelumora_domain::confidentiality::Visibility;
        use caelumora_domain::tool::ToolVisibility;
        /// Asks nobody, and keeps what it was given.
        #[derive(Debug, Default)]
        struct Told(Mutex<Vec<Envelope>>);
        #[async_trait::async_trait]
        impl Moderator for Told {
            async fn route(&self, event: &Envelope, _context: &SecurityContext) -> Routing {
                self.0.lock().unwrap().push(event.clone());
                Routing::default()
            }
            fn add_actor(&self, _: Arc<dyn Actor>) {}
            fn remove_actor(&self, _: ActorId) {}
        }
        let alice = make_user("alice");
        let told = Arc::new(Told::default());
        let channel = channel_moderated_by(&[&alice], Arc::<Told>::clone(&told)).await;
        let (tx, mut rx) = opened();
        let hers = ActorId::User(alice.id);
        let abort_handle = tokio::spawn(async {}).abort_handle();
        let running = HashMap::from([(
            hers,
            RunningActor {
                id: hers,
                abort_handle,
                task: TaskTracker::new(),
                inbox: Some(tx),
                leaving: None,
                ended: Vec::new(),
            },
        )]);
        let agent = ActorId::Agent(AgentId::new());
        let hidden = ToolCall {
            id: Some("1".to_string()),
            arguments: serde_json::json!({}),
            visibility: ToolVisibility::HIDDEN,
            ..make_tool_call("look")
        };
        let thinking = Message {
            sender_id: agent,
            role: MessageRole::Assistant,
            reasoning: Some("weighing".to_string()),
            reasoning_visibility: Visibility::Owner,
            tool_calls: vec![hidden],
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let said = |message: Message| {
            Envelope::new(
                EventContext { sender_id: agent, ..channel.event_context() },
                ActorEvent::CompleteMessageGenerated(message),
            )
        };
        let parts = |event: &ActorEvent| match event {
            ActorEvent::CompleteMessageGenerated(m) => {
                (m.content.clone(), m.reasoning.clone(), m.tool_calls.len())
            }
            other => panic!("expected a message, got {}", other.kind()),
        };
        let recorded = || async {
            let last = channel
                .reading_history(|history| history.last().expect("recorded").entry.clone())
                .await;
            (
                last.entry
                    .said()
                    .map(|m| (m.content.clone(), m.reasoning.clone(), m.tool_calls.len())),
                last.recipients,
            )
        };

        channel
            .let_in(
                said(Message { content: Some("found it".to_string()), ..thinking.clone() }),
                &running,
            )
            .await;
        assert_eq!(
            parts(&rx.try_recv().await.expect("delivered").event),
            (Some("found it".to_string()), None, 0),
            "alice is given what everybody may see"
        );
        assert_eq!(
            told.0.lock().unwrap().iter().map(|e| parts(&e.event)).collect::<Vec<_>>(),
            [(Some("found it".to_string()), None, 0)],
            "and so is the moderator"
        );
        assert_eq!(
            recorded().await,
            (
                Some((Some("found it".to_string()), Some("weighing".to_string()), 1)),
                Recipients::participants(true)
            ),
            "the channel records it whole"
        );

        channel.let_in(said(thinking), &running).await;
        assert!(rx.try_recv().await.is_none(), "nothing of it is left to show");
        assert_eq!(told.0.lock().unwrap().len(), 1, "nor to decide on");
        assert_eq!(
            recorded().await,
            (Some((None, Some("weighing".to_string()), 1)), Recipients::NOBODY),
            "recorded whole, as gone to nobody"
        );
    }

    /// Issue 404: what others hear of a message is announced with the message once it is stored, as
    /// news of the channel, with its thread; a step of a plan, a notice of the kernel or what is
    /// said in another thread than the line without mentioning a user (issue 557) is not.
    #[tokio::test]
    async fn what_others_hear_is_announced_once_it_is_stored() {
        use crate::session::testing::{running_session_of_a_user, spec, test_kernel};
        use caelumora_domain::channel::{Channel, Said, SAID};
        let kernel = test_kernel();
        let (session, ctx, _) = running_session_of_a_user(&kernel).await;
        let channel = session.create_channel(&ctx, spec("side", vec![])).await.unwrap();
        let owner = ctx.initiator_id;
        let said = |message: &Message| {
            Envelope::new(
                EventContext { sender_id: owner, ..channel.event_context() },
                ActorEvent::CompleteMessageGenerated(message.clone()),
            )
        };
        let heard = Message {
            id: MessageId::new(),
            role: MessageRole::User,
            sender_id: owner,
            content: Some("hello".to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        };
        let step = Message { id: MessageId::new(), internal: true, ..heard.clone() };
        let notice = Message { id: MessageId::new(), role: MessageRole::System, ..heard.clone() };
        let aside = Message { id: MessageId::new(), ..heard.clone() };
        let mentioning = Message { id: MessageId::new(), addressees: vec![owner], ..heard.clone() };
        let agent = ActorId::Agent(AgentId::new());
        let addressing = Message { id: MessageId::new(), addressees: vec![agent], ..heard.clone() };
        let mut rx = kernel.event_bus().subscribe_all();

        for message in [&heard, &step, &notice] {
            channel.append_history(&said(message), Audience::Everybody).await;
        }
        let thread_id = ThreadId::new();
        for message in [&aside, &mentioning, &addressing] {
            let in_thread = said(message);
            let context = EventContext { thread_id, ..in_thread.context };
            channel.append_history(&Envelope { context, ..in_thread }, Audience::Everybody).await;
        }

        let announced: Vec<(MessageId, ThreadId)> = rx
            .drain()
            .await
            .iter()
            .filter_map(|event| event.about::<Channel>().filter(|((_, id), _)| *id == channel.id()))
            .filter_map(|(_, change)| change.data::<Said>(SAID))
            .map(|said| (said.message.id, said.thread))
            .collect();
        let line = ThreadId::line(channel.id());
        assert_eq!(
            announced,
            [(heard.id, line), (mentioning.id, thread_id)],
            "the words others hear in the line, and a mention of a user in another thread, with its \
             thread"
        );
        assert_eq!(channel.history().await.len(), 6, "though every one is kept");
    }

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn a_message_carries_only_what_its_sender_uploaded_for_the_channel() {
        let world = World::new().await;
        let session = world.shared_session(&[], &[]).await;
        let main = Arc::clone(&session.channels()[0]);
        let channel = main.channel();
        let hers = World::as_user(world.alice);
        let attachments = world.kernel.attachment_manager();
        let upload = || attachments.create(&hers, &channel, "a.txt", None, (1, file("a")));
        let (alice, bob) = (ActorId::User(world.alice), ActorId::User(world.bob));
        let sent = |sender: ActorId, event: ActorEvent| {
            Envelope::new(EventContext { sender_id: sender, ..main.event_context() }, event)
        };
        let said = |sender: ActorId, attachments: Vec<ContentRef>| {
            sent(
                sender,
                ActorEvent::CompleteMessageGenerated(Message {
                    sender_id: sender,
                    role: MessageRole::User,
                    attachments,
                    ..Message::new(main.classification())
                }),
            )
        };

        let hers = upload().await.unwrap();
        assert_eq!(
            main.admit(&said(bob, vec![hers.clone()])).await,
            Err(Rejection::Invalid),
            "not his"
        );
        let in_her_name = sent(
            bob,
            ActorEvent::CompleteMessageGenerated(Message {
                sender_id: alice,
                role: MessageRole::User,
                attachments: vec![hers.clone()],
                ..Message::new(main.classification())
            }),
        );
        assert_eq!(
            main.admit(&in_her_name).await,
            Err(Rejection::Invalid),
            "not his for a message naming her"
        );
        let call = ToolCall { id: None, arguments: serde_json::json!({}), ..make_tool_call("f") };
        let giving = |file: ContentRef| {
            let result = ToolCallResult {
                files: vec![file],
                ..ToolCallResult::of(
                    &call,
                    Default::default(),
                    "done".to_string(),
                    main.classification(),
                )
            };
            sent(
                alice,
                ActorEvent::CompleteMessageGenerated(Message {
                    sender_id: alice,
                    role: MessageRole::Tool,
                    tool_results: vec![result],
                    ..Message::new(main.classification())
                }),
            )
        };
        let png = ToolContent {
            name: "d.png".to_string(),
            bytes: Bytes::from_static(b"\x89PNG\r\n\x1a\ndrawn"),
        };
        let drawn = attachments
            .create_content(
                &World::as_user(world.alice),
                &channel,
                main.classification(),
                vec![png],
            )
            .await
            .unwrap()
            .remove(0);
        assert_eq!(
            main.admit(&giving(drawn.clone())).await,
            Ok(()),
            "what a tool gave (issue 512): kept for the channel"
        );
        assert_eq!(
            main.admit(&giving(drawn.clone())).await,
            Ok(()),
            "and waiting for no message, so claimed by none"
        );
        let drawn_elsewhere = ContentRef {
            key: format!("attachments/{}/{}", ChannelId::new(), drawn.id),
            ..drawn.clone()
        };
        assert_eq!(
            main.admit(&giving(drawn_elsewhere)).await,
            Err(Rejection::Invalid),
            "not another channel's"
        );
        let drawn_above = ContentRef {
            classification: Classification::general(Confidentiality::StrictlyConfidential),
            ..drawn
        };
        assert_eq!(
            main.admit(&giving(drawn_above)).await,
            Err(Rejection::ClassificationRefused),
            "nor what may not flow in"
        );
        let elsewhere = ContentRef {
            key: format!("attachments/{}/{}", ChannelId::new(), hers.id),
            ..hers.clone()
        };
        let showing = |shown: ContentRef| {
            sent(
                alice,
                ActorEvent::CompleteMessageGenerated(Message {
                    sender_id: alice,
                    role: MessageRole::Tool,
                    tool_results: vec![ToolCallResult {
                        shown: vec![shown],
                        ..ToolCallResult::of(
                            &call,
                            Default::default(),
                            "shown".to_string(),
                            main.classification(),
                        )
                    }],
                    ..Message::new(main.classification())
                }),
            )
        };
        assert_eq!(
            main.admit(&showing(hers.clone())).await,
            Ok(()),
            "what a result showed: one of the channel's"
        );
        assert_eq!(
            main.admit(&showing(elsewhere.clone())).await,
            Err(Rejection::Invalid),
            "not another channel's"
        );
        let above = ContentRef {
            classification: Classification::general(Confidentiality::StrictlyConfidential),
            ..hers.clone()
        };
        assert_eq!(
            main.admit(&showing(above)).await,
            Err(Rejection::ClassificationRefused),
            "nor what may not flow in"
        );
        assert_eq!(
            main.admit(&said(alice, vec![elsewhere])).await,
            Err(Rejection::Invalid),
            "only as it was uploaded"
        );
        let mut many = Vec::new();
        for _ in 0..11 {
            many.push(upload().await.unwrap());
        }
        assert_eq!(
            main.admit(&said(alice, many.clone())).await,
            Err(Rejection::Invalid),
            "more than a message carries"
        );
        assert_eq!(main.admit(&said(alice, many[..10].to_vec())).await, Ok(()), "and claimed so");
        assert_eq!(
            main.admit(&said(alice, many[..1].to_vec())).await,
            Err(Rejection::Invalid),
            "by one message only"
        );

        let actor = main.user_actor(world.alice).unwrap();
        let mut watch = actor.watch();
        actor
            .send_message(None, None, None, vec![hers.id])
            .await
            .expect("an attachment alone is a message");
        let ActorEvent::CompleteMessageGenerated(message) =
            shown(&mut watch, "CompleteMessageGenerated").await
        else {
            unreachable!()
        };
        assert_eq!(message.attachments, slice::from_ref(&hers));
        assert_eq!(
            main.admit(&said(alice, vec![hers.clone()])).await,
            Err(Rejection::Invalid),
            "claimed by the message said"
        );

        let modified = |attachments: Vec<ContentRef>| {
            sent(alice, ActorEvent::MessageModified(Message { attachments, ..message.clone() }))
        };
        let added = upload().await.unwrap();
        assert_eq!(
            main.admit(&modified(vec![hers.clone(), added.clone()])).await,
            Ok(()),
            "what it carries, and what she uploaded"
        );
        let his = attachments
            .create(&World::as_user(world.bob), &channel, "b.txt", None, (1, file("b")))
            .await
            .unwrap();
        assert_eq!(
            main.admit(&modified(vec![hers.clone(), his])).await,
            Err(Rejection::Invalid),
            "never another's upload"
        );

        // Taken back, it carries nothing any more: of what it carried her actor finds none to make
        // a modification of, and refuses at once; one naming none the channel rejects (issue 567).
        actor.revoke_message(message.id).await.expect("hers to take back");
        time::timeout(Duration::from_secs(5), async {
            while main.message(message.id).await.is_none_or(|m| m.revoked.is_none()) {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("taken back");
        let refused = actor
            .modify_message(message.id, None, None, vec![hers.id])
            .await
            .expect_err("nothing it carried is found")
            .downcast::<AttachmentError>()
            .expect("an attachment's error");
        assert!(matches!(refused, AttachmentError::NotFound(id) if id == hers.id));
        actor
            .modify_message(message.id, Some("again".to_string()), None, Vec::new())
            .await
            .expect("for the channel to reject");
    }

    /// Issue 626: what was queued as the channel took an envelope is taken with it, the pieces of
    /// each sender's partial message taken in by its last envelope - past what another sent in
    /// between, which keeps its place, but not past another envelope of its own -; what arrives
    /// while they are taken waits for the next round.
    #[test]
    fn the_queued_pieces_of_a_partial_message_are_taken_as_one() {
        use caelumora_domain::actor::testing::context_in;
        use caelumora_domain::message::testing::make_message;
        let context = context_in(ChannelId::new());
        let talker = ActorId::Agent(AgentId::new());
        let other = ActorId::Agent(AgentId::new());
        let partial = |sender: ActorId, text: &str| {
            let piece = Message {
                sender_id: sender,
                role: MessageRole::Assistant,
                content: Some(text.to_string()),
                ..make_message(MessageId::new())
            };
            Envelope::new(
                EventContext { sender_id: sender, ..context.clone() },
                ActorEvent::PartialMessageGenerated(piece),
            )
        };
        let started = |sender: ActorId| {
            Envelope::new(
                EventContext { sender_id: sender, ..context.clone() },
                ActorEvent::ResponseStarted { turn_id: TurnId::new() },
            )
        };
        let (tx, mut queue) = mpsc::channel(INPUT_CAPACITY);
        for envelope in [
            partial(talker, "lo"),
            started(other),
            partial(talker, ", "),
            partial(other, "Hi"),
            partial(talker, "world"),
            started(talker),
            partial(talker, "again"),
        ] {
            tx.try_send(envelope).expect("room");
        }
        let taken = taken_together(partial(talker, "Hel"), &mut queue);
        tx.try_send(partial(talker, "!")).expect("room");

        let said: Vec<_> = taken
            .iter()
            .map(|envelope| match &envelope.event {
                ActorEvent::PartialMessageGenerated(message) => {
                    (envelope.context.sender_id, message.content.clone().unwrap_or_default())
                }
                event => (envelope.context.sender_id, event.kind().to_string()),
            })
            .collect();
        let as_said = |sender, text: &str| (sender, text.to_string());
        assert_eq!(
            said,
            [
                as_said(talker, "Hello, world"),
                as_said(other, "ResponseStarted"),
                as_said(other, "Hi"),
                as_said(talker, "ResponseStarted"),
                as_said(talker, "again"),
            ],
            "the pieces of each sender as one, every sender's envelopes in their order"
        );
        let next = queue.try_recv().expect("what arrived later is queued still");
        let ActorEvent::PartialMessageGenerated(message) = &next.event else {
            panic!("a partial, got {}", next.event.kind())
        };
        assert_eq!(message.content.as_deref(), Some("!"), "for the next round");
    }
}
