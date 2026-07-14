use crate::agent::AgentId;
use crate::channel::ChannelId;
use crate::session::SessionId;
use crate::tool::ToolCall;
use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

define_id!(ApproverId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approver {
    pub id: ApproverId,
    pub strategy: ApprovalStrategy,
}

#[derive(Debug)]
pub struct ApproverController {
    approver: ArcSwap<Approver>,
}

impl ApproverController {
    pub fn new(approver: Approver) -> Self {
        Self {
            approver: ArcSwap::from_pointee(approver),
        }
    }

    pub fn load(&self) -> arc_swap::Guard<Arc<Approver>> {
        self.approver.load()
    }

    pub fn id(&self) -> ApproverId {
        self.approver.load().id
    }

    pub fn strategy(&self) -> ApprovalStrategy {
        self.approver.load().strategy
    }
}

define_id!(ApprovalId);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApprovalStrategy {
    Any,
    Majority,
    All,
    DenyOnFirst,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: ApprovalId,
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    pub agent_id: AgentId,
    pub tool_call: ToolCall,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResponse {
    pub id: ApprovalId,
    pub agent_id: AgentId,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Timeout,
}
