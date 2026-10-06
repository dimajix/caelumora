//! The run control of an `AgentActor`: what its run loop shares with
//! `cancel`, `retry`, `stop` and `wakeup` (see `RunControl`).

use caelumora_model::actor::{ActorState, ProcessingStateHolder};
use caelumora_model::work::TurnId;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio_util::sync::CancellationToken;

/// What `cancel`, `retry` or `stop` asked of the run loop, which takes it
/// after every step and alone decides what follows (issue 152). A stop wins
/// over everything; of a cancel and a retry, the first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Interrupt {
    /// End the turn under way and take it over (`Cancelling`).
    Cancel = 1,
    /// Give the turn under way up and end the run loop: the actor left the
    /// channel.
    Stop = 2,
    /// Give the turn under way up, because its answer was asked for again
    /// (issue 235): a turn of its own answers instead.
    Retry = 3,
}

/// `RunControl::interrupt` when nothing was asked.
const NO_INTERRUPT: u8 = 0;

/// Where the run loop of an agent stands, and what `cancel`, `stop` and
/// `wakeup` asked of it. These come from other tasks and are synchronous:
/// they only leave word here and interrupt the step under way; the run loop
/// takes the word after every step and alone decides what follows (issue
/// 152).
#[derive(Debug)]
pub(crate) struct RunControl {
    state: ProcessingStateHolder,
    /// The turn that runs or ran last, by its id (issue 235), and its
    /// token, both replaced when the next one begins. The token is cancelled
    /// by `cancel` and `stop` while the turn runs: the streaming loop waits
    /// on it next to the model's stream, so that a cancel drops the request
    /// instead of waiting for its next chunk, and so does waiting for an
    /// approval. `None` before the first turn. A `std` mutex, because
    /// `cancel` is synchronous.
    turn: std::sync::Mutex<(Option<TurnId>, CancellationToken)>,
    /// What `cancel` or `stop` asked of the run loop (an `Interrupt`, or
    /// `NO_INTERRUPT`), taken after every step.
    interrupt: AtomicU8,
}

impl RunControl {
    /// The run control of an agent that stands at `state`, in the turn
    /// `turn` if it resumes one.
    pub(crate) fn new(state: ActorState, turn: Option<TurnId>) -> Self {
        Self {
            state: ProcessingStateHolder::new(state),
            turn: std::sync::Mutex::new((turn, CancellationToken::new())),
            interrupt: AtomicU8::new(NO_INTERRUPT),
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
    pub(crate) fn wakeup(&self) {
        self.state.replace(ActorState::Sleeping, ActorState::Idle);
    }

    /// The turn `turn` begins: it gets a token of its own, and a cancel or
    /// a retry that came as the turn before ended has nothing to end here;
    /// a stop still has.
    pub(crate) fn begin_turn(&self, turn: TurnId) {
        *self.lock_turn() = (Some(turn), CancellationToken::new());
        let _ = self.interrupt.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |asked| (asked != Interrupt::Stop as u8).then_some(NO_INTERRUPT));
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
        matches!(self.state(), ActorState::WaitingForApproval | ActorState::ExecutingTools | ActorState::Planning)
    }

    /// The token of the current turn, cancelled by `cancel` and `stop`.
    pub(crate) fn token(&self) -> CancellationToken {
        self.lock_turn().1.clone()
    }

    fn lock_turn(&self) -> std::sync::MutexGuard<'_, (Option<TurnId>, CancellationToken)> {
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

    /// Asks the run loop to end: a model call in flight is interrupted, and
    /// the turn under way is given up instead of taken over.
    pub(crate) fn stop(&self) {
        self.interrupt(Interrupt::Stop);
    }

    /// Leaves `interrupt` for the run loop - a stop is not turned back into
    /// a cancel - and interrupts the step under way.
    fn interrupt(&self, interrupt: Interrupt) {
        match interrupt {
            Interrupt::Cancel | Interrupt::Retry => {
                let _ = self.interrupt.compare_exchange(NO_INTERRUPT, interrupt as u8, Ordering::SeqCst, Ordering::SeqCst);
            }
            Interrupt::Stop => self.interrupt.store(Interrupt::Stop as u8, Ordering::SeqCst),
        }
        self.token().cancel();
    }

    /// What was asked of the run loop since it last looked, if anything.
    pub(crate) fn take_interrupt(&self) -> Option<Interrupt> {
        match self.interrupt.swap(NO_INTERRUPT, Ordering::SeqCst) {
            v if v == Interrupt::Cancel as u8 => Some(Interrupt::Cancel),
            v if v == Interrupt::Stop as u8 => Some(Interrupt::Stop),
            v if v == Interrupt::Retry as u8 => Some(Interrupt::Retry),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancel_interrupts_only_a_turn_under_way() {
        for state in [ActorState::Idle, ActorState::Sleeping, ActorState::Reflecting, ActorState::Cancelling] {
            let control = RunControl::new(state, None);
            control.cancel();
            assert_eq!(control.take_interrupt(), None, "{state:?}");
            assert!(!control.token().is_cancelled(), "{state:?}");
        }
        for state in [ActorState::Planning, ActorState::ExecutingTools, ActorState::WaitingForApproval] {
            let control = RunControl::new(state, None);
            control.cancel();
            assert!(control.token().is_cancelled(), "{state:?}");
            assert_eq!(control.take_interrupt(), Some(Interrupt::Cancel), "{state:?}");
            assert_eq!(control.take_interrupt(), None, "taken once: {state:?}");
        }
    }

    #[test]
    fn a_stop_wins_over_a_cancel_whichever_comes_first() {
        let control = RunControl::new(ActorState::Planning, None);
        control.cancel();
        control.stop();
        assert_eq!(control.take_interrupt(), Some(Interrupt::Stop));

        let control = RunControl::new(ActorState::Planning, None);
        control.stop();
        control.cancel();
        assert_eq!(control.take_interrupt(), Some(Interrupt::Stop));
    }

    #[test]
    fn a_new_turn_drops_a_leftover_cancel_but_not_a_stop() {
        let control = RunControl::new(ActorState::Planning, None);
        control.cancel();
        control.begin_turn(TurnId::new());
        assert!(!control.token().is_cancelled(), "the new turn has a token of its own");
        assert_eq!(control.take_interrupt(), None);

        control.stop();
        control.begin_turn(TurnId::new());
        assert_eq!(control.take_interrupt(), Some(Interrupt::Stop));
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

    #[test]
    fn only_a_sleeping_agent_is_woken() {
        let control = RunControl::new(ActorState::Sleeping, None);
        control.wakeup();
        assert_eq!(control.state(), ActorState::Idle);

        let control = RunControl::new(ActorState::Planning, None);
        control.wakeup();
        assert_eq!(control.state(), ActorState::Planning);
    }
}
