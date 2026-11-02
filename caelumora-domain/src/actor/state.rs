//! How far an actor is in its work (`ActorState`), as other tasks - the moderator, the channel -
//! read it while the actor's run loop moves it on (`ProcessingStateHolder`).

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ActorState {
    #[default]
    Idle,
    /// Calling the model and streaming its response - covers everything from
    /// initial research/deliberation through proposing or extending a plan
    /// to executing its steps, and the ordinary single-shot tool loop for
    /// personalities without extended planning.
    Planning,
    /// Running approved tool calls.
    ExecutingTools,
    /// Persisting context/history and closing out the turn.
    Reflecting,
    Cancelling,
    /// Fall asleep, do not generate new messages
    Sleeping,
    /// Waiting for a person: an approval of a tool call (issue 165).
    WaitingForUser,
    /// Carrying out a command an awake agent was asked for (issues 269,
    /// 299) - `/compact`, say, which has its model summarise. Between the
    /// turns of its answers, as `Idle` is: a command turn is no answer, and
    /// a request to respond waits until it is done; nor can a cancel end it.
    /// A sleeping agent carries a command out asleep, and stays `Sleeping`.
    CarryingOutCommand,
    /// Doing something of its own accord within its turn, besides answering
    /// (issue 284) - compacting its context: the counterpart of
    /// `CarryingOutCommand`, which it does when asked. What it does is told
    /// by `ActorEvent::ActivityStarted`.
    Busy,
}

impl ActorState {
    /// Every state, in the order of its declaration - its byte
    /// (`#[repr(u8)]`) is its place here.
    const ALL: [ActorState; 9] = [
        ActorState::Idle,
        ActorState::Planning,
        ActorState::ExecutingTools,
        ActorState::Reflecting,
        ActorState::Cancelling,
        ActorState::Sleeping,
        ActorState::WaitingForUser,
        ActorState::CarryingOutCommand,
        ActorState::Busy,
    ];

    /// Whether the state belongs to a turn - an error in it ends the turn,
    /// not the actor - rather than waiting between turns. Carrying out a
    /// command is between turns: it is done while the actor waits.
    pub fn in_turn(self) -> bool {
        !matches!(self, ActorState::Idle | ActorState::Sleeping | ActorState::CarryingOutCommand)
    }

    /// Whether a cancel can still end the turn in this state (issues 235,
    /// 243): not one ending already, nor none.
    pub fn interruptible(self) -> bool {
        matches!(
            self,
            ActorState::Planning
                | ActorState::ExecutingTools
                | ActorState::WaitingForUser
                | ActorState::Busy
        )
    }

    fn from_raw(v: u8) -> Self {
        Self::ALL.get(usize::from(v)).copied().unwrap_or_default()
    }

    fn raw(self) -> u8 {
        self as u8
    }
}

/// The live state of an actor, read by other tasks - the moderator, the
/// channel - while the actor's run loop moves it on.
#[derive(Debug, Default)]
pub struct ProcessingStateHolder {
    state: AtomicU8,
}

impl ProcessingStateHolder {
    pub fn new(state: ActorState) -> Self {
        Self { state: AtomicU8::new(state.raw()) }
    }

    pub fn load(&self) -> ActorState {
        ActorState::from_raw(self.state.load(Ordering::SeqCst))
    }

    pub fn store(&self, state: ActorState) {
        self.state.store(state.raw(), Ordering::SeqCst);
    }

    /// Sets the state to `state` and returns the one before.
    pub fn swap(&self, state: ActorState) -> ActorState {
        ActorState::from_raw(self.state.swap(state.raw(), Ordering::SeqCst))
    }

    /// Moves the state from `from` to `to` if it is `from`; whether it was.
    pub fn replace(&self, from: ActorState, to: ActorState) -> bool {
        self.state
            .compare_exchange(from.raw(), to.raw(), Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_is_kept_as_its_own_byte() {
        for state in ActorState::ALL {
            assert_eq!(ActorState::from_raw(state.raw()), state);
        }
        let holder = ProcessingStateHolder::new(ActorState::Sleeping);
        assert!(!holder.replace(ActorState::Planning, ActorState::Idle), "not in that state");
        assert!(holder.replace(ActorState::Sleeping, ActorState::Idle));
        assert_eq!(holder.load(), ActorState::Idle);
        assert!(ActorState::WaitingForUser.in_turn() && !ActorState::Sleeping.in_turn());
        assert!(
            !ActorState::CarryingOutCommand.in_turn()
                && !ActorState::CarryingOutCommand.interruptible(),
            "no answer, and no cancel ends it"
        );
    }
}
