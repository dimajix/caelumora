//! The run control of an agent's actor - an `AgentActor` or an
//! `ExternalAgentActor` (issue 363): what its run loop shares with
//! `cancel`, `retry`, `stop`, `leave`, `wakeup` and `interrupt_call` (see
//! `RunControl`). Held by its body (`AgentBody`).

use crate::tool::Interruption;
use caelumora_model::actor::{ActorState, ProcessingStateHolder};
use caelumora_model::work::TurnId;
use std::collections::HashMap;
use std::future::pending;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use tokio::sync::{oneshot, Notify};
use tokio_util::sync::CancellationToken;
use std::sync::{Mutex, MutexGuard};

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
    /// The turn that runs or ran last, by its id (issue 235), and its
    /// token, both replaced when the next one begins. The token is cancelled
    /// by `cancel`, `stop` and `leave` while the turn runs: the streaming loop waits
    /// on it next to the model's stream, so that a cancel drops the request
    /// instead of waiting for its next chunk, and so does waiting for an
    /// approval. `None` before the first turn. A `std` mutex, because
    /// `cancel` is synchronous. Every token is a child of `ending`.
    turn: Mutex<(Option<TurnId>, CancellationToken)>,
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
    /// The calls of the last tool step that can be interrupted on their own
    /// (issue 276), by their ids, each with where the name of whoever
    /// interrupts it goes. The run loop holds the other end while the call
    /// runs or waits to run, and drops it after: a call that ran can no
    /// longer be interrupted.
    calls: Mutex<HashMap<String, oneshot::Sender<String>>>,
}

impl RunControl {
    /// The run control of an agent that stands at `state`, in the turn
    /// `turn` if it resumes one.
    pub(crate) fn new(state: ActorState, turn: Option<TurnId>) -> Self {
        let ending = CancellationToken::new();
        Self {
            state: ProcessingStateHolder::new(state),
            announced: ProcessingStateHolder::new(state),
            woken: Notify::new(),
            turn: Mutex::new((turn, ending.child_token())),
            ending,
            left: AtomicBool::new(false),
            interrupt: AtomicU8::new(NO_INTERRUPT),
            calls: Mutex::new(HashMap::new()),
        }
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

    /// The state the run loop stands at, if it has not announced it yet
    /// (issue 243); it counts as announced from now.
    pub(crate) fn unannounced(&self) -> Option<ActorState> {
        let state = self.state();
        (self.announced.swap(state) != state).then_some(state)
    }

    /// The turn `turn` begins: it gets a token of its own, and a cancel or
    /// a retry that came as the turn before ended has nothing to end here;
    /// a stop or leaving still has.
    pub(crate) fn begin_turn(&self, turn: TurnId) {
        *self.lock_turn() = (Some(turn), self.ending.child_token());
        let _ = self.interrupt.try_update(Ordering::SeqCst, Ordering::SeqCst, |asked| (!ends_run(asked)).then_some(NO_INTERRUPT));
    }

    /// The turn under way, if the run loop is in one that can still be
    /// ended (issue 235) - not one ending already, which a cancel or a
    /// retry would find over.
    pub(crate) fn turn_under_way(&self) -> Option<TurnId> {
        let (turn, _) = *self.lock_turn();
        turn.filter(|_| self.interruptible())
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

    fn lock_turn(&self) -> MutexGuard<'_, (Option<TurnId>, CancellationToken)> {
        self.turn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Asks for the turn under way to end and be taken over. Outside a turn
    /// there is nothing to cancel, and a turn already ending is ended
    /// anyway.
    pub(crate) fn cancel(&self) {
        if self.interruptible() {
            self.interrupt(Interrupt::Cancel);
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
        if under_way.0 != Some(turn) || !self.interruptible() {
            return false;
        }
        let _ = self.interrupt.compare_exchange(NO_INTERRUPT, Interrupt::Retry as u8, Ordering::SeqCst, Ordering::SeqCst);
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
                let _ = self.interrupt.compare_exchange(NO_INTERRUPT, interrupt as u8, Ordering::SeqCst, Ordering::SeqCst);
            }
            Interrupt::Stop => {
                let _ = self.interrupt.try_update(Ordering::SeqCst, Ordering::SeqCst, |asked| (asked != Interrupt::Leave as u8).then_some(Interrupt::Stop as u8));
            }
            Interrupt::Leave => self.interrupt.store(Interrupt::Leave as u8, Ordering::SeqCst),
        }
        self.token().cancel();
    }

    /// The calls `ids` of a tool step of the turn under way begin (issue
    /// 276): from now on each can be interrupted on its own. Returns, by
    /// id, what tells the run loop who interrupted a call - for
    /// `interruption`, or asked before the call runs.
    pub(crate) fn begin_calls(&self, ids: impl IntoIterator<Item = String>) -> HashMap<String, oneshot::Receiver<String>> {
        let (senders, receivers) = ids.into_iter()
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
        if under_way.0 != Some(turn) || !self.interruptible() {
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
    pub(crate) async fn interruption(&self, turn: CancellationToken, by_user: Option<oneshot::Receiver<String>>) -> Interruption {
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
    use std::time::Duration;
    use tokio::time;

    #[test]
    fn a_cancel_interrupts_only_a_turn_under_way() {
        for state in [ActorState::Idle, ActorState::Sleeping, ActorState::Reflecting, ActorState::Cancelling] {
            let control = RunControl::new(state, None);
            control.cancel();
            assert_eq!(control.take_interrupt(), None, "{state:?}");
            assert!(!control.token().is_cancelled(), "{state:?}");
        }
        for state in [ActorState::Planning, ActorState::ExecutingTools, ActorState::WaitingForUser] {
            let control = RunControl::new(state, None);
            control.cancel();
            assert!(control.token().is_cancelled(), "{state:?}");
            assert_eq!(control.take_interrupt(), Some(Interrupt::Cancel), "{state:?}");
            assert_eq!(control.take_interrupt(), None, "taken once: {state:?}");
        }
    }

    #[test]
    fn a_stop_wins_over_a_cancel_whichever_comes_first_and_leaving_over_both() {
        let control = RunControl::new(ActorState::Planning, None);
        control.cancel();
        control.stop();
        assert_eq!(control.take_interrupt(), Some(Interrupt::Stop));

        let control = RunControl::new(ActorState::Planning, None);
        control.stop();
        control.cancel();
        assert_eq!(control.take_interrupt(), Some(Interrupt::Stop));

        let control = RunControl::new(ActorState::Planning, None);
        control.leave();
        control.stop();
        control.cancel();
        assert_eq!(control.take_interrupt(), Some(Interrupt::Leave));

        let control = RunControl::new(ActorState::Planning, None);
        control.stop();
        control.leave();
        assert_eq!(control.take_interrupt(), Some(Interrupt::Leave));
    }

    #[test]
    fn a_new_turn_drops_a_leftover_cancel_but_not_a_stop_nor_leaving() {
        let control = RunControl::new(ActorState::Planning, None);
        control.cancel();
        control.begin_turn(TurnId::new());
        assert!(!control.token().is_cancelled(), "the new turn has a token of its own");
        assert_eq!(control.take_interrupt(), None);

        control.stop();
        control.begin_turn(TurnId::new());
        assert_eq!(control.take_interrupt(), Some(Interrupt::Stop));
        assert!(control.token().is_cancelled(), "a turn begun after a stop has nothing to run for");
        assert!(control.is_ending() && !control.has_left(), "stopped, it did not leave");

        let control = RunControl::new(ActorState::Planning, None);
        control.leave();
        control.begin_turn(TurnId::new());
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
        let resumed = TurnId::new();
        let control = RunControl::new(ActorState::ExecutingTools, Some(resumed));
        assert_eq!(control.turn_under_way(), Some(resumed), "a resumed turn is the one it was");

        let next = TurnId::new();
        control.go_to(ActorState::Idle);
        assert_eq!(control.turn_under_way(), None, "between turns");
        control.begin_turn(next);
        control.go_to(ActorState::Planning);
        assert_eq!(control.turn_under_way(), Some(next));
        control.go_to(ActorState::Reflecting);
        assert_eq!(control.turn_under_way(), None, "nor once it ends");
    }

    #[test]
    fn a_retry_interrupts_only_the_turn_it_names_while_it_can_still_be_ended() {
        let (turn, other) = (TurnId::new(), TurnId::new());
        let control = RunControl::new(ActorState::Planning, Some(turn));
        assert!(!control.retry(other), "another turn");
        assert!(!control.token().is_cancelled() && control.take_interrupt().is_none(), "ends nothing");
        assert!(control.retry(turn));
        assert!(control.token().is_cancelled());
        assert_eq!(control.take_interrupt(), Some(Interrupt::Retry));

        for state in [ActorState::Idle, ActorState::Reflecting, ActorState::Cancelling] {
            let control = RunControl::new(state, Some(turn));
            assert!(!control.retry(turn), "not under way any more: {state:?}");
            assert_eq!(control.take_interrupt(), None, "{state:?}");
        }
    }

    #[test]
    fn of_a_cancel_and_a_retry_the_first_wins_and_a_stop_over_both() {
        let turn = TurnId::new();
        let control = RunControl::new(ActorState::Planning, Some(turn));
        control.cancel();
        assert!(control.retry(turn), "the turn was the one under way");
        assert_eq!(control.take_interrupt(), Some(Interrupt::Cancel), "but the cancel came first");

        let control = RunControl::new(ActorState::Planning, Some(turn));
        assert!(control.retry(turn));
        control.cancel();
        assert_eq!(control.take_interrupt(), Some(Interrupt::Retry));

        let control = RunControl::new(ActorState::Planning, Some(turn));
        assert!(control.retry(turn));
        control.stop();
        assert_eq!(control.take_interrupt(), Some(Interrupt::Stop));

        // One that came as the turn ended has nothing to end in the next.
        let control = RunControl::new(ActorState::Planning, Some(turn));
        assert!(control.retry(turn));
        control.begin_turn(TurnId::new());
        assert_eq!(control.take_interrupt(), None);
    }

    #[tokio::test]
    async fn a_call_is_interrupted_alone_only_in_the_turn_it_runs_in_and_only_once() {
        let (turn, other) = (TurnId::new(), TurnId::new());
        let control = RunControl::new(ActorState::ExecutingTools, Some(turn));
        let mut calls = control.begin_calls(["a".to_string(), "b".to_string()]);
        assert!(!control.interrupt_call(other, "a", "alice".to_string()), "not of another turn");
        assert!(!control.interrupt_call(turn, "c", "alice".to_string()), "not a call of the step");

        assert!(control.interrupt_call(turn, "a", "alice".to_string()));
        assert!(!control.interrupt_call(turn, "a", "bob".to_string()), "once");
        let interrupted = control.interruption(control.token(), calls.remove("a")).await;
        assert_eq!(interrupted, Interruption::ByUser("alice".to_string()));
        assert!(!control.token().is_cancelled() && control.take_interrupt().is_none(), "the turn goes on");

        // A call that ran can no longer be interrupted.
        drop(calls.remove("b"));
        assert!(!control.interrupt_call(turn, "b", "alice".to_string()));
    }

    #[tokio::test]
    async fn the_turn_s_end_interrupts_every_call_and_wins() {
        let turn = TurnId::new();
        let control = RunControl::new(ActorState::ExecutingTools, Some(turn));
        let mut calls = control.begin_calls(["a".to_string()]);
        assert!(control.interrupt_call(turn, "a", "alice".to_string()));
        control.cancel();
        assert_eq!(control.interruption(control.token(), calls.remove("a")).await, Interruption::TurnEnded);
        assert_eq!(control.interruption(control.token(), None).await, Interruption::TurnEnded, "also a call without an id");

        let control = RunControl::new(ActorState::Cancelling, Some(turn));
        let _calls = control.begin_calls(["a".to_string()]);
        assert!(!control.interrupt_call(turn, "a", "alice".to_string()), "not in a turn that is ending");
    }

    /// A call broken off because the channel stops is told so (issue 377):
    /// its turn is kept, unlike one that is cancelled or given up.
    #[tokio::test]
    async fn a_stop_interrupts_every_call_as_stopped_and_leaving_as_ended() {
        let turn = TurnId::new();
        let control = RunControl::new(ActorState::ExecutingTools, Some(turn));
        let mut calls = control.begin_calls(["a".to_string()]);
        control.stop();
        assert_eq!(control.interruption(control.token(), calls.remove("a")).await, Interruption::Stopped);

        let control = RunControl::new(ActorState::ExecutingTools, Some(turn));
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
        time::timeout(Duration::from_secs(1), control.woken()).await.expect("told, though nobody waited yet");
        assert_eq!(control.unannounced(), Some(ActorState::Idle));
        assert_eq!(control.unannounced(), None, "announced once");

        control.wakeup();
        assert!(time::timeout(Duration::from_millis(50), control.woken()).await.is_err(), "awake already: nothing to tell");
        control.go_to(ActorState::Planning);
        control.go_to(ActorState::Idle);
        assert_eq!(control.unannounced(), None, "back where it was announced");
    }
}
