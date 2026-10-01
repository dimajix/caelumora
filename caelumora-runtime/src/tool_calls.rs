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

use caelumora_model::actor::ActorId;
use caelumora_model::agent::{AgentId, TurnState};
use caelumora_model::approval::{ApprovalId, ApprovalRequest, ApprovalStatus};
use caelumora_model::message::{Message, MessageId, MessageRole};
use caelumora_model::tool::{ToolCall, ToolCallResult, ToolCallState, ToolResultStatus};
use std::mem;

/// The tool calls of the current step of a turn of `agent_id`.
pub(crate) struct ToolCalls<'t> {
    turn: &'t mut TurnState,
    agent_id: AgentId,
    agent_name: &'t str,
}

impl<'t> ToolCalls<'t> {
    pub(crate) fn new(turn: &'t mut TurnState, agent_id: AgentId, agent_name: &'t str) -> Self {
        Self { turn, agent_id, agent_name }
    }

    /// Whether the step has calls.
    pub(crate) fn is_empty(&self) -> bool {
        self.turn.tool_calls.is_empty()
    }

    /// Takes the calls of the step over, as the actor prepared them - some
    /// answered already (issue 118) - and returns the approval requests for
    /// those that wait for a decision.
    pub(crate) fn register(&mut self, calls: Vec<ToolCallState>) -> Vec<ApprovalRequest> {
        let (session_id, channel_id) = (self.turn.event_context.session_id, self.turn.event_context.channel_id);
        let requests = calls.iter()
            .filter(|c| c.status == ApprovalStatus::Pending)
            .map(|c| ApprovalRequest {
                id: c.id,
                session_id,
                channel_id,
                agent_id: self.agent_id,
                tool_call: c.tool_call.clone(),
                description: c.description.clone(),
            })
            .collect();
        self.turn.tool_calls = calls;
        requests
    }

    /// Records the decision on the call `id`; `false` if there is no such
    /// call.
    pub(crate) fn decide(&mut self, id: ApprovalId, status: ApprovalStatus) -> bool {
        match self.turn.tool_calls.iter_mut().find(|c| c.id == id) {
            Some(call) => {
                call.status = status;
                true
            }
            None => false,
        }
    }

    /// Answers every call that a decision settles without running it -
    /// denied, timed out - and returns the approved ones that still wait for
    /// their result, for the actor to run. What the model is given for each
    /// outcome is decided in `crate::tool` (issue 15).
    pub(crate) fn to_run(&mut self) -> Vec<(ApprovalId, ToolCall)> {
        let mut to_run = Vec::new();
        // A fixed text of the runtime's: as the turn (issue 168).
        let label = self.turn.label();
        for call in self.turn.tool_calls.iter_mut().filter(|c| c.result.is_none()) {
            let answer = match call.status {
                ApprovalStatus::Denied => Some((ToolResultStatus::Denied, crate::tool::denied_result())),
                ApprovalStatus::Timeout => Some((ToolResultStatus::TimedOut, crate::tool::timed_out_result())),
                ApprovalStatus::Approved => {
                    to_run.push((call.id, call.tool_call.clone()));
                    None
                }
                ApprovalStatus::Pending => None,
            };
            if let Some((status, content)) = answer {
                call.result = Some(ToolCallResult::of(&call.tool_call, status, content, label.clone()));
            }
        }
        to_run
    }

    /// Records `result` as what the call `id` gave.
    pub(crate) fn record(&mut self, id: ApprovalId, result: ToolCallResult) {
        if let Some(call) = self.turn.tool_calls.iter_mut().find(|c| c.id == id) {
            call.result = Some(result);
        }
    }

    /// Ends the step as its turn ends (issue 148): answers every call still
    /// without a result as not executed, `because` - why the turn ended -
    /// and keeps the tool message as `collect` does, so that no call is left
    /// unanswered and the results of those that ran are kept. Returns the
    /// approval requests still waiting for a decision, for the agent to
    /// withdraw.
    pub(crate) fn abandon(&mut self, because: &str, internal: bool) -> Vec<ApprovalId> {
        if self.is_empty() {
            return Vec::new();
        }
        let waiting = self.turn.tool_calls.iter()
            .filter(|c| c.status == ApprovalStatus::Pending && c.result.is_none())
            .map(|c| c.id)
            .collect();
        // A fixed text of the runtime's: as the turn (issue 168).
        let label = self.turn.label();
        for call in self.turn.tool_calls.iter_mut().filter(|c| c.result.is_none()) {
            let content = crate::tool::abandoned_result(because);
            call.result = Some(ToolCallResult::of(&call.tool_call, ToolResultStatus::NotExecuted, content, label.clone()));
        }
        self.collect(internal);
        waiting
    }

    /// Once every call has its result: the tool message that gives them to
    /// the model, kept in the turn - `internal` like the step that made the
    /// calls - and the step's calls done. `None` while a call still waits
    /// for its approval.
    pub(crate) fn collect(&mut self, internal: bool) -> Option<Message> {
        if self.turn.tool_calls.iter().any(|c| c.result.is_none()) {
            return None;
        }
        let tool_results: Vec<_> = mem::take(&mut self.turn.tool_calls).into_iter()
            .filter_map(|c| c.result)
            .collect();
        // The model sees the results from here on (issue 168).
        for result in &tool_results {
            self.turn.see(&result.classification);
        }
        let message = Message {
            id: MessageId::new(),
            sender_id: ActorId::Agent(self.agent_id),
            sender_name: self.agent_name.to_string(),
            role: MessageRole::Tool,
            tool_results,
            content: None,
            timestamp: chrono::Utc::now(),
            internal,
            ..Message::new(self.turn.label())
        };
        self.turn.push(message.clone());
        Some(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::tool::ToolVisibility;

    fn call(name: &str, status: ApprovalStatus) -> ToolCallState {
        ToolCallState {
            id: ApprovalId::new(),
            status,
            tool_call: ToolCall { id: Some(name.to_string()), function_name: name.to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN },
            description: String::new(),
            result: None,
        }
    }

    #[test]
    fn a_steps_calls_are_answered_run_and_given_back_to_the_model_in_one_message() {
        let mut turn = TurnState::default();
        fn view(turn: &mut TurnState) -> ToolCalls<'_> {
            ToolCalls::new(turn, AgentId::default(), "agent")
        }

        // Registering asks for the approvals still to be given.
        let requests = view(&mut turn).register(vec![call("run", ApprovalStatus::Approved), call("ask", ApprovalStatus::Pending)]);
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
        assert_eq!(turn.messages().last().map(|m| m.id), Some(message.id), "the turn keeps it for the model");
    }
}
