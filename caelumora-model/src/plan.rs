use crate::agent::AgentId;
use crate::approval::{ApprovalId, ApprovalStatus};
use crate::channel::ChannelId;
use crate::session::SessionId;
use serde::{Deserialize, Serialize};

define_id!(PlanId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: PlanId,
    pub summary: String,
    pub steps: Vec<PlanStep>,
}

/// Which part of an approved plan's sequential execution is currently in
/// progress. Only meaningful once `PlanState.status == Approved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanProgress {
    /// Currently working the step at this index (0-based) into `plan.steps`.
    Step(usize),
    /// Every step has converged; the next completion with no tool calls is
    /// the genuine final answer.
    Finalizing,
}

/// Turn-scoped bookkeeping for the plan currently awaiting/holding approval,
/// analogous to `ToolCallState` for tool calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanState {
    pub id: ApprovalId,
    pub status: ApprovalStatus,
    pub plan: Plan,
    /// The id of the `create_plan` tool call that produced this plan, so the
    /// approval/rejection outcome can be reported back as a matching tool
    /// result message once the decision is known.
    pub tool_call_id: Option<String>,
    /// Which step (or finalization) execution is currently working on.
    pub progress: PlanProgress,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanApprovalRequest {
    pub id: ApprovalId,
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    pub agent_id: AgentId,
    pub plan: Plan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanApprovalResponse {
    pub id: ApprovalId,
    pub agent_id: AgentId,
}
