//! How a workspace is reached (issue 251, part A): every workspace lies in
//! a storage, served by one `TerminalProvider`, which makes and removes the
//! place of a workspace there and hands out its `Terminal` - the one handle
//! through which its files are read and written. Whatever works on a
//! workspace - the panel, uploads and downloads, the tools of agents -
//! holds that handle; nothing else reaches the files.
//!
//! What a caller writes as a path is made a `WorkspacePath` before any
//! provider sees it: relative, without `..`, so that no provider is ever
//! handed one that leaves the workspace. An error never names more of a
//! path than the caller wrote.
//!
//! A terminal may have a `Shell`, a capability of the storage: commands run
//! in the workspace's directory, each seen only under the scope it was
//! started with - a channel - and kept a while after it ended. A storage
//! knows its commands (`commands.rs`, issue 254): its provider lists those
//! that may still run and ends one when told, and none runs longer than the
//! storage's `max_run`.

mod commands;
mod local;
mod local_shell;
mod open_terminal;

pub(crate) use commands::CommandKey;
pub(crate) use local::{write_whole, LocalProvider, WriteFailure};
pub(crate) use open_terminal::{DockerProvider, SharedProvider};

use async_trait::async_trait;
use bytes::Bytes;
use caelumora_model::confidentiality::Classification;
use caelumora_model::workspace::WorkspaceId;
use chrono::{DateTime, Utc};
use futures::Stream;
use std::fmt::Debug;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// The content of a file, read or to be written, in pieces.
pub type ByteStream = Pin<Box<dyn Stream<Item = std::io::Result<Bytes>> + Send>>;

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
                Component::Normal(part) => components.push(part.to_str().ok_or_else(invalid)?.to_string()),
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => return Err(invalid()),
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

    /// It below `base`, a directory of the host.
    pub(crate) fn under(&self, base: &Path) -> PathBuf {
        self.components.iter().fold(base.to_path_buf(), |path, part| path.join(part))
    }
}

impl std::fmt::Display for WorkspacePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

/// The one handle to a workspace's files (issue 251): every path is one
/// inside the workspace, and what a path names that is not there is
/// `NotFound`.
#[async_trait]
pub trait Terminal: Send + Sync + Debug {
    /// The entries of the directory `dir`, by name.
    async fn list(&self, dir: &WorkspacePath) -> TerminalResult<Vec<FileEntry>>;
    /// What `path` names.
    async fn stat(&self, path: &WorkspacePath) -> TerminalResult<FileEntry>;
    /// The content of the file `path`, with its size.
    async fn read(&self, path: &WorkspacePath) -> TerminalResult<(u64, ByteStream)>;
    /// Writes `content` to the file `path`, replacing what was there, and
    /// makes the directories it lies in; at most `limit` bytes, or nothing
    /// is written. Answers how many bytes were written.
    async fn write(&self, path: &WorkspacePath, content: ByteStream, limit: u64) -> TerminalResult<u64>;
    /// Removes the file `path`, or the directory if it is empty.
    async fn delete(&self, path: &WorkspacePath) -> TerminalResult<()>;
    /// Makes the directory `path`, and those it lies in.
    async fn mkdir(&self, path: &WorkspacePath) -> TerminalResult<()>;
    /// Moves `from` to `to`, which must not exist.
    async fn move_entry(&self, from: &WorkspacePath, to: &WorkspacePath) -> TerminalResult<()>;
    /// Its shell as `scope` sees it - the commands started under that scope
    /// alone (issue 296) -, if its storage has one.
    fn shell(&self, scope: &str) -> Option<Arc<dyn Shell>>;
}

/// Where a command stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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

/// What a command printed since an offset, and where it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandState {
    pub id: String,
    pub status: CommandStatus,
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

/// Commands in a workspace (issue 251) as one scope sees them: started at
/// once, followed by what they print, killed with everything they started.
/// A shell is bound to its scope when it is made (`Terminal::shell`, issue
/// 296) and sees the commands started under it alone; for a storage a
/// scope is any text.
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
}

/// What makes and reaches the places of the workspaces of one storage
/// (issue 251, part A).
#[async_trait]
pub(crate) trait TerminalProvider: Send + Sync + Debug {
    /// Makes the place of the workspace `id`: empty.
    async fn create(&self, id: WorkspaceId) -> TerminalResult<()>;
    /// Removes the place of the workspace `id`, with everything in it - the
    /// commands running there ended first (issue 254); nothing there is no
    /// error.
    async fn remove(&self, id: WorkspaceId) -> TerminalResult<()>;
    /// The terminal of the workspace `id`.
    async fn terminal_for(&self, id: WorkspaceId) -> TerminalResult<Arc<dyn Terminal>>;
    /// Whether its terminals have a shell - asked without reaching one.
    fn has_shell(&self) -> bool;
    /// The commands of its workspaces that may still run (issue 254) -
    /// listed without reaching any place.
    fn commands(&self) -> Vec<CommandKey>;
    /// Ends command `key` with everything it started, without asking
    /// anything (issue 254): for what the runtime decided may no longer
    /// run. One that ended meanwhile, or is no longer known, is no error;
    /// a place that is not running is not started for it.
    ///
    /// # Errors
    /// A `TerminalError` of reaching its place.
    async fn end_command(&self, key: &CommandKey) -> TerminalResult<()>;
    /// Ends the commands of workspace `id` that may still run, as
    /// `end_command` ends one: what runs in a place goes before the place.
    ///
    /// # Errors
    /// A `TerminalError` of reaching its place.
    async fn end_commands_of(&self, id: WorkspaceId) -> TerminalResult<()> {
        for key in self.commands().into_iter().filter(|(workspace, _, _)| *workspace == id) {
            self.end_command(&key).await?;
        }
        Ok(())
    }
    /// What may be put into this storage: the classification a workspace
    /// in it may have at most; `None` for no bound (issue 251, part B2).
    fn destination(&self) -> Option<&Classification>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_one_inside_the_workspace_or_none() {
        assert_eq!(WorkspacePath::parse("a/b").unwrap().to_string(), "a/b");
        assert_eq!(WorkspacePath::parse("./a//b/").unwrap(), WorkspacePath::parse("a/b").unwrap(), "normalised");
        assert!(WorkspacePath::parse("").unwrap().is_root());
        assert!(WorkspacePath::parse(".").unwrap().is_root());
        for refused in ["/etc/passwd", "../x", "a/../../x", "a/..", "a\0b"] {
            let error = WorkspacePath::parse(refused).expect_err(refused);
            assert!(matches!(&error, TerminalError::InvalidPath(written) if written == refused), "{refused:?}: {error}");
        }
    }

    #[test]
    fn a_path_knows_its_name_and_its_directory() {
        let path = WorkspacePath::parse("docs/report.md").unwrap();
        assert_eq!(path.file_name(), Some("report.md"));
        assert_eq!(path.parent().to_string(), "docs");
        assert_eq!(path.under(Path::new("/srv/ws")), PathBuf::from("/srv/ws/docs/report.md"));
        assert_eq!(WorkspacePath::root().file_name(), None);
    }
}
