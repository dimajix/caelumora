//! What interrupts the tool calls of an agent's turn: which calls of the step under way a user may
//! interrupt on their own (issue 276), and whether a call was broken off by that, by its turn's
//! end, or by its channel stopping (issue 377).

use super::run_control::RunControl;
use crate::tool::Interruption;
use caelumora_domain::work::TurnId;
use std::collections::HashMap;
use std::future::pending;
use std::sync::{Mutex, MutexGuard};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// The calls of the last tool step of an agent's turn that can be interrupted on their own (issue
/// 276), kept with the turn they belong to, so that interrupting one reaches no call of another
/// turn: by their ids, each with where the name of whoever interrupts it goes. The run loop holds
/// the other end while the call runs or waits to run, and drops it after: a call that ran can no
/// longer be interrupted.
#[derive(Debug, Default)]
pub(crate) struct CallInterrupts {
    calls: Mutex<Option<(TurnId, Senders)>>,
}

/// Where the name of whoever interrupts a call goes, by the call's id.
type Senders = HashMap<String, oneshot::Sender<String>>;

impl CallInterrupts {
    /// The calls `ids` of a tool step of the turn `turn` begin: from now on each can be
    /// interrupted on its own. Returns, by id, what tells the run loop who interrupted a call -
    /// for `interruption`, or asked before the call runs.
    pub(crate) fn begin(
        &self,
        turn: TurnId,
        ids: impl IntoIterator<Item = String>,
    ) -> HashMap<String, oneshot::Receiver<String>> {
        let (senders, receivers) = ids
            .into_iter()
            .map(|id| {
                let (sender, receiver) = oneshot::channel();
                ((id.clone(), sender), (id, receiver))
            })
            .unzip();
        *self.lock() = Some((turn, senders));
        receivers
    }

    /// Interrupts the call `call` of the turn `turn` alone, on behalf of `by`, and returns whether
    /// it ran or waited to run in the turn under way of `control`, which can still be ended
    /// (`RunControl::under_way`). The turn goes on.
    pub(crate) fn interrupt(
        &self,
        control: &RunControl,
        turn: TurnId,
        call: &str,
        by: String,
    ) -> bool {
        if control.under_way().is_none_or(|(_, under_way)| under_way != turn) {
            return false;
        }
        let mut calls = self.lock();
        let Some((_, of_turn)) = calls.as_mut().filter(|(of, _)| *of == turn) else {
            return false;
        };
        of_turn.remove(call).is_some_and(|interrupt| interrupt.send(by).is_ok())
    }

    fn lock(&self) -> MutexGuard<'_, Option<(TurnId, Senders)>> {
        self.calls.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// What interrupts a call of the turn whose token is `turn`: the turn ending, or the channel
/// stopping (issue 377) - told apart by `control`, a stop ending its run without the agent
/// leaving -, or - through `by_user`, for a call that can be interrupted on its own
/// (`CallInterrupts::begin`) - a user interrupting this call alone (issue 276). The turn's end
/// wins.
pub(crate) async fn interruption(
    control: &RunControl,
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
        () = turn.cancelled() => match control.is_ending() && !control.has_left() {
            true => Interruption::Stopped,
            false => Interruption::TurnEnded,
        },
        by = by_user => Interruption::ByUser(by),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::actor::ActorState;
    use caelumora_domain::channel::ThreadId;

    /// A control in the tool step of the turn `turn`.
    fn executing(turn: TurnId) -> RunControl {
        RunControl::new(ActorState::ExecutingTools, Some((ThreadId::default(), turn)))
    }

    #[tokio::test]
    async fn a_call_is_interrupted_alone_only_in_the_turn_it_runs_in_and_only_once() {
        let (turn, other) = (TurnId::new(), TurnId::new());
        let control = executing(turn);
        let call_interrupts = CallInterrupts::default();
        let mut by_user = call_interrupts.begin(turn, ["a".to_string(), "b".to_string()]);
        let interrupt =
            |turn, call: &str, by: &str| call_interrupts.interrupt(&control, turn, call, by.into());
        assert!(!interrupt(other, "a", "alice"), "not of another turn");
        assert!(!interrupt(turn, "c", "alice"), "not a call of the step");

        assert!(interrupt(turn, "a", "alice"));
        assert!(!interrupt(turn, "a", "bob"), "once");
        let interrupted = interruption(&control, control.token(), by_user.remove("a")).await;
        assert_eq!(interrupted, Interruption::ByUser("alice".to_string()));
        assert!(
            !control.token().is_cancelled() && control.take_interrupt().is_none(),
            "the turn goes on"
        );

        // A call that ran can no longer be interrupted.
        drop(by_user.remove("b"));
        assert!(!interrupt(turn, "b", "alice"));

        // Nor, of the turn before, the call of the next one's step of the same id.
        let mut next = call_interrupts.begin(TurnId::new(), ["c".to_string()]);
        assert!(!interrupt(turn, "c", "alice"), "of a turn whose calls are gone");
        let waiting = next.remove("c").map(|mut c| c.try_recv());
        assert_eq!(waiting, Some(Err(oneshot::error::TryRecvError::Empty)), "the next turn's call");
    }

    #[tokio::test]
    async fn the_turn_s_end_interrupts_every_call_and_wins() {
        let turn = TurnId::new();
        let control = executing(turn);
        let call_interrupts = CallInterrupts::default();
        let mut by_user = call_interrupts.begin(turn, ["a".to_string()]);
        assert!(call_interrupts.interrupt(&control, turn, "a", "alice".to_string()));
        control.cancel(None);
        assert_eq!(
            interruption(&control, control.token(), by_user.remove("a")).await,
            Interruption::TurnEnded
        );
        assert_eq!(
            interruption(&control, control.token(), None).await,
            Interruption::TurnEnded,
            "also a call without an id"
        );

        let control = RunControl::new(ActorState::Cancelling, Some((ThreadId::default(), turn)));
        let _by_user = call_interrupts.begin(turn, ["a".to_string()]);
        assert!(
            !call_interrupts.interrupt(&control, turn, "a", "alice".to_string()),
            "not in a turn that is ending"
        );
    }

    /// A call broken off because the channel stops is told so (issue 377): its turn is kept, unlike
    /// one that is cancelled or given up.
    #[tokio::test]
    async fn a_stop_interrupts_every_call_as_stopped_and_leaving_as_ended() {
        let turn = TurnId::new();
        let control = executing(turn);
        let mut by_user = CallInterrupts::default().begin(turn, ["a".to_string()]);
        control.stop();
        assert_eq!(
            interruption(&control, control.token(), by_user.remove("a")).await,
            Interruption::Stopped
        );

        let control = executing(turn);
        control.stop();
        control.leave();
        assert_eq!(interruption(&control, control.token(), None).await, Interruption::TurnEnded);
    }
}
