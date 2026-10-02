//! What Caelumora keeps of an external agent (issue 179): an agent that runs
//! elsewhere and takes part in a channel through an `ExternalAgentActor`.
//! We cannot look into it - its context, its tools, its memory are its own -
//! so what we keep is what passed between it and us, and the handles of the
//! protocol that tie the next exchange to the last one.

use crate::message::MessageId;
use crate::message_history::MessageHistory;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The remote agent's handle for its conversation with one channel - an
/// A2A `contextId`. Opaque: only the remote agent reads it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationId(pub String);

impl fmt::Display for ConversationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The remote agent's handle for one piece of work it was given - an A2A
/// task `id`. Opaque like `ConversationId`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub String);

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a task that has not ended stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenTaskState {
    /// The remote agent is working on it.
    Working,
    /// The remote agent asked something back; the next message to it
    /// continues the task.
    InputRequired,
}

/// A task given to the remote agent that has not ended: after a restart the
/// actor re-attaches to it, since it may have ended meanwhile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenTask {
    pub task: TaskId,
    /// When it was given.
    pub since: DateTime<Utc>,
    pub state: OpenTaskState,
}

/// An external agent in one channel, as its `ExternalAgentActor` keeps it.
/// Not an `AgentChannelState`: there is no turn, no transcript, no notes -
/// nothing of what we cannot see (issue 179). Stored under a key of its own.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExternalAgentChannelState {
    /// What was sent to the remote agent and what it answered - all we know
    /// of it (issue 06).
    pub history: MessageHistory,
    /// The remote agent's conversation with this channel, once it opened
    /// one.
    pub conversation: Option<ConversationId>,
    /// The tasks given to it that have not ended.
    pub open_tasks: Vec<OpenTask>,
    /// The last message of the channel's history the remote agent was given:
    /// the next request catches it up on what was said after it. `None`
    /// before the first request.
    pub caught_up_to: Option<MessageId>,
}
