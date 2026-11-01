//! What a library is handed of a workspace (issue 251): the handle to its files (`Terminal`), its
//! reading part (`TerminalReader`), its commands (`Shell`), and the paths inside it
//! (`WorkspacePath`); how they are served is the kernel's (`terminal`).

use async_trait::async_trait;
use bytes::Bytes;
use caelumora_model::workspace::CommandStatus;
use chrono::{DateTime, Utc};
use futures::Stream;
use std::fmt::Debug;
use std::path::{Component, Path};
use std::pin::Pin;
use std::time::Duration;
use std::{fmt, io};

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
    fn a_path_knows_its_name_and_its_directory() {
        let path = WorkspacePath::parse("docs/report.md").unwrap();
        assert_eq!(path.file_name(), Some("report.md"));
        assert_eq!(path.parent().to_string(), "docs");
        assert_eq!(path.components(), ["docs", "report.md"]);
        assert_eq!(WorkspacePath::root().file_name(), None);
    }
}
