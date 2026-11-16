//! What a library is handed of a workspace (issue 251): the handle to its files (`Terminal`), its
//! reading part (`TerminalReader`), its commands (`Shell`) and how running one is called, shown and
//! answered, and the paths inside it (`WorkspacePath`); how they are served is the kernel's
//! (`caelumora_kernel::terminal`).
//!
//! A command is run by a call of `RUN_COMMAND`, shown as `SHELL_PRESENTATION`, answered by its
//! `CommandState` with the end of its output (`CommandState::keeping_last`), and killed if the call
//! is dropped while it waits (`KillOnDrop`).

use async_trait::async_trait;
use bytes::Bytes;
use caelumora_domain::tool::ToolPresentation;
use caelumora_domain::workspace::CommandStatus;
use chrono::{DateTime, Utc};
use futures::Stream;
use serde::Serialize;
use std::fmt::Debug;
use std::path::{Component, Path};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use std::{fmt, io, mem};
use tokio::runtime::Handle;
use tokio::time;
use tracing::warn;

/// The name of the call that runs a command in a workspace (issue 251).
pub const RUN_COMMAND: &str = "run_command";

/// How a shell command is shown (issue 251): its `command` as a line of code, its `output` as a
/// terminal shows it, and its exit code.
pub const SHELL_PRESENTATION: ToolPresentation = ToolPresentation::new("shell");

/// How long the kill of a dropped call's command may take.
const KILL_WITHIN: Duration = Duration::from_secs(5);

/// The content of a file, read or to be written, in pieces.
pub type ByteStream = Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send>>;

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    /// A path that is not one inside a workspace, as the caller wrote it.
    #[error("'{0}' is not a path inside the workspace")]
    InvalidPath(String),
    #[error("'{0}' does not exist in the workspace")]
    NotFound(String),
    #[error("'{0}' exists already in the workspace")]
    Exists(String),
    #[error("'{0}' is a directory")]
    IsADirectory(String),
    #[error("'{0}' is not a directory")]
    NotADirectory(String),
    #[error("the directory '{0}' is not empty")]
    NotEmpty(String),
    #[error("the file is larger than {limit} bytes")]
    TooLarge { limit: u64 },
    /// The place of the workspace could not be used: what failed, never
    /// where on the host.
    #[error("the workspace's storage failed: {0}")]
    Unavailable(String),
    /// A command not known under the scope it was asked for - never
    /// started there, or forgotten a while after it ended.
    #[error("no command '{0}' is known here: it was never started here, or ended a while ago and its output was given then")]
    UnknownCommand(String),
}

pub type TerminalResult<T> = Result<T, TerminalError>;

/// A path inside a workspace, as a caller wrote it, normalised: relative,
/// of ordinary components only. The empty path is the workspace itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspacePath {
    components: Vec<String>,
}

impl WorkspacePath {
    /// `text` as a path inside a workspace: `a/b`, `./a/b/` and `a//b` are
    /// the same; a leading `/`, a `..`, a drive or a NUL byte is refused.
    ///
    /// # Errors
    /// `TerminalError::InvalidPath`, naming `text`.
    pub fn parse(text: &str) -> TerminalResult<Self> {
        let invalid = || TerminalError::InvalidPath(text.to_string());
        if text.contains('\0') {
            return Err(invalid());
        }
        let mut components = Vec::new();
        for component in Path::new(text).components() {
            match component {
                Component::Normal(part) => {
                    components.push(part.to_str().ok_or_else(invalid)?.to_string())
                }
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(invalid())
                }
            }
        }
        Ok(Self { components })
    }

    /// The workspace itself.
    pub fn root() -> Self {
        Self::default()
    }

    /// Whether it is the workspace itself.
    pub fn is_root(&self) -> bool {
        self.components.is_empty()
    }

    /// Its last component - the name of what it names - `None` for the
    /// workspace itself.
    pub fn file_name(&self) -> Option<&str> {
        self.components.last().map(String::as_str)
    }

    /// The directory it lies in; the workspace itself for one at its top.
    pub fn parent(&self) -> Self {
        let mut components = self.components.clone();
        components.pop();
        Self { components }
    }

    /// Its components, in order: from the top of the workspace down to what it names.
    pub fn components(&self) -> &[String] {
        &self.components
    }
}

impl fmt::Display for WorkspacePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.components.join("/"))
    }
}

/// What a directory of a workspace holds, one entry at a time.
#[derive(Debug, Clone, PartialEq)]
pub struct FileEntry {
    /// Its name in its directory; one that is no UTF-8 is shown lossily
    /// and cannot be opened by its name.
    pub name: String,
    pub is_dir: bool,
    /// Its size in bytes; 0 for a directory.
    pub size: u64,
    pub modified: Option<DateTime<Utc>>,
}

/// What of a workspace's files can be read (issue 516): listed, looked at, read - every path one
/// inside the workspace, and what a path names that is not there `NotFound`. What a work that only
/// reads is handed: it can change nothing.
#[async_trait]
pub trait TerminalReader: Send + Sync + Debug {
    /// The entries of the directory `dir`, by name.
    async fn list(&self, dir: &WorkspacePath) -> TerminalResult<Vec<FileEntry>>;
    /// What `path` names.
    async fn stat(&self, path: &WorkspacePath) -> TerminalResult<FileEntry>;
    /// The content of the file `path`, with its size.
    async fn read(&self, path: &WorkspacePath) -> TerminalResult<(u64, ByteStream)>;
    /// The largest file the workspace's storage takes (its `max_file_bytes`): what its `Terminal`
    /// writes beyond it is refused.
    fn max_file_bytes(&self) -> u64;
}

/// The one handle to a workspace's files (issue 251): what it reads, and what changes them. Its
/// shell is not its: a provider hands it out, for a scope (`TerminalProvider::shell_for`).
#[async_trait]
pub trait Terminal: TerminalReader {
    /// Writes `content` to the file `path`, replacing what was there, and makes the directories it
    /// lies in; at most `max_file_bytes`, or nothing is written. Answers how many bytes were
    /// written.
    async fn write(&self, path: &WorkspacePath, content: ByteStream) -> TerminalResult<u64>;
    /// Removes the file `path`, or the directory if it is empty.
    async fn delete(&self, path: &WorkspacePath) -> TerminalResult<()>;
    /// Makes the directory `path`, and those it lies in.
    async fn mkdir(&self, path: &WorkspacePath) -> TerminalResult<()>;
    /// Moves `from` to `to`, which must not exist.
    async fn move_entry(&self, from: &WorkspacePath, to: &WorkspacePath) -> TerminalResult<()>;
}

/// What a command printed since an offset, and where it stands - written as a call's result gives
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandState {
    pub id: String,
    pub status: CommandStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
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

/// The command `id` of `shell`, killed if this is dropped before `disarm`: a call dropped while it
/// waits for its command - its turn cancelled or stopped - kills the command, in a task of
/// its own bounded by `KILL_WITHIN`, which the drop does not await. A command left running would go
/// on writing into the workspace after the call that started it.
pub struct KillOnDrop {
    shell: Arc<dyn Shell>,
    id: String,
    armed: bool,
}

impl KillOnDrop {
    /// Kills the command `id` of `shell` if dropped before `disarm`.
    pub fn new(shell: Arc<dyn Shell>, id: String) -> Self {
        Self { shell, id, armed: true }
    }

    /// Leaves the command running once this is dropped.
    pub fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(runtime) = Handle::try_current() else { return };
        let (shell, id) = (Arc::clone(&self.shell), mem::take(&mut self.id));
        runtime.spawn(async move {
            match time::timeout(KILL_WITHIN, shell.kill(&id, 0)).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => warn!(command = %id, "The command of an interrupted call could not be killed: {e}"),
                Err(_) => warn!(command = %id, "The command of an interrupted call was not killed in time"),
            }
        });
    }
}

/// Commands in a workspace (issue 251) as one scope sees them: started at once, followed by what
/// they print, killed with everything they started. A shell is bound to its scope when it is made
/// (`TerminalProvider::shell_for`, issue 296) and sees the commands started under it alone; for a
/// storage a scope is any text.
#[async_trait]
pub trait Shell: Send + Sync + Debug {
    /// Starts `command` in the workspace's directory; answers its id at once.
    async fn start(&self, command: &str) -> TerminalResult<String>;
    /// What command `id` printed since `offset`, once it ended or `wait` is
    /// over, whichever is first.
    async fn output(&self, id: &str, offset: u64, wait: Duration) -> TerminalResult<CommandState>;
    /// Ends command `id` and what it started; what it printed since
    /// `offset`.
    async fn kill(&self, id: &str, offset: u64) -> TerminalResult<CommandState>;

    /// Ends command `id` with everything it started, as a provider ends one
    /// when told (`TerminalProvider::end_command`): one that ended
    /// meanwhile, or is no longer known, is no error.
    ///
    /// # Errors
    /// A `TerminalError` of reaching its place.
    async fn end(&self, id: &str) -> TerminalResult<()> {
        match self.kill(id, 0).await {
            Ok(_) | Err(TerminalError::UnknownCommand(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_one_inside_the_workspace_or_none() {
        assert_eq!(WorkspacePath::parse("a/b").unwrap().to_string(), "a/b");
        assert_eq!(
            WorkspacePath::parse("./a//b/").unwrap(),
            WorkspacePath::parse("a/b").unwrap(),
            "normalised"
        );
        assert!(WorkspacePath::parse("").unwrap().is_root());
        assert!(WorkspacePath::parse(".").unwrap().is_root());
        for refused in ["/etc/passwd", "../x", "a/../../x", "a/..", "a\0b"] {
            let error = WorkspacePath::parse(refused).expect_err(refused);
            assert!(
                matches!(&error, TerminalError::InvalidPath(written) if written == refused),
                "{refused:?}: {error}"
            );
        }
    }

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

    #[test]
    fn a_path_knows_its_name_and_its_directory() {
        let path = WorkspacePath::parse("docs/report.md").unwrap();
        assert_eq!(path.file_name(), Some("report.md"));
        assert_eq!(path.parent().to_string(), "docs");
        assert_eq!(path.components(), ["docs", "report.md"]);
        assert_eq!(WorkspacePath::root().file_name(), None);
    }
}
