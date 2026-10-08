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

mod local;

pub(crate) use local::LocalProvider;

use async_trait::async_trait;
use bytes::Bytes;
use caelumora_model::workspace::WorkspaceId;
use chrono::{DateTime, Utc};
use futures::Stream;
use std::fmt::Debug;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

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
}

/// What makes and reaches the places of the workspaces of one storage
/// (issue 251, part A).
#[async_trait]
pub(crate) trait TerminalProvider: Send + Sync + Debug {
    /// Makes the place of the workspace `id`: empty.
    async fn create(&self, id: WorkspaceId) -> TerminalResult<()>;
    /// Removes the place of the workspace `id`, with everything in it;
    /// nothing there is no error.
    async fn remove(&self, id: WorkspaceId) -> TerminalResult<()>;
    /// The terminal of the workspace `id`.
    async fn terminal_for(&self, id: WorkspaceId) -> TerminalResult<Arc<dyn Terminal>>;
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
