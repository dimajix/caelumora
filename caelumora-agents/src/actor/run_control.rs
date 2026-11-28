//! The run control of an agent's actor - an `AgentActor` or an `ExternalAgentActor` (issue 363):
//! what its run loop shares with `cancel`, `retry`, `stop`, `leave` and `wakeup` (see
//! `RunControl`). Held by its body (`AgentBody`).

use caelumora_domain::actor::{ActorState, ProcessingStateHolder};
use caelumora_domain::agent::AgentChannelState;
use caelumora_domain::channel::ThreadId;
use caelumora_domain::work::TurnId;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard};
use tokio::sync::Notify;
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

/// Why a turn under way does not run (issues 602, 591).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Aside {
    /// It waits for a person: so does the agent (`standing`), and it runs again once settled
    /// (`resume`).
    WaitingForAPerson,
    /// Its work goes on elsewhere - an external agent's open task -, and ends there: the run loop
    /// keeps it in step with that work (`keep_going_on`).
    GoingOn,
}

/// A turn set aside: in which thread, why, and what `cancel` or `retry` asked of it meanwhile
/// (`Interrupt::Cancel`, `Interrupt::Retry`), for the run loop to take up.
#[derive(Debug, Clone, Copy)]
struct SetAside {
    thread: ThreadId,
    why: Aside,
    asked: Option<Interrupt>,
}

/// `RunControl::interrupt` when nothing was asked.
const NO_INTERRUPT: u8 = 0;

/// Where the run loop of an agent stands, and what `cancel`, `stop`, `leave` and `wakeup` asked of
/// it. These come from other tasks and are synchronous: they only leave word here and interrupt the
/// step under way; the run loop takes the word after every step and alone decides what follows
/// (issue 152). The same for both kinds of agent (issue 363): an external agent's answer is a turn
/// here, with a token of its own, and it never sleeps. The calls a user interrupts on their own are
/// the agent's (`CallInterrupts`).
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
    /// The turns under way that do not run (issues 602, 591), by their ids: those that wait for a
    /// person - one a thread -, and those whose work goes on elsewhere. The turn that runs is
    /// never among them. Changed under the turn's lock alone, so that a cancel or a retry finds a
    /// turn either running or set aside.
    set_aside: Mutex<BTreeMap<TurnId, SetAside>>,
    /// Tells the run loop, waiting between turns, that `cancel` or `retry` asked a turn set aside
    /// to end.
    asked: Notify,
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
        }
    }

    /// The run control of an agent whose state in its channel is `state`, as it resumes: in the
    /// turn stored under way that goes on by itself (`AgentThreadState::has_work_under_way`), in
    /// whichever thread - one at a time (issue 557) -, at the step it stands at
    /// (`TurnState::step`), or else asleep or idle as it stood outside a turn, its own as a whole
    /// (`AgentChannelState::asleep`); every other turn stored - one waiting for a person - set
    /// aside (issue 602), and so the turn of every open task of an external agent that works
    /// (issue 591).
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
        let waiting = waiting.filter_map(|(thread, part)| {
            let turn = part.turn.as_ref()?.work.turn_id;
            Some((turn, SetAside { thread: *thread, why: Aside::WaitingForAPerson, asked: None }))
        });
        let going_on = state
            .remote
            .working_turns()
            .map(|(thread, turn)| (turn, SetAside { thread, why: Aside::GoingOn, asked: None }));
        *control.lock_set_aside() = waiting.chain(going_on).collect();
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
            ActorState::Idle if !self.waiting_threads().is_empty() => ActorState::WaitingForUser,
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
    /// would find over - else the one set aside there waiting for a person.
    pub(crate) fn turn_under_way(&self, thread: ThreadId) -> Option<TurnId> {
        match self.under_way() {
            Some((there, turn)) if there == thread => Some(turn),
            _ => self.waiting_in(thread),
        }
    }

    /// The running turn does not run on, for `why` (issues 602, 591): it is set aside, and no turn
    /// runs; the run loop goes on between turns - and resumes a turn waiting for a person once it
    /// goes on by itself (`resume`). A cancel or a retry asked of it since its step ended, which
    /// the run loop has not taken, goes with it, and the run loop is told (`asked`). Nothing to set
    /// aside if no turn runs.
    pub(crate) fn set_aside(&self, why: Aside) {
        let mut running = self.lock_turn();
        if let Some((thread, turn)) = running.0.take() {
            let asked = self.take_turn_interrupt();
            self.lock_set_aside().insert(turn, SetAside { thread, why, asked });
            if asked.is_some() {
                self.asked.notify_one();
            }
        }
        running.1 = self.ending.child_token();
    }

    /// The turn set aside in `thread` waiting for a person runs again at `step` (issue 602), with a
    /// token of its own, as a turn begins - one step under the turn's lock, so that a cancel or a
    /// retry finds it either set aside or running -; what `cancel` or `retry` asked of it
    /// meanwhile, for the run loop to take up. `None` if no turn waits there.
    pub(crate) fn resume(&self, thread: ThreadId, step: ActorState) -> Option<Option<Interrupt>> {
        let mut running = self.lock_turn();
        let turn = self.waiting_in(thread)?;
        let SetAside { asked, .. } = self.lock_set_aside().remove(&turn)?;
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

    /// The threads of the turns set aside waiting for a person (issue 602).
    pub(crate) fn waiting_threads(&self) -> BTreeSet<ThreadId> {
        let set_aside = self.lock_set_aside();
        let waiting = set_aside.values().filter(|aside| aside.why == Aside::WaitingForAPerson);
        waiting.map(|aside| aside.thread).collect()
    }

    /// The turn set aside in `thread` waiting for a person, if one is.
    fn waiting_in(&self, thread: ThreadId) -> Option<TurnId> {
        let set_aside = self.lock_set_aside();
        let mut waiting =
            set_aside.iter().filter(|(_, aside)| aside.why == Aside::WaitingForAPerson);
        waiting.find(|(_, aside)| aside.thread == thread).map(|(turn, _)| *turn)
    }

    /// A turn set aside for `why` that `cancel` or `retry` asked to end (issues 602, 591), with its
    /// thread, if any: for the run loop to end it - to resume and end a turn waiting for a person,
    /// to end the work of one that goes on.
    pub(crate) fn asked_aside(&self, why: Aside) -> Option<(ThreadId, TurnId)> {
        let set_aside = self.lock_set_aside();
        let mut of_kind = set_aside.iter().filter(|(_, aside)| aside.why == why);
        of_kind.find_map(|(turn, aside)| aside.asked.map(|_| (aside.thread, *turn)))
    }

    /// Keeps the turns set aside whose work goes on in step with that work (issue 591): `turns`,
    /// each with its thread, are set aside so - those that were keep what was asked of them -, and
    /// every other one goes, its work ended, however it ended. The turn that runs is never set
    /// aside so.
    pub(crate) fn keep_going_on(&self, turns: &[(ThreadId, TurnId)]) {
        let running = self.lock_turn();
        let mut set_aside = self.lock_set_aside();
        let going_on = |turn: &TurnId| turns.iter().any(|(_, going_on)| going_on == turn);
        set_aside.retain(|turn, aside| aside.why != Aside::GoingOn || going_on(turn));
        let runs = running.0.map(|(_, turn)| turn);
        for (thread, turn) in turns.iter().filter(|(_, turn)| Some(*turn) != runs) {
            let fresh = SetAside { thread: *thread, why: Aside::GoingOn, asked: None };
            set_aside.entry(*turn).or_insert(fresh);
        }
    }

    /// Returns once `cancel` or `retry` asked a turn set aside to end - at once if one did since
    /// this was last waited for.
    pub(crate) async fn asked(&self) {
        self.asked.notified().await;
    }

    fn lock_set_aside(&self) -> MutexGuard<'_, BTreeMap<TurnId, SetAside>> {
        self.set_aside.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Asks the turns set aside that `which` picks, by turn and how it was set aside, to end as
    /// `asked` says - what was asked first wins -, and tells the waiting run loop; whether one was
    /// asked.
    fn ask_set_aside(&self, which: impl Fn(TurnId, &SetAside) -> bool, asked: Interrupt) -> bool {
        let mut set_aside = self.lock_set_aside();
        let mut any = false;
        for (_, aside) in set_aside.iter_mut().filter(|(turn, aside)| which(**turn, aside)) {
            aside.asked.get_or_insert(asked);
            any = true;
        }
        drop(set_aside);
        if any {
            self.asked.notify_one();
        }
        any
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

    /// The thread of the turn the run loop works on now (issue 595): where the agent answers
    /// (`ActorState::answers`). `None` between turns, and while a turn of it waits for a person.
    pub(crate) fn answering_in(&self) -> Option<ThreadId> {
        self.thread().filter(|_| self.state().answers())
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

    /// Asks for the turns under way in `thread` to end - in every thread if `None` (issue 602): the
    /// one that runs there, taken over, and those set aside there, of either kind: one waiting for
    /// a person taken over once the run loop resumed it, one whose work goes on ended where that
    /// work goes on (issue 591) -, and returns whether there was one (issue 611) - decided under
    /// the turn's lock, as `retry` decides, so that a turn set aside or resumed meanwhile is found
    /// either running or set aside. Outside a turn there is nothing to cancel, and a running turn
    /// already ending is ended anyway: it is none. A turn set aside counts until the run loop has
    /// taken it up.
    pub(crate) fn cancel(&self, thread: Option<ThreadId>) -> bool {
        let under_way = self.lock_turn();
        let there = thread.is_none_or(|thread| under_way.0.is_some_and(|(t, _)| t == thread));
        let running = there && self.interruptible();
        if running {
            self.ask(Interrupt::Cancel);
            under_way.1.cancel();
        }
        let there = |_, aside: &SetAside| thread.is_none_or(|thread| aside.thread == thread);
        self.ask_set_aside(there, Interrupt::Cancel) || running
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
            // Or a turn set aside waiting for a person (issue 602): the work of one that goes on
            // elsewhere is not done again.
            let of_turn =
                |aside, set: &SetAside| aside == turn && set.why == Aside::WaitingForAPerson;
            return self.ask_set_aside(of_turn, Interrupt::Retry);
        }
        self.ask(Interrupt::Retry);
        under_way.1.cancel();
        true
    }

    /// Asks the run loop to end because the channel stops (issue 377): the step under way is
    /// interrupted - a model call in flight, a tool, its turn's token being a child of `ending` -
    /// and the turn kept where it stands, to be resumed. Whoever sees the turn's token cancelled so
    /// finds the run ending (`is_ending`): tokio-util cancels a token's children while it holds the
    /// token's lock, its own flag set, which is what tells a stop from a cancel
    /// (`call_interrupts::interruption`, the external agent's exchange).
    pub(crate) fn stop(&self) {
        self.ask(Interrupt::Stop);
        self.ending.cancel();
    }

    /// Asks the run loop to end because the actor left the channel: the step under way is
    /// interrupted, as by `stop`, and the turn given up instead of taken over.
    pub(crate) fn leave(&self) {
        self.left.store(true, Ordering::SeqCst);
        self.ask(Interrupt::Leave);
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

    /// Leaves `interrupt` for the run loop - leaving is not turned back into a stop, nor either
    /// into a cancel, and a cancel or a retry asked first wins over the other -, interrupting
    /// nothing: whoever asks interrupts the step itself - a cancel or a retry the turn's token,
    /// under the turn's lock, a stop or leaving `ending`.
    fn ask(&self, interrupt: Interrupt) {
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
        assert_eq!(control.waiting_threads(), BTreeSet::from([there]), "the other set aside");
    }

    /// Issue 591: one cancel asks every turn set aside it reaches to end - in every thread - and
    /// each runs again with what was asked of it.
    #[test]
    fn a_cancel_asks_every_turn_set_aside_it_reaches() {
        let mut state = AgentChannelState::default();
        let work =
            || Work::new(EventContext::default(), None, Classification::PUBLIC, TimeZone::UTC);
        let threads = [ThreadId::new(), ThreadId::new()];
        for thread in threads {
            state.begin_turn(thread, work());
            state.turn_mut(thread).expect("its turn").step = ActorState::WaitingForUser;
        }
        let control = RunControl::resuming(&state);
        assert!(control.cancel(None), "there were turns to end");
        let asked = control.asked_aside(Aside::WaitingForAPerson);
        assert!(asked.is_some_and(|(asked, _)| threads.contains(&asked)));
        for thread in threads {
            let asked = control.resume(thread, ActorState::WaitingForUser);
            assert_eq!(asked, Some(Some(Interrupt::Cancel)), "each asked");
            control.set_aside(Aside::WaitingForAPerson);
        }
    }

    /// Issue 591: the turn of an external agent's open task that works is set aside as going on as
    /// it resumes: not waiting for a person - the agent tells no wait, and takes requests in its
    /// thread -, reached by a cancel but not by a retry, and kept in step with its work.
    #[test]
    fn a_turn_whose_work_goes_on_is_set_aside_reached_by_a_cancel_and_kept_in_step() {
        use caelumora_domain::external_agent::TaskId;

        let mut state = AgentChannelState::default();
        let work = Work::new(EventContext::default(), None, Classification::PUBLIC, TimeZone::UTC);
        let (turn, thread) = (work.turn_id, work.event_context.thread_id);
        state.remote.begin(&TaskId("t-1".to_string()), &work);
        let control = RunControl::resuming(&state);
        assert_eq!(control.thread(), None, "it runs not");
        assert!(control.waiting_threads().is_empty(), "nor waits for a person");
        assert_eq!(control.standing(), ActorState::Idle, "and the agent tells no wait");
        assert_eq!(control.turn_under_way(thread), None, "nor is it a turn to answer again");
        assert!(!control.retry(turn), "which a retry does not reach");
        assert!(control.cancel(Some(thread)), "a cancel does");
        assert_eq!(
            control.asked_aside(Aside::GoingOn),
            Some((thread, turn)),
            "for the run loop to end it"
        );

        control.keep_going_on(&[(thread, turn)]);
        assert_eq!(
            control.asked_aside(Aside::GoingOn),
            Some((thread, turn)),
            "kept while its work goes on"
        );
        control.keep_going_on(&[]);
        assert_eq!(control.asked_aside(Aside::GoingOn), None, "gone once it ended");
        assert!(!control.cancel(None), "and nothing to end any more");
    }

    /// Issue 591: a cancel reaches the turns whose work goes on as it finds them, not one set aside
    /// so after it; and keeping them in step sets aside one whose work goes on that was not, but
    /// never the turn that runs.
    #[test]
    fn a_cancel_reaches_no_turn_set_aside_after_it() {
        use caelumora_domain::external_agent::TaskId;

        let mut state = AgentChannelState::default();
        let work = Work::new(EventContext::default(), None, Classification::PUBLIC, TimeZone::UTC);
        let (first, thread) = (work.turn_id, work.event_context.thread_id);
        state.remote.begin(&TaskId("t-1".to_string()), &work);
        let control = RunControl::resuming(&state);
        assert!(control.cancel(None), "the first is reached");
        let second = TurnId::new();
        control.begin_turn(thread, second);
        control.go_to(ActorState::Planning);
        control.keep_going_on(&[(thread, first), (thread, second)]);
        assert!(control.waiting_threads().is_empty(), "nothing waits");
        control.set_aside(Aside::GoingOn);
        control.go_to(ActorState::Idle);
        assert_eq!(control.asked_aside(Aside::GoingOn), Some((thread, first)), "the first asked");
        control.keep_going_on(&[(thread, second)]);
        assert_eq!(control.asked_aside(Aside::GoingOn), None, "the second not");
        assert!(control.cancel(Some(thread)), "until the next cancel");

        let third = TurnId::new();
        control.keep_going_on(&[(thread, second), (thread, third)]);
        assert!(control.cancel(None));
        control.keep_going_on(&[(thread, third)]);
        assert_eq!(control.asked_aside(Aside::GoingOn), Some((thread, third)), "kept in step");
        assert_eq!(control.asked_aside(Aside::WaitingForAPerson), None, "of its kind alone");
    }

    #[test]
    fn a_cancel_interrupts_only_a_turn_under_way() {
        for state in
            [ActorState::Idle, ActorState::Sleeping, ActorState::Reflecting, ActorState::Cancelling]
        {
            let control = RunControl::new(state, None);
            assert!(!control.cancel(None), "no turn to end: {state:?}");
            assert_eq!(control.take_interrupt(), None, "{state:?}");
            assert!(!control.token().is_cancelled(), "{state:?}");
        }
        for state in [ActorState::Planning, ActorState::ExecutingTools, ActorState::WaitingForUser]
        {
            let control = RunControl::new(state, None);
            assert!(control.cancel(None), "a turn to end: {state:?}");
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
        assert!(!control.cancel(Some(there)), "no turn there");
        assert_eq!(control.take_interrupt(), None, "the turn runs in another thread");
        assert!(control.cancel(Some(here)), "a turn here");
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

    /// Issue 595: the agent answers in the thread of the turn whose step runs - not between turns,
    /// nor while its turn waits for a person, about to be set aside, nor once it was.
    #[test]
    fn it_answers_where_a_step_of_its_turn_runs() {
        let there = ThreadId::new();
        let control = RunControl::new(ActorState::Idle, None);
        assert_eq!(control.answering_in(), None, "before the first turn");
        control.begin_turn(there, TurnId::new());
        control.go_to(ActorState::Planning);
        assert_eq!(control.answering_in(), Some(there), "where its turn runs");
        control.go_to(ActorState::WaitingForUser);
        assert_eq!(control.answering_in(), None, "waiting for a person");
        control.set_aside(Aside::WaitingForAPerson);
        control.go_to(ActorState::Idle);
        assert_eq!(control.answering_in(), None, "set aside");
        control.begin_turn(there, TurnId::new());
        control.go_to(ActorState::Reflecting);
        assert_eq!(control.answering_in(), Some(there), "closing out its turn");
        control.go_to(ActorState::Idle);
        assert_eq!(control.answering_in(), None, "between turns");
    }

    /// Issue 602: a turn that waits for a person is set aside, and no turn runs; a cancel of its
    /// thread or a retry of it asks it to end without touching the turn that runs, and it runs
    /// again with what was asked of it - what was asked first.
    #[tokio::test]
    async fn a_turn_set_aside_is_asked_to_end_and_resumed_with_what_was_asked() {
        let (line, there) = (ThreadId::default(), ThreadId::new());
        let (waiting, running) = (TurnId::new(), TurnId::new());
        let control = RunControl::new(ActorState::WaitingForUser, Some((there, waiting)));
        control.set_aside(Aside::WaitingForAPerson);
        control.go_to(ActorState::Idle);
        assert_eq!(control.thread(), None, "no turn runs");
        assert_eq!(control.turn_under_way(there), Some(waiting), "it is set aside there");
        assert_eq!(control.standing(), ActorState::WaitingForUser, "and the agent tells it waits");

        control.begin_turn(line, running);
        control.go_to(ActorState::Planning);
        assert!(control.cancel(Some(line)), "a turn runs in the line");
        assert_eq!(control.take_interrupt(), Some(Interrupt::Cancel), "the turn that runs");
        assert_eq!(
            control.asked_aside(Aside::WaitingForAPerson).map(|(thread, _)| thread),
            None,
            "not the one set aside elsewhere"
        );
        assert!(control.retry(waiting), "a retry of the one set aside");
        assert!(control.cancel(Some(there)), "the turn set aside there counts");
        control.asked().await;
        assert_eq!(
            control.asked_aside(Aside::WaitingForAPerson).map(|(thread, _)| thread),
            Some(there),
            "it is asked to end"
        );
        assert_eq!(control.take_interrupt(), None, "the turn that runs is left as it is");

        control.go_to(ActorState::Idle);
        let asked = control.resume(there, ActorState::WaitingForUser);
        assert_eq!(asked, Some(Some(Interrupt::Retry)), "what was asked first");
        assert_eq!(control.thread(), Some(there), "it runs again");
        assert_eq!(control.state(), ActorState::WaitingForUser, "at the step it stands at");
        assert!(control.waiting_threads().is_empty(), "and is set aside no more");

        // A cancel asked as its step ended, before the run loop took it, goes with it as it is set
        // aside.
        control.cancel(Some(there));
        control.set_aside(Aside::WaitingForAPerson);
        control.asked().await;
        assert_eq!(
            control.asked_aside(Aside::WaitingForAPerson).map(|(thread, _)| thread),
            Some(there),
            "not lost"
        );
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
