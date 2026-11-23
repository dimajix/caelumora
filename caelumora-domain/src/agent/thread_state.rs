//! What an agent keeps of one thread of its channel (`AgentThreadState`, issue 602): its turn
//! there, at the step it stands at, and what its model calls there used.

use crate::actor::ActorState;
use crate::message::TokenUsage;
use crate::tool::{ToolCallResult, ToolCallStatus, ToolResultStatus};
use crate::turn::TurnState;
use serde::{Deserialize, Serialize};
use tracing::warn;

/// What an agent keeps of one thread of its channel (issue 602): its turn there, at the step it
/// stands at, and what its model calls there used. Its branch of the conversation is the head of
/// the thread in its transcript, which holds every thread's (`AgentChannelState::transcript`).
/// Where the agent stands outside a turn is its own as a whole (`AgentChannelState::asleep`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentThreadState {
    /// The turn under way in the thread, `None` between turns (issue 85), at the step it stands at
    /// (`TurnState::step`). Stored with the rest, so that a turn cut off by a crash is resumed
    /// (issue 75). An external agent does not resume from it: it is resumed from its open tasks
    /// (issue 363).
    pub turn: Option<TurnState>,
    /// What the agent's model calls in the thread used (issue 209), each counted once, as the
    /// message it produced is recorded - said, or replacing older messages in a compaction
    /// (`TokenUsage::counted`) - rather than summed over the transcript (issue 376). `None` while
    /// no call reported its tokens; an external agent's never do. How full the context is, is the
    /// last call's measure, and after a compaction the new branch as estimated then
    /// (`AgentChannelState::compact`, issue 390): never read anew, however often it is asked for.
    pub used: Option<TokenUsage>,
}

impl AgentThreadState {
    /// Makes it fit to run from (issue 75). A turn stored is a turn to resume - the server stopped
    /// or crashed in the middle of it - stored with the step that was to come next (issue 85), and
    /// most steps resume as they are: `Planning` asks the model with the turn so far, `Reflecting`
    /// takes the finished turn over, `WaitingForUser` keeps waiting, `Cancelling` ends the turn. A
    /// deliberate end never leaves a turn in the store (`stop`, `cancel`, the stop tool).
    ///
    /// `ExecutingTools` is the exception: a tool may have run without its result being stored, and
    /// running it again would repeat its effect (issue 20). Every approved call without a result
    /// is answered as not executed, with what `not_executed` gives of the turn - what the model is
    /// told of it, in the variant the turn was begun with -, so that the next step builds the tool
    /// message and the model decides again; results already stored stay. Returns how many calls
    /// were answered so.
    pub fn prepare_resume(&mut self, not_executed: impl Fn(&TurnState) -> String) -> usize {
        // A command turn is carried out between two stores and never stored under way (issue
        // 269); one found is dropped as its request would have been, unanswered - and so is
        // carrying it out (issue 299).
        if self.turn.as_ref().is_some_and(|turn| turn.command.is_some()) {
            warn!("A command turn was stored under way; it is dropped");
            self.turn = None;
        }
        let Some(turn) = self.turn.as_mut().filter(|turn| turn.step == ActorState::ExecutingTools)
        else {
            return 0;
        };
        let mut answered = 0;
        let not_executed = not_executed(turn);
        let classification = turn.work.classification.clone();
        for call in &mut turn.tool_calls {
            if matches!(call.status, ToolCallStatus::Ready { .. }) {
                call.status = ToolCallStatus::Finished(ToolCallResult::of(
                    &call.tool_call,
                    ToolResultStatus::NotExecuted,
                    not_executed.clone(),
                    classification.clone(),
                ));
                answered += 1;
            }
        }
        answered
    }

    /// Whether its turn goes on by itself once the channel runs again (issue 388): a turn stored
    /// at a step it resumes from - not one waiting for a person, whose answer starts the channel
    /// anyway, nor a command turn, which is never resumed.
    pub fn has_work_under_way(&self) -> bool {
        self.turn.as_ref().is_some_and(|turn| {
            turn.command.is_none() && turn.step.in_turn() && turn.step != ActorState::WaitingForUser
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::ActorId;
    use crate::approval::ToolMatcher;
    use crate::confidentiality::Classification;
    use crate::tool::testing::{make_tool_call, make_tool_description};
    use crate::tool::{ToolCall, ToolCallState};
    use crate::turn::CommandRequest;

    #[test]
    fn a_resumed_turn_answers_unfinished_tool_calls_as_not_executed_instead_of_running_them() {
        let tool_call =
            ToolCall { id: None, arguments: serde_json::json!({}), ..make_tool_call("write_note") };
        let ready = ToolCallState::approved(tool_call.clone());
        let done = ToolCallState::answered(
            tool_call.clone(),
            ToolCallResult::of(
                &tool_call,
                ToolResultStatus::Ok,
                "done".to_string(),
                Classification::CHANNEL_DEFAULT,
            ),
        );
        let waiting = ToolCallState::pending(
            tool_call.clone(),
            false,
            Some(ToolMatcher::proposed(
                &make_tool_description(&tool_call.function_name),
                &tool_call.arguments,
            )),
            None,
        );
        let state_in = |step: ActorState| AgentThreadState {
            turn: Some(TurnState {
                step,
                tool_calls: vec![ready.clone(), done.clone(), waiting.clone()],
                ..Default::default()
            }),
            used: None,
        };
        let calls = |state: &AgentThreadState| {
            state.turn.as_ref().map(|t| t.tool_calls.clone()).unwrap_or_default()
        };

        let mut executing = state_in(ActorState::ExecutingTools);
        assert_eq!(
            executing.prepare_resume(|_| "not executed".to_string()),
            1,
            "one approved call had no result"
        );
        let executing = calls(&executing);
        let [unfinished, finished, pending] = executing.as_slice() else { panic!("three calls") };
        let unfinished = unfinished.result().expect("answered");
        assert_eq!(unfinished.status, ToolResultStatus::NotExecuted);
        assert_eq!(unfinished.content, "not executed", "the model is told what it was given");
        assert_eq!(
            finished.result().map(|r| r.content.as_str()),
            Some("done"),
            "a stored result stays"
        );
        assert!(pending.approval_id().is_some(), "a call nobody approved is still waiting");

        // Every other state resumes as it is.
        for other in [ActorState::Planning, ActorState::WaitingForUser, ActorState::Reflecting] {
            let mut state = state_in(other);
            assert_eq!(state.prepare_resume(|_| "not executed".to_string()), 0, "{other:?}");
            assert!(matches!(calls(&state)[0].status, ToolCallStatus::Ready { .. }));
            assert_eq!(state.turn.map(|turn| turn.step), Some(other));
        }

        // A command turn is never resumed, nor carrying it out.
        let mut command = AgentThreadState {
            turn: Some(TurnState {
                step: ActorState::CarryingOutCommand,
                command: Some(a_command()),
                ..Default::default()
            }),
            used: None,
        };
        command.prepare_resume(|_| "not executed".to_string());
        assert!(command.turn.is_none());
    }

    #[test]
    fn work_under_way_is_what_goes_on_by_itself_when_the_channel_runs_again() {
        let in_turn = |step: ActorState| AgentThreadState {
            turn: Some(TurnState { step, ..TurnState::default() }),
            used: None,
        };
        for going_on in [
            ActorState::Planning,
            ActorState::ExecutingTools,
            ActorState::Reflecting,
            ActorState::Cancelling,
            ActorState::Busy,
        ] {
            assert!(in_turn(going_on).has_work_under_way(), "{going_on:?}");
        }
        assert!(!in_turn(ActorState::WaitingForUser).has_work_under_way(), "it waits for a person");
        assert!(!AgentThreadState::default().has_work_under_way(), "idle");
        let command = AgentThreadState {
            turn: Some(TurnState {
                step: ActorState::Planning,
                command: Some(a_command()),
                ..Default::default()
            }),
            used: None,
        };
        assert!(!command.has_work_under_way(), "a command turn is dropped");
    }

    fn a_command() -> CommandRequest {
        CommandRequest { command: "mark".to_string(), args: String::new(), issuer: ActorId::None }
    }
}
