//! Where a command of a workspace's shell stands (`CommandStatus`), and what it printed since an
//! offset (`CommandState`) - as the shell answers it, a shell tool gives it its model and the REST
//! API answers it.

use serde::{Deserialize, Serialize};

/// Where a command of a workspace's shell stands (issue 294).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Running,
    /// Ended by itself, with its exit code if it has one.
    Completed,
    /// Ended because it was killed.
    Killed,
    /// Could not be run, or ended by a signal it was not sent.
    Failed,
}

/// What a command of a workspace's shell printed since an offset, and where it stands (issues 294,
/// 586): what the shell answers, what a call of a shell tool gives its model - with how to go on
/// with one still running, which is no part of it -, and what the REST API answers. The web UI
/// reads a shell tool's result as one, in `commandState` (`caelumora-web-ui/src/messages.ts`),
/// and tells an exit code not given by its key being absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandState {
    /// The command, by which it is followed and killed.
    pub id: String,
    pub status: CommandStatus,
    /// Its exit code, once it ended with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// What it printed - standard output and error in the order they came -
    /// since the offset asked for, as text.
    pub output: String,
    /// Where to ask from next time.
    pub next_offset: u64,
    /// Whether output before the offset asked for, or between it and what
    /// is given, was dropped: more than is kept.
    pub truncated: bool,
}
