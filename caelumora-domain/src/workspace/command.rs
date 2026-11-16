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
/// 586, 587): what the shell answers, what a call of a shell tool or a person's command gives - the
/// end of its output (`keeping_last`), and from a tool how to go on with one still running, which
/// is no part of it -, and what the REST API answers. The web UI
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

impl CommandState {
    /// The state with the last `max_bytes` of its output alone - cut from the front, past a
    /// character the cut falls inside -, `truncated` if anything was cut.
    pub fn keeping_last(mut self, max_bytes: usize) -> CommandState {
        if self.output.len() > max_bytes {
            let cut = self.output.ceil_char_boundary(self.output.len() - max_bytes);
            self.output.drain(..cut);
            self.truncated = true;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_end_of_an_output_is_kept_cut_past_a_character() {
        let state = CommandState {
            id: "x".to_string(),
            status: CommandStatus::Completed,
            exit_code: Some(0),
            output: "abcdéf".to_string(),
            next_offset: 7,
            truncated: false,
        };
        let kept = state.clone().keeping_last(2);
        assert_eq!(
            (kept.output.as_str(), kept.truncated),
            ("f", true),
            "the cut falls inside é and moves past it"
        );
        assert_eq!(state.clone().keeping_last(7), state, "all of it fits");
    }

    #[test]
    fn a_state_is_written_as_a_call_s_result_without_an_exit_code_it_has_not() {
        let running = CommandState {
            id: "x".to_string(),
            status: CommandStatus::Running,
            exit_code: None,
            output: "begun\n".to_string(),
            next_offset: 6,
            truncated: false,
        };
        assert_eq!(
            serde_json::to_value(&running).unwrap(),
            serde_json::json!({
                "id": "x",
                "status": "running",
                "output": "begun\n",
                "next_offset": 6,
                "truncated": false,
            })
        );
        let ended =
            CommandState { status: CommandStatus::Completed, exit_code: Some(1), ..running };
        assert_eq!(serde_json::to_value(&ended).unwrap()["exit_code"], 1);
    }
}
