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

impl Plan {
    /// How many steps a plan may have, those it is extended by included
    /// (issue 412): each step may call the model as often as a turn, so the
    /// steps bound what a plan may cost.
    pub const MAX_STEPS: usize = 20;
}

/// Which part of an approved plan's sequential execution is currently in
/// progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanProgress {
    /// Currently working the step at this index (0-based) into `plan.steps`.
    Step(usize),
    /// Every step has converged; the next completion with no tool calls is
    /// the genuine final answer.
    Finalizing,
}

/// Turn-scoped bookkeeping for the plan currently executing. Only ever
/// constructed after the `create_plan` tool call that produced it has
/// already been through the generic tool-call approval gate, so its mere
/// existence implies "approved" - there is no separate pending/approved
/// status to track here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanState {
    pub plan: Plan,
    /// Which step (or finalization) execution is currently working on.
    pub progress: PlanProgress,
}
