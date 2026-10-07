//! The tool calls of a turn's current step (issue 83): registering them with
//! the approvals they wait for, answering those a decision settles, recording
//! the results of those that ran, and - once every call has its result - the
//! tool message that gives them to the model.
//!
//! The calls are part of the turn's working state (`TurnState::tool_calls`),
//! stored with it so that a step resumes after a restart. [`ToolCalls`] is a
//! view on them that `Turn::tool_calls` hands out for one use at a time:
//! running a call needs the whole turn, which a longer-lived view would hold.
//! Running is the actor's - it owns the executor.

use caelumora_model::turn::TurnState;
use caelumora_model::approval::{ApprovalId, ApprovalRequest, ApprovalStatus};
use caelumora_model::message::{Message, MessageId, MessageRole};
use caelumora_model::tool::{ToolCall, ToolCallResult, ToolCallState, ToolCallStatus, ToolResultStatus};
use std::mem;

/// The tool calls of the current step of a turn, of the agent the turn's
/// context is sent by, called `agent_name` now.
pub(crate) struct ToolCalls<'t> {
    turn: &'t mut TurnState,
    agent_name: &'t str,
}

impl<'t> ToolCalls<'t> {
    pub(crate) fn new(turn: &'t mut TurnState, agent_name: &'t str) -> Self {
        Self { turn, agent_name }
    }

    /// Whether the step has calls.
    pub(crate) fn is_empty(&self) -> bool {
        self.turn.tool_calls.is_empty()
    }

    /// Takes the calls of the step over, as the actor prepared them - some
    /// answered already (issue 118) - and returns the approval requests for
    /// those that wait for a decision.
    pub(crate) fn register(&mut self, calls: Vec<ToolCallState>) -> Vec<ApprovalRequest> {
        self.turn.tool_calls = calls;
        self.pending_approvals()
    }

    /// The approval requests of the calls that still wait for a decision
    /// (`TurnState::pending_approvals`).
    pub(crate) fn pending_approvals(&self) -> Vec<ApprovalRequest> {
        self.turn.pending_approvals()
    }

    /// Records the decision on the approval `id`: an approved call may run,
    /// one denied or timed out is answered so at once, without running it.
    /// What the model is given for each outcome is decided in `crate::tool`
    /// (issue 15). `false` if no call waits for that approval.
    pub(crate) fn decide(&mut self, id: ApprovalId, decision: ApprovalStatus) -> bool {
        // A fixed text of the runtime's: as the turn (issue 168).
        let label = self.turn.work.label();
        let Some(call) = self.turn.tool_calls.iter_mut().find(|c| c.approval_id() == Some(id)) else {
            return false;
        };
        call.status = match decision {
            ApprovalStatus::Approved => ToolCallStatus::Ready,
            ApprovalStatus::Denied => ToolCallStatus::Finished(ToolCallResult::of(&call.tool_call, ToolResultStatus::Denied, crate::tool::denied_result(), label)),
            ApprovalStatus::Timeout => ToolCallStatus::Finished(ToolCallResult::of(&call.tool_call, ToolResultStatus::TimedOut, crate::tool::timed_out_result(), label)),
        };
        true
    }

    /// The calls that may run and have not yet, for the actor to run, each
    /// with its place in the step, which `record` takes.
    pub(crate) fn to_run(&self) -> Vec<(usize, ToolCall)> {
        self.turn.tool_calls.iter().enumerate()
            .filter(|(_, c)| matches!(c.status, ToolCallStatus::Ready))
            .map(|(index, c)| (index, c.tool_call.clone()))
            .collect()
    }

    /// Records `result` as what the call at `index` of the step gave.
    pub(crate) fn record(&mut self, index: usize, result: ToolCallResult) {
        if let Some(call) = self.turn.tool_calls.get_mut(index) {
            call.status = ToolCallStatus::Finished(result);
        }
    }

    /// Ends the step as its turn ends (issue 148): answers every call not
    /// yet finished as not executed, `because` - why the turn ended - and
    /// keeps the tool message as `collect` does, so that no call is left
    /// unanswered and the results of those that ran are kept. Returns the
    /// calls it abandoned as they stood before, still waiting or ready.
    pub(crate) fn abandon(&mut self, because: &str, internal: bool) -> Vec<ToolCallState> {
        // A fixed text of the runtime's: as the turn (issue 168).
        let label = self.turn.work.label();
        let mut abandoned = Vec::new();
        for call in self.turn.tool_calls.iter_mut().filter(|c| !c.is_finished()) {
            abandoned.push(call.clone());
            let content = crate::tool::abandoned_result(because);
            call.status = ToolCallStatus::Finished(ToolCallResult::of(&call.tool_call, ToolResultStatus::NotExecuted, content, label.clone()));
        }
        self.collect(internal);
        abandoned
    }

    /// Once every call has its result: the tool message that gives them to
    /// the model, kept in the turn - `internal` like the step that made the
    /// calls - and the step's calls done. `None` while a call still waits
    /// for its approval, and for a step without calls: there is nothing to
    /// give.
    pub(crate) fn collect(&mut self, internal: bool) -> Option<Message> {
        if self.is_empty() || self.turn.tool_calls.iter().any(|c| !c.is_finished()) {
            return None;
        }
        let tool_results: Vec<_> = mem::take(&mut self.turn.tool_calls).into_iter()
            .filter_map(|c| match c.status {
                ToolCallStatus::Finished(result) => Some(result),
                _ => None,
            })
            .collect();
        // The model sees the results from here on (issue 168).
        for result in &tool_results {
            self.turn.work.see(&result.classification);
        }
        let message = Message {
            id: MessageId::new(),
            sender_id: self.turn.work.event_context.sender_id,
            sender_name: self.agent_name.to_string(),
            role: MessageRole::Tool,
            tool_results,
            content: None,
            timestamp: chrono::Utc::now(),
            internal,
            turn_id: Some(self.turn.work.turn_id),
            ..Message::new(self.turn.work.label())
        };
        self.turn.push(message.clone());
        Some(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::AgentId;
    use caelumora_model::tool::ToolVisibility;

    fn call(name: &str) -> ToolCall {
        ToolCall { id: Some(name.to_string()), function_name: name.to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() }
    }

    #[test]
    fn a_steps_calls_are_answered_run_and_given_back_to_the_model_in_one_message() {
        let mut turn = TurnState::default();
        turn.work.event_context.sender_id = ActorId::Agent(AgentId::default());
        fn view(turn: &mut TurnState) -> ToolCalls<'_> {
            ToolCalls::new(turn, "agent")
        }

        // Registering asks for the approvals still to be given.
        let requests = view(&mut turn).register(vec![
            ToolCallState::approved(call("run")),
            ToolCallState::pending(call("ask"), String::new(), false, caelumora_model::approval::ToolMatcher::exact(&call("ask"))),
        ]);
        assert_eq!(requests.iter().map(|r| r.tool_call.function_name.as_str()).collect::<Vec<_>>(), ["ask"]);

        // Waiting for one of them, there is no tool message yet.
        let to_run = view(&mut turn).to_run();
        assert_eq!(to_run.iter().map(|(_, c)| c.function_name.as_str()).collect::<Vec<_>>(), ["run"]);
        view(&mut turn).record(to_run[0].0, ToolCallResult::of(&to_run[0].1, ToolResultStatus::Ok, "ran".to_string(), caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT));
        assert!(view(&mut turn).collect(false).is_none(), "one call still waits");

        // A denial answers the other without running it.
        assert!(view(&mut turn).decide(requests[0].id, ApprovalStatus::Denied));
        assert!(!view(&mut turn).decide(ApprovalId::new(), ApprovalStatus::Denied), "no such call");
        assert!(view(&mut turn).to_run().is_empty(), "nothing left to run");
        let message = view(&mut turn).collect(false).expect("every call has its result");
        let results: Vec<_> = message.tool_results.iter().map(|r| (r.tool_name.as_str(), r.status)).collect();
        assert_eq!(results, [("run", ToolResultStatus::Ok), ("ask", ToolResultStatus::Denied)]);
        assert_eq!(message.role, MessageRole::Tool);
        assert!(view(&mut turn).is_empty(), "the step's calls are done");
        assert!(view(&mut turn).collect(false).is_none(), "nothing is left to give");
        assert_eq!(turn.messages().last().map(|m| m.id), Some(message.id), "the turn keeps it for the model");
    }
}
