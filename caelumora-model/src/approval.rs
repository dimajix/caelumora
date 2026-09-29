use crate::agent::AgentId;
use crate::channel::ChannelId;
use crate::session::SessionId;
use crate::tool::ToolCall;
use serde::{Deserialize, Serialize};

define_id!(ApproverId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approver {
    pub id: ApproverId,
    pub strategy: ApprovalStrategy,
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
