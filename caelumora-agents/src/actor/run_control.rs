//! The run control of an agent's actor - an `AgentActor` or an
//! `ExternalAgentActor` (issue 363): what its run loop shares with
//! `cancel`, `retry`, `stop`, `leave`, `wakeup` and `interrupt_call` (see
//! `RunControl`). Held by its body (`AgentBody`).

use crate::tool::Interruption;
use caelumora_domain::actor::{ActorState, ProcessingStateHolder};
use caelumora_domain::agent::AgentChannelState;
use caelumora_domain::channel::ThreadId;
use caelumora_domain::work::TurnId;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::pending;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard};
use tokio::sync::{oneshot, Notify};
use tokio_util::sync::CancellationToken;

/// What `cancel`, `retry`, `stop` or `leave` asked of the run loop, which
/// takes it after every step and alone decides what follows (issue 152).
/// Leaving wins over everything, then a stop; of a cancel and a retry, the
/// first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Interrupt {
    /// End the turn under way and take it over (`Cancelling`).
    Cancel = 1,
    /// Keep the turn under way at the step it stands at and end the run
    /// loop: the channel stops, and the turn is resumed when it runs again
    /// (issue 377).
    Stop = 2,
    /// Give the turn under way up, because its answer was asked for again
    /// (issue 235): a turn of its own answers instead.
    Retry = 3,
    /// Give the turn under way up and end the run loop: the actor left the
    /// channel.
    Leave = 4,
}

/// `RunControl::interrupt` when nothing was asked.
const NO_INTERRUPT: u8 = 0;

/// Where the run loop of an agent stands, and what `cancel`, `stop`,
/// `leave` and `wakeup` asked of it. These come from other tasks and are synchronous:
/// they only leave word here and interrupt the step under way; the run loop
/// takes the word after every step and alone decides what follows (issue
/// 152). The same for both kinds of agent (issue 363): an external agent's
/// answer is a turn here, with a token of its own, and it never sleeps nor
/// has calls to interrupt on their own.
#[derive(Debug)]
pub(crate) struct RunControl {
    state: ProcessingStateHolder,
    /// The state the run loop last announced (issue 243).
    announced: ProcessingStateHolder,
    /// Tells the run loop, waiting between turns, that `wakeup` woke it:
    /// the channel moves the state itself, and the run loop announces it.
    woken: Notify,
    /// The turn that runs or ran last, by its thread (issue 602) and its id (issue 235), and its
    /// token, all replaced when the next one begins. The token is cancelled by `cancel`, `stop` and
    /// `leave` while the turn runs: the streaming loop waits on it next to the model's stream, so
    /// that a cancel drops the request instead of waiting for its next chunk. `None` before the
    /// first turn, and while no turn runs since the last was set aside (`set_aside`). A `std`
    /// mutex, because `cancel` is synchronous. Every token is a child of `ending`.
    turn: Mutex<(Option<(ThreadId, TurnId)>, CancellationToken)>,
    /// Cancelled by `stop` and `leave`: the run loop is to end. A turn begun
    /// after it finds its token cancelled already; a run loop that waits for
    /// more than a turn - for its input, or an external agent following its
    /// open tasks - waits for it too (`ending`).
    ending: CancellationToken,
    /// Set by `leave`, for good: the actor left the channel, and what goes
    /// on elsewhere on its behalf ends with it (`has_left`). Unlike the
    /// interrupt, it is not taken.
    left: AtomicBool,
    /// What `cancel`, `retry`, `stop` or `leave` asked of the run loop (an
    /// `Interrupt`, or `NO_INTERRUPT`), taken after every step.
    interrupt: AtomicU8,
    /// The turns set aside because they wait for a person (issue 602), by their threads - one a
    /// thread -, each with what `cancel` or `retry` asked of it meanwhile (`Interrupt::Cancel`,
    /// `Interrupt::Retry`), which the run loop takes up as it resumes the turn (`resume`). The turn
    /// that runs is never among them.
    set_aside: Mutex<BTreeMap<ThreadId, (TurnId, Option<Interrupt>)>>,
    /// Tells the run loop, waiting between turns, that `cancel` or `retry` asked a turn set aside
    /// to end.
    asked: Notify,
    /// The calls of the last tool step that can be interrupted on their own
    /// (issue 276), by their ids, each with where the name of whoever
    /// interrupts it goes. The run loop holds the other end while the call
    /// runs or waits to run, and drops it after: a call that ran can no
    /// longer be interrupted.
    calls: Mutex<HashMap<String, oneshot::Sender<String>>>,
}

impl RunControl {
    /// The run control of an agent that stands at `state`, in the turn `turn` of its thread if it
    /// resumes one.
    pub(crate) fn new(state: ActorState, turn: Option<(ThreadId, TurnId)>) -> Self {
        let ending = CancellationToken::new();
        Self {
            state: ProcessingStateHolder::new(state),
            announced: ProcessingStateHolder::new(state),
            woken: Notify::new(),
            turn: Mutex::new((turn, ending.child_token())),
            ending,
            left: AtomicBool::new(false),
            interrupt: AtomicU8::new(NO_INTERRUPT),
            set_aside: Mutex::new(BTreeMap::new()),
            asked: Notify::new(),
            calls: Mutex::new(HashMap::new()),
        }
    }

    /// The run control of an agent whose state in its channel is `state`, as it resumes: in the
    /// turn stored under way that goes on by itself (`AgentThreadState::has_work_under_way`), in
    /// whichever thread - one at a time (issue 557) -, at the step it stands at
    /// (`TurnState::step`), or else asleep or idle as it stood outside a turn, its own as a whole
    /// (`AgentChannelState::asleep`); every other turn stored - one waiting for a person - set
    /// aside (issue 602).
    pub(crate) fn resuming(state: &AgentChannelState) -> Self {
        // Asleep, it takes no turn up before it is woken: every turn waits.
        let resumed = state.threads.iter().filter(|_| !state.asleep).find_map(|(thread, part)| {
            let turn = part.turn.as_ref().filter(|_| part.has_work_under_way())?;
            Some((turn.step, (*thread, turn.work.turn_id)))
        });
        let control = match resumed {
            Some((step, turn)) => Self::new(step, Some(turn)),
            None if state.asleep => Self::new(ActorState::Sleeping, None),
            None => Self::new(ActorState::Idle, None),
        };
        let running = resumed.map(|(_, (thread, _))| thread);
        let waiting = state.threads.iter().filter(|(thread, _)| Some(**thread) != running);
        *control.lock_set_aside() = waiting
            .filter_map(|(thread, part)| Some((*thread, (part.turn.as_ref()?.work.turn_id, None))))
            .collect();
        // What it tells first, as a whole, counts as told.
        control.announced.store(control.standing());
        control
    }

    /// Where the run loop stands.
    pub(crate) fn state(&self) -> ActorState {
        self.state.load()
    }

    /// Moves the run loop to `state`.
    pub(crate) fn go_to(&self, state: ActorState) {
        self.state.store(state);
    }

    /// Ends a sleep. Only a sleeping agent is woken; forcing `Idle` from
    /// any other state (e.g. mid-`Planning`) would corrupt a turn under way.
    /// The run loop is told (`woken`), so that it announces it.
    pub(crate) fn wakeup(&self) {
        if self.state.replace(ActorState::Sleeping, ActorState::Idle) {
            self.woken.notify_one();
        }
    }

    /// Returns once `wakeup` woke the agent - at once if it did since this
    /// was last waited for.
    pub(crate) async fn woken(&self) {
        self.woken.notified().await;
    }

    /// Where the agent stands as a whole (issue 602): where the run loop stands, but waiting for a
    /// person while it is idle and a turn set aside waits for one - what it tells of itself.
    pub(crate) fn standing(&self) -> ActorState {
        match self.state() {
            ActorState::Idle if !self.lock_set_aside().is_empty() => ActorState::WaitingForUser,
            state => state,
        }
    }

    /// Where the agent stands as a whole (`standing`), if it has not announced it yet (issue
    /// 243); it counts as announced from now.
    pub(crate) fn unannounced(&self) -> Option<ActorState> {
        let state = self.standing();
        (self.announced.swap(state) != state).then_some(state)
    }

    /// The turn `turn` begins in `thread`: it gets a token of its own, and a cancel or a retry that
    /// came as the turn before ended has nothing to end here; a stop or leaving still has.
    pub(crate) fn begin_turn(&self, thread: ThreadId, turn: TurnId) {
        *self.lock_turn() = (Some((thread, turn)), self.ending.child_token());
        let _ = self.interrupt.try_update(Ordering::SeqCst, Ordering::SeqCst, |asked| {
            (!ends_run(asked)).then_some(NO_INTERRUPT)
        });
    }

    /// The turn under way in `thread` that can still be ended (issues 235, 602): the one that runs
    /// there, if the run loop is in it and it is not ending already, which a cancel or a retry
    /// would find over - else the one set aside there.
    pub(crate) fn turn_under_way(&self, thread: ThreadId) -> Option<TurnId> {
        match self.under_way() {
            Some((there, turn)) if there == thread => Some(turn),
            _ => self.lock_set_aside().get(&thread).map(|(turn, _)| *turn),
        }
    }

    /// The running turn waits for a person (issue 602): it is set aside, its thread holding it, and
    /// no turn runs; the run loop goes on between turns, and resumes it once it goes on by itself
    /// (`resume`). A cancel or a retry asked of it since its step ended, which the run loop has
    /// not taken, goes with it, and the run loop is told (`asked`). Nothing to set aside if no turn
    /// runs.
    pub(crate) fn set_aside(&self) {
        let mut running = self.lock_turn();
        if let Some((thread, turn)) = running.0.take() {
            let asked = self.take_turn_interrupt();
            self.lock_set_aside().insert(thread, (turn, asked));
            if asked.is_some() {
                self.asked.notify_one();
            }
        }
        running.1 = self.ending.child_token();
    }

    /// The turn set aside in `thread` runs again at `step` (issue 602), with a token of its own,
    /// as a turn begins - one step under the turn's lock, so that a cancel or a retry finds it
    /// either set aside or running -; what `cancel` or `retry` asked of it meanwhile, for the run
    /// loop to take up. `None` if no turn is set aside there.
    pub(crate) fn resume(&self, thread: ThreadId, step: ActorState) -> Option<Option<Interrupt>> {
        let mut running = self.lock_turn();
        let (turn, asked) = self.lock_set_aside().remove(&thread)?;
        *running = (Some((thread, turn)), self.ending.child_token());
        self.go_to(step);
        self.take_turn_interrupt();
        Some(asked)
    }

    /// Takes a cancel or a retry asked of the turn that runs, leaving a stop or leaving where it
    /// is.
    fn take_turn_interrupt(&self) -> Option<Interrupt> {
        let taken = self.interrupt.try_update(Ordering::SeqCst, Ordering::SeqCst, |asked| {
            (!ends_run(asked) && asked != NO_INTERRUPT).then_some(NO_INTERRUPT)
        });
        match taken {
            Ok(asked) if asked == Interrupt::Cancel as u8 => Some(Interrupt::Cancel),
            Ok(asked) if asked == Interrupt::Retry as u8 => Some(Interrupt::Retry),
            _ => None,
        }
    }

    /// The threads of the turns set aside (issue 602).
    pub(crate) fn set_aside_threads(&self) -> BTreeSet<ThreadId> {
        self.lock_set_aside().keys().copied().collect()
    }

    /// The thread of a turn set aside that `cancel` or `retry` asked to end (issue 602), if any:
    /// for the run loop to resume it and end it.
    pub(crate) fn asked_of_set_aside(&self) -> Option<ThreadId> {
        let set_aside = self.lock_set_aside();
        set_aside.iter().find_map(|(thread, (_, asked))| asked.map(|_| *thread))
    }

    /// Returns once `cancel` or `retry` asked a turn set aside to end - at once if one did since
    /// this was last waited for.
    pub(crate) async fn asked(&self) {
        self.asked.notified().await;
    }

    fn lock_set_aside(&self) -> MutexGuard<'_, BTreeMap<ThreadId, (TurnId, Option<Interrupt>)>> {
        self.set_aside.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Asks the turn set aside in `thread` to end as `asked` says - what was asked first wins -,
    /// and tells the waiting run loop; whether a turn was set aside there.
    fn ask_set_aside(&self, thread: ThreadId, asked: Interrupt) -> bool {
        let mut set_aside = self.lock_set_aside();
        let Some((_, already)) = set_aside.get_mut(&thread) else { return false };
        already.get_or_insert(asked);
        drop(set_aside);
        self.asked.notify_one();
        true
    }

    /// The turn under way, as `turn_under_way` says, with the thread it runs in (issue 602).
    pub(crate) fn under_way(&self) -> Option<(ThreadId, TurnId)> {
        let (turn, _) = *self.lock_turn();
        turn.filter(|_| self.interruptible())
    }

    /// The thread of the turn that runs or ran last (issue 602): whose part of the agent's state
    /// its steps work with. `None` before the first turn, and once the last was set aside.
    pub(crate) fn thread(&self) -> Option<ThreadId> {
        self.lock_turn().0.map(|(thread, _)| thread)
    }

    /// Whether the run loop is in a turn that an interrupt can still end.
    fn interruptible(&self) -> bool {
        self.state().interruptible()
    }

    /// The token of the current turn, cancelled by `cancel`, `stop` and
    /// `leave`.
    pub(crate) fn token(&self) -> CancellationToken {
        self.lock_turn().1.clone()
    }

    fn lock_turn(&self) -> MutexGuard<'_, (Option<(ThreadId, TurnId)>, CancellationToken)> {
        self.turn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Asks for the turns under way in `thread` to end and be taken over - in every thread if
    /// `None` (issue 602): the one that runs there, and one set aside there. Outside a turn there
    /// is nothing to cancel, and a turn already ending is ended anyway.
    pub(crate) fn cancel(&self, thread: Option<ThreadId>) {
        let there = thread.is_none_or(|thread| self.thread() == Some(thread));
        if there && self.interruptible() {
            self.interrupt(Interrupt::Cancel);
        }
        let set_aside = match thread {
            Some(thread) => vec![thread],
            None => self.set_aside_threads().into_iter().collect(),
        };
        for thread in set_aside {
            self.ask_set_aside(thread, Interrupt::Cancel);
        }
    }

    /// Asks for the turn `turn` to be given up, because its answer is asked
    /// for again (issue 235), and returns whether `turn` is the turn under
    /// way - decided under the turn's lock, so that no turn begins in
    /// between. Only that turn is interrupted: a request about another, or
    /// about one that ended, ends nothing. A cancel asked first wins - the
    /// turn is taken over - but the turn was the one under way all the same.
    pub(crate) fn retry(&self, turn: TurnId) -> bool {
        let under_way = self.lock_turn();
        if under_way.0.map(|(_, under_way)| under_way) != Some(turn) || !self.interruptible() {
            // Or a turn set aside, which waits for a person (issue 602).
            let set_aside = self.lock_set_aside();
            let thread = set_aside.iter().find(|(_, (aside, _))| *aside == turn).map(|(t, _)| *t);
            drop(set_aside);
            return thread.is_some_and(|thread| self.ask_set_aside(thread, Interrupt::Retry));
        }
        let _ = self.interrupt.compare_exchange(
            NO_INTERRUPT,
            Interrupt::Retry as u8,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        under_way.1.cancel();
        true
    }

    /// Asks the run loop to end because the channel stops (issue 377): the
    /// step under way is interrupted - a model call in flight, a tool - and
    /// the turn kept where it stands, to be resumed.
    pub(crate) fn stop(&self) {
        self.interrupt(Interrupt::Stop);
        self.ending.cancel();
    }

    /// Asks the run loop to end because the actor left the channel: the
    /// step under way is interrupted, and the turn given up instead of
    /// taken over.
    pub(crate) fn leave(&self) {
        self.left.store(true, Ordering::SeqCst);
        self.interrupt(Interrupt::Leave);
        self.ending.cancel();
    }

    /// Returns once `stop` or `leave` asked the run loop to end - at once if
    /// one did.
    pub(crate) async fn ending(&self) {
        self.ending.cancelled().await;
    }

    /// Whether `stop` or `leave` asked the run loop to end.
    pub(crate) fn is_ending(&self) -> bool {
        self.ending.is_cancelled()
    }

    /// Whether `leave` asked the run loop to end: the actor left the
    /// channel.
    pub(crate) fn has_left(&self) -> bool {
        self.left.load(Ordering::SeqCst)
    }

    /// Leaves `interrupt` for the run loop - leaving is not turned back into
    /// a stop, nor either into a cancel - and interrupts the step under way.
    fn interrupt(&self, interrupt: Interrupt) {
        match interrupt {
            Interrupt::Cancel | Interrupt::Retry => {
                let _ = self.interrupt.compare_exchange(
                    NO_INTERRUPT,
                    interrupt as u8,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
            }
            Interrupt::Stop => {
                let _ = self.interrupt.try_update(Ordering::SeqCst, Ordering::SeqCst, |asked| {
                    (asked != Interrupt::Leave as u8).then_some(Interrupt::Stop as u8)
                });
            }
            Interrupt::Leave => self.interrupt.store(Interrupt::Leave as u8, Ordering::SeqCst),
        }
        self.token().cancel();
    }

    /// The calls `ids` of a tool step of the turn under way begin (issue
    /// 276): from now on each can be interrupted on its own. Returns, by
    /// id, what tells the run loop who interrupted a call - for
    /// `interruption`, or asked before the call runs.
    pub(crate) fn begin_calls(
        &self,
        ids: impl IntoIterator<Item = String>,
    ) -> HashMap<String, oneshot::Receiver<String>> {
        let (senders, receivers) = ids
            .into_iter()
            .map(|id| {
                let (sender, receiver) = oneshot::channel();
                ((id.clone(), sender), (id, receiver))
            })
            .unzip();
        *self.lock_calls() = senders;
        receivers
    }

    /// Interrupts the call `call` of the turn `turn` alone (issue 276), on
    /// behalf of `by`, and returns whether it ran or waited to run in the
    /// turn under way - decided under the turn's lock, as `retry` decides,
    /// so that no turn begins in between. The turn goes on.
    pub(crate) fn interrupt_call(&self, turn: TurnId, call: &str, by: String) -> bool {
        let under_way = self.lock_turn();
        if under_way.0.map(|(_, under_way)| under_way) != Some(turn) || !self.interruptible() {
            return false;
        }
        let Some(interrupt) = self.lock_calls().remove(call) else { return false };
        interrupt.send(by).is_ok()
    }

    fn lock_calls(&self) -> MutexGuard<'_, HashMap<String, oneshot::Sender<String>>> {
        self.calls.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// What was asked of the run loop since it last looked, if anything.
    pub(crate) fn take_interrupt(&self) -> Option<Interrupt> {
        match self.interrupt.swap(NO_INTERRUPT, Ordering::SeqCst) {
            v if v == Interrupt::Cancel as u8 => Some(Interrupt::Cancel),
            v if v == Interrupt::Stop as u8 => Some(Interrupt::Stop),
            v if v == Interrupt::Retry as u8 => Some(Interrupt::Retry),
            v if v == Interrupt::Leave as u8 => Some(Interrupt::Leave),
            _ => None,
        }
    }

    /// What interrupts a call of the turn whose token is `turn`: the turn
    /// ending, or the channel stopping (issue 377) - told apart by what was
    /// asked, which the run loop takes only after the step -, or - through
    /// `by_user`, for a call that can be interrupted on its own
    /// (`begin_calls`) - a user interrupting this call alone (issue 276).
    /// The turn's end wins.
    pub(crate) async fn interruption(
        &self,
        turn: CancellationToken,
        by_user: Option<oneshot::Receiver<String>>,
    ) -> Interruption {
        let by_user = async {
            match by_user {
                Some(by_user) => match by_user.await {
                    Ok(by) => by,
                    // Nobody can interrupt it any more: the turn's end can.
                    Err(_) => pending().await,
                },
                None => pending().await,
            }
        };
        tokio::select! {
            biased;
            () = turn.cancelled() => match self.interrupt.load(Ordering::SeqCst) == Interrupt::Stop as u8 {
                true => Interruption::Stopped,
                false => Interruption::TurnEnded,
            },
            by = by_user => Interruption::ByUser(by),
        }
    }
}

/// Whether `asked`, an `Interrupt` or `NO_INTERRUPT`, ends the run loop:
/// a stop or leaving, which a new turn does not drop.
fn ends_run(asked: u8) -> bool {
    asked == Interrupt::Stop as u8 || asked == Interrupt::Leave as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::actor::EventContext;
    use caelumora_domain::confidentiality::Classification;
    use caelumora_domain::value::time_zone::TimeZone;
    use caelumora_domain::work::Work;
    use std::time::Duration;
    use tokio::time;

    /// Issue 602: an agent resumes the turn stored under way in whichever thread it runs, at the
    /// step it stands at; without one, asleep or idle as it stood outside a turn, its own as a
    /// whole and of no thread.
    #[test]
    fn it_resumes_the_turn_under_way_in_whichever_thread_or_as_it_stood() {
        let there = ThreadId::new();
        let mut state = AgentChannelState { asleep: true, ..AgentChannelState::default() };
        let control = RunControl::resuming(&state);
        assert_eq!(control.state(), ActorState::Sleeping, "asleep, as it stood");
        assert_eq!(control.thread(), None, "no turn");
        state.asleep = false;
        assert_eq!(RunControl::resuming(&state).state(), ActorState::Idle, "or awake");

        let work =
            || Work::new(EventContext::default(), None, Classification::PUBLIC, TimeZone::UTC);
        let waiting = state.begin_turn(there, work());
        state.turn_mut(there).expect("its turn").step = ActorState::WaitingForUser;
        let control = RunControl::resuming(&state);
        assert_eq!(control.thread(), None, "a turn waiting for a person runs not");
        assert_eq!(control.turn_under_way(there), Some(waiting), "it is set aside");
        assert_eq!(control.state(), ActorState::Idle, "the agent waits between turns");
        assert_eq!(control.standing(), ActorState::WaitingForUser, "and tells it waits");

        let line = ThreadId::default();
        let going_on = state.begin_turn(line, work());
        state.turn_mut(line).expect("its turn").step = ActorState::Planning;
        let control = RunControl::resuming(&state);
        assert_eq!(control.thread(), Some(line), "the turn that goes on runs");
        assert_eq!(control.turn_under_way(line), Some(going_on));
        assert_eq!(control.state(), ActorState::Planning, "at the step it stands at");
        assert_eq!(control.set_aside_threads(), BTreeSet::from([there]), "the other set aside");
    }

    #[test]
    fn a_cancel_interrupts_only_a_turn_under_way() {
        for state in
            [ActorState::Idle, ActorState::Sleeping, ActorState::Reflecting, ActorState::Cancelling]
        {
            let control = RunControl::new(state, None);
            control.cancel(None);
            assert_eq!(control.take_interrupt(), None, "{state:?}");
            assert!(!control.token().is_cancelled(), "{state:?}");
        }
        for state in [ActorState::Planning, ActorState::ExecutingTools, ActorState::WaitingForUser]
        {
            let control = RunControl::new(state, None);
            control.cancel(None);
            assert!(control.token().is_cancelled(), "{state:?}");
            assert_eq!(control.take_interrupt(), Some(Interrupt::Cancel), "{state:?}");
            assert_eq!(control.take_interrupt(), None, "taken once: {state:?}");
        }
    }

    /// Issue 602: a cancel of a thread interrupts the turn under way only if it runs there.
    #[test]
    fn a_cancel_of_a_thread_interrupts_only_a_turn_running_there() {
        let (here, there) = (ThreadId::default(), ThreadId::new());
        let control = RunControl::new(ActorState::Planning, Some((here, TurnId::new())));
        control.cancel(Some(there));
        assert_eq!(control.take_interrupt(), None, "the turn runs in another thread");
        control.cancel(Some(here));
        assert_eq!(control.take_interrupt(), Some(Interrupt::Cancel), "it runs here");
    }

    #[test]
    fn a_stop_wins_over_a_cancel_whichever_comes_first_and_leaving_over_both() {
        let control = RunControl::new(ActorState::Planning, None);
        control.cancel(None);
        control.stop();
        assert_eq!(control.take_interrupt(), Some(Interrupt::Stop));

        let control = RunControl::new(ActorState::Planning, None);
        control.stop();
        control.cancel(None);
        assert_eq!(control.take_interrupt(), Some(Interrupt::Stop));

        let control = RunControl::new(ActorState::Planning, None);
        control.leave();
        control.stop();
        control.cancel(None);
        assert_eq!(control.take_interrupt(), Some(Interrupt::Leave));

        let control = RunControl::new(ActorState::Planning, None);
        control.stop();
        control.leave();
        assert_eq!(control.take_interrupt(), Some(Interrupt::Leave));
    }

    #[test]
    fn a_new_turn_drops_a_leftover_cancel_but_not_a_stop_nor_leaving() {
        let control = RunControl::new(ActorState::Planning, None);
        control.cancel(None);
        control.begin_turn(ThreadId::default(), TurnId::new());
        assert!(!control.token().is_cancelled(), "the new turn has a token of its own");
        assert_eq!(control.take_interrupt(), None);

        control.stop();
        control.begin_turn(ThreadId::default(), TurnId::new());
        assert_eq!(control.take_interrupt(), Some(Interrupt::Stop));
        assert!(control.token().is_cancelled(), "a turn begun after a stop has nothing to run for");
        assert!(control.is_ending() && !control.has_left(), "stopped, it did not leave");

        let control = RunControl::new(ActorState::Planning, None);
        control.leave();
        control.begin_turn(ThreadId::default(), TurnId::new());
        assert_eq!(control.take_interrupt(), Some(Interrupt::Leave));
        assert!(control.token().is_cancelled());
        assert!(control.is_ending());
        assert!(control.has_left(), "for good: not taken with the interrupt");
    }

    #[tokio::test]
    async fn the_end_is_waited_for_also_once_it_came() {
        for end in [RunControl::stop, RunControl::leave] {
            let control = RunControl::new(ActorState::Idle, None);
            assert!(!control.is_ending());
            end(&control);
            control.ending().await;
            assert!(control.is_ending());
        }
    }

    #[test]
    fn the_turn_under_way_is_known_while_it_runs() {
        assert_eq!(RunControl::new(ActorState::Idle, None).thread(), None, "before the first turn");
        let (resumed, there) = (TurnId::new(), ThreadId::new());
        let control = RunControl::new(ActorState::ExecutingTools, Some((there, resumed)));
        assert_eq!(
            control.turn_under_way(there),
            Some(resumed),
            "a resumed turn is the one it was"
        );
        assert_eq!(control.thread(), Some(there), "in the thread it was");

        let (next, elsewhere) = (TurnId::new(), ThreadId::new());
        control.go_to(ActorState::Idle);
        assert_eq!(control.turn_under_way(there), None, "between turns");
        control.begin_turn(elsewhere, next);
        control.go_to(ActorState::Planning);
        assert_eq!(control.turn_under_way(elsewhere), Some(next));
        assert_eq!(control.thread(), Some(elsewhere), "the next turn's thread");
        control.go_to(ActorState::Reflecting);
        assert_eq!(control.turn_under_way(elsewhere), None, "nor once it ends");
        assert_eq!(control.thread(), Some(elsewhere), "its steps still work with its thread");
    }

    /// Issue 602: a turn that waits for a person is set aside, and no turn runs; a cancel of its
    /// thread or a retry of it asks it to end without touching the turn that runs, and it runs
    /// again with what was asked of it - what was asked first.
    #[tokio::test]
    async fn a_turn_set_aside_is_asked_to_end_and_resumed_with_what_was_asked() {
        let (line, there) = (ThreadId::default(), ThreadId::new());
        let (waiting, running) = (TurnId::new(), TurnId::new());
        let control = RunControl::new(ActorState::WaitingForUser, Some((there, waiting)));
        control.set_aside();
        control.go_to(ActorState::Idle);
        assert_eq!(control.thread(), None, "no turn runs");
        assert_eq!(control.turn_under_way(there), Some(waiting), "it is set aside there");
        assert_eq!(control.standing(), ActorState::WaitingForUser, "and the agent tells it waits");

        control.begin_turn(line, running);
        control.go_to(ActorState::Planning);
        control.cancel(Some(line));
        assert_eq!(control.take_interrupt(), Some(Interrupt::Cancel), "the turn that runs");
        assert_eq!(control.asked_of_set_aside(), None, "not the one set aside elsewhere");
        assert!(control.retry(waiting), "a retry of the one set aside");
        control.cancel(Some(there));
        control.asked().await;
        assert_eq!(control.asked_of_set_aside(), Some(there), "it is asked to end");
        assert_eq!(control.take_interrupt(), None, "the turn that runs is left as it is");

        control.go_to(ActorState::Idle);
        let asked = control.resume(there, ActorState::WaitingForUser);
        assert_eq!(asked, Some(Some(Interrupt::Retry)), "what was asked first");
        assert_eq!(control.thread(), Some(there), "it runs again");
        assert_eq!(control.state(), ActorState::WaitingForUser, "at the step it stands at");
        assert!(control.set_aside_threads().is_empty(), "and is set aside no more");

        // A cancel asked as its step ended, before the run loop took it, goes with it as it is set
        // aside.
        control.cancel(Some(there));
        control.set_aside();
        control.asked().await;
        assert_eq!(control.asked_of_set_aside(), Some(there), "not lost");
        assert_eq!(control.take_interrupt(), None, "nor left for another turn");
    }

    #[test]
    fn a_retry_interrupts_only_the_turn_it_names_while_it_can_still_be_ended() {
        let (turn, other) = (TurnId::new(), TurnId::new());
        let control = RunControl::new(ActorState::Planning, Some((ThreadId::default(), turn)));
        assert!(!control.retry(other), "another turn");
        assert!(
            !control.token().is_cancelled() && control.take_interrupt().is_none(),
            "ends nothing"
        );
        assert!(control.retry(turn));
        assert!(control.token().is_cancelled());
        assert_eq!(control.take_interrupt(), Some(Interrupt::Retry));

        for state in [ActorState::Idle, ActorState::Reflecting, ActorState::Cancelling] {
            let control = RunControl::new(state, Some((ThreadId::default(), turn)));
            assert!(!control.retry(turn), "not under way any more: {state:?}");
            assert_eq!(control.take_interrupt(), None, "{state:?}");
        }
    }

    #[test]
    fn of_a_cancel_and_a_retry_the_first_wins_and_a_stop_over_both() {
        let turn = TurnId::new();
        let control = RunControl::new(ActorState::Planning, Some((ThreadId::default(), turn)));
        control.cancel(None);
        assert!(control.retry(turn), "the turn was the one under way");
        assert_eq!(control.take_interrupt(), Some(Interrupt::Cancel), "but the cancel came first");

        let control = RunControl::new(ActorState::Planning, Some((ThreadId::default(), turn)));
        assert!(control.retry(turn));
        control.cancel(None);
        assert_eq!(control.take_interrupt(), Some(Interrupt::Retry));

        let control = RunControl::new(ActorState::Planning, Some((ThreadId::default(), turn)));
        assert!(control.retry(turn));
        control.stop();
        assert_eq!(control.take_interrupt(), Some(Interrupt::Stop));

        // One that came as the turn ended has nothing to end in the next.
        let control = RunControl::new(ActorState::Planning, Some((ThreadId::default(), turn)));
        assert!(control.retry(turn));
        control.begin_turn(ThreadId::default(), TurnId::new());
        assert_eq!(control.take_interrupt(), None);
    }

    #[tokio::test]
    async fn a_call_is_interrupted_alone_only_in_the_turn_it_runs_in_and_only_once() {
        let (turn, other) = (TurnId::new(), TurnId::new());
        let control =
            RunControl::new(ActorState::ExecutingTools, Some((ThreadId::default(), turn)));
        let mut calls = control.begin_calls(["a".to_string(), "b".to_string()]);
        assert!(!control.interrupt_call(other, "a", "alice".to_string()), "not of another turn");
        assert!(!control.interrupt_call(turn, "c", "alice".to_string()), "not a call of the step");

        assert!(control.interrupt_call(turn, "a", "alice".to_string()));
        assert!(!control.interrupt_call(turn, "a", "bob".to_string()), "once");
        let interrupted = control.interruption(control.token(), calls.remove("a")).await;
        assert_eq!(interrupted, Interruption::ByUser("alice".to_string()));
        assert!(
            !control.token().is_cancelled() && control.take_interrupt().is_none(),
            "the turn goes on"
        );

        // A call that ran can no longer be interrupted.
        drop(calls.remove("b"));
        assert!(!control.interrupt_call(turn, "b", "alice".to_string()));
    }

    #[tokio::test]
    async fn the_turn_s_end_interrupts_every_call_and_wins() {
        let turn = TurnId::new();
        let control =
            RunControl::new(ActorState::ExecutingTools, Some((ThreadId::default(), turn)));
        let mut calls = control.begin_calls(["a".to_string()]);
        assert!(control.interrupt_call(turn, "a", "alice".to_string()));
        control.cancel(None);
        assert_eq!(
            control.interruption(control.token(), calls.remove("a")).await,
            Interruption::TurnEnded
        );
        assert_eq!(
            control.interruption(control.token(), None).await,
            Interruption::TurnEnded,
            "also a call without an id"
        );

        let control = RunControl::new(ActorState::Cancelling, Some((ThreadId::default(), turn)));
        let _calls = control.begin_calls(["a".to_string()]);
        assert!(
            !control.interrupt_call(turn, "a", "alice".to_string()),
            "not in a turn that is ending"
        );
    }

    /// A call broken off because the channel stops is told so (issue 377):
    /// its turn is kept, unlike one that is cancelled or given up.
    #[tokio::test]
    async fn a_stop_interrupts_every_call_as_stopped_and_leaving_as_ended() {
        let turn = TurnId::new();
        let control =
            RunControl::new(ActorState::ExecutingTools, Some((ThreadId::default(), turn)));
        let mut calls = control.begin_calls(["a".to_string()]);
        control.stop();
        assert_eq!(
            control.interruption(control.token(), calls.remove("a")).await,
            Interruption::Stopped
        );

        let control =
            RunControl::new(ActorState::ExecutingTools, Some((ThreadId::default(), turn)));
        control.stop();
        control.leave();
        assert_eq!(control.interruption(control.token(), None).await, Interruption::TurnEnded);
    }

    #[test]
    fn only_a_sleeping_agent_is_woken() {
        let control = RunControl::new(ActorState::Sleeping, None);
        control.wakeup();
        assert_eq!(control.state(), ActorState::Idle);

        let control = RunControl::new(ActorState::Planning, None);
        control.wakeup();
        assert_eq!(control.state(), ActorState::Planning);
    }

    /// The run loop is told of a wakeup only if it woke the agent (issue
    /// 243), and finds the state unannounced.
    #[tokio::test]
    async fn a_wakeup_tells_the_run_loop_and_is_announced_once() {
        let control = RunControl::new(ActorState::Sleeping, None);
        assert_eq!(control.unannounced(), None, "it starts at what it stood at");
        control.wakeup();
        time::timeout(Duration::from_secs(1), control.woken())
            .await
            .expect("told, though nobody waited yet");
        assert_eq!(control.unannounced(), Some(ActorState::Idle));
        assert_eq!(control.unannounced(), None, "announced once");

        control.wakeup();
        assert!(
            time::timeout(Duration::from_millis(50), control.woken()).await.is_err(),
            "awake already: nothing to tell"
        );
        control.go_to(ActorState::Planning);
        control.go_to(ActorState::Idle);
        assert_eq!(control.unannounced(), None, "back where it was announced");
    }
}
