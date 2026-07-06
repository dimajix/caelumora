use crate::agent::AgentId;
use crate::channel::ChannelId;
use crate::message::ToolCall;
use crate::session::SessionId;
use serde::{Deserialize, Serialize};

define_id!(ApprovalId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: ApprovalId,
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    pub agent_id: AgentId,
    pub tool_call: ToolCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResponse {
    pub id: ApprovalId,
    pub status: ApprovalStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApprovalStatus {
    Approved,
    Denied,
}
