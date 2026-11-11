//! The `local` provider (issue 251, part A): a directory per workspace under a root on Caelumora's
//! host, read and written by Caelumora itself - or, as the storage `directory` (issue 554), every
//! workspace in the one directory a program of one person was started in. A symbolic link inside a
//! workspace is followed only where its target stays inside: it is checked when a path is used, so
//! a link made between the check and the use - by a command running in the workspace - is the one
//! gap, closed only by a provider that keeps commands elsewhere.

use super::local_shell::{Environment, LocalCommands, LocalShell};
use super::{
    check_max_file_bytes, check_max_run_secs, default_max_file_bytes, default_max_run_secs,
    CommandKey, TerminalProvider,
};
use crate::backend::Backends;
use async_trait::async_trait;
use caelumora_api::component::{ComponentError, ComponentResult, Params};
use caelumora_api::terminal::{
    ByteStream, FileEntry, Shell, Terminal, TerminalError, TerminalReader, TerminalResult,
    WorkspacePath,
};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::workspace::WorkspaceId;
use futures::future::join_all;
use futures::StreamExt;
use serde::Deserialize;
use std::fs::{self as std_fs, Metadata};
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use tracing::warn;
use uuid::Uuid;

/// The parameters of `local`: a directory per workspace under `root`, on
/// Caelumora's host, read and written by Caelumora itself. With `shell`,
/// agents run commands there - on Caelumora's host, as its user: for
/// development and tests, never where the host is to be protected.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalStorageConfig {
    root: PathBuf,
    #[serde(default)]
    shell: bool,
    /// What may be put into this storage (issue 251): the classification a
    /// workspace in it may have at most. Without it, `public` where it has
    /// a shell - a command may send anything anywhere - and no bound where
    /// it has none, since nothing then leaves the process.
    #[serde(default)]
    destination: Option<Classification>,
    /// How long a command may run (issue 254): killed, with what it
    /// started, once it ran so long. Of use only with `shell`.
    #[serde(default = "default_max_run_secs")]
    max_run_secs: u64,
    /// The largest file a person may put into a workspace of it.
    #[serde(default = "default_max_file_bytes")]
    max_file_bytes: u64,
}

impl Params for LocalStorageConfig {
    /// # Errors
    /// `InvalidConfig` if `max_run_secs` is no time a command may run, or `max_file_bytes` no size
    /// a file may have.
    fn check(&self) -> ComponentResult<()> {
        check_max_run_secs(self.max_run_secs)?;
        check_max_file_bytes(self.max_file_bytes)
    }
}

/// The parameters of `directory`: every workspace in `dir` - the directory a program of one person
/// was started in -, named in `root` by a link to it, so that it is found there after the program
/// was started elsewhere. Removing a workspace never removes `dir`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectoryStorageConfig {
    /// Where the links that name the workspaces lie: a directory of this storage's own.
    root: PathBuf,
    /// The directory every workspace made from now on lies in, as it is when the kernel is built:
    /// a workspace made before keeps the one its link names.
    dir: PathBuf,
    /// Whether agents run commands there, as its user, on Caelumora's host.
    #[serde(default)]
    shell: bool,
    /// What may be put into this storage, named, since the one where it is left out, `public` with
    /// a shell, is not what a person's own directory is for.
    destination: Classification,
    /// What a command is given of the program's environment: nothing of it unless named.
    #[serde(default)]
    environment: Environment,
    /// How long a command may run (issue 254). Of use only with `shell`.
    #[serde(default = "default_max_run_secs")]
    max_run_secs: u64,
    /// The largest file a person may put into a workspace of it.
    #[serde(default = "default_max_file_bytes")]
    max_file_bytes: u64,
}

impl Params for DirectoryStorageConfig {
    /// # Errors
    /// As `LocalStorageConfig::check`.
    fn check(&self) -> ComponentResult<()> {
        check_max_run_secs(self.max_run_secs)?;
        check_max_file_bytes(self.max_file_bytes)
    }
}

/// Registers the storages `local` and `directory` with `backends`.
pub(super) fn register_backends(backends: &mut Backends) {
    backends.register::<dyn TerminalProvider, _, _>("local", |_, config: LocalStorageConfig, _| {
        Ok(Arc::new(LocalProvider::new(
            config.root,
            config.shell,
            Duration::from_secs(config.max_run_secs),
            config.max_file_bytes,
            config.destination,
        )))
    });
    backends.register::<dyn TerminalProvider, _, _>(
        "directory",
        |_, config: DirectoryStorageConfig, _| {
            let dir = std_fs::canonicalize(&config.dir).map_err(|e| {
                ComponentError::InvalidConfig(format!("dir cannot be reached: {}", e.kind()))
            })?;
            if !dir.is_dir() {
                return Err(ComponentError::InvalidConfig("dir is no directory".to_string()));
            }
            let provider = LocalProvider::new(
                config.root,
                config.shell,
                Duration::from_secs(config.max_run_secs),
                config.max_file_bytes,
                Some(config.destination),
            );
            Ok(Arc::new(provider.sharing(dir, config.environment)))
        },
    );
}

/// Where the workspaces of a storage lie.
#[derive(Debug)]
enum Placement {
    /// Each in a directory of its own below the root, made and removed with it.
    Own,
    /// All in this directory, each named below the root by a link to it, which is made and removed
    /// with it; the directory never.
    Shared(PathBuf),
}

#[derive(Debug)]
pub(crate) struct LocalProvider {
    root: PathBuf,
    placement: Placement,
    /// What a command of its shells is given of the environment.
    environment: Environment,
    /// The commands of its workspaces, if it has a shell.
    commands: Option<Arc<LocalCommands>>,
    /// How long a command may run.
    max_run: Duration,
    /// The largest file a workspace of it takes.
    max_file_bytes: u64,
    destination: Option<Classification>,
}

impl LocalProvider {
    /// The provider of a storage whose workspaces lie under `root`, with a shell if `shell`, its
    /// commands running for `max_run` at most and its files of `max_file_bytes` at most; what may
    /// be put into it at most `destination`, or `public` with a shell and no bound without one.
    pub(crate) fn new(
        root: PathBuf,
        shell: bool,
        max_run: Duration,
        max_file_bytes: u64,
        destination: Option<Classification>,
    ) -> Self {
        let destination = destination.or_else(|| shell.then_some(Classification::PUBLIC));
        Self {
            root,
            placement: Placement::Own,
            environment: Environment::Isolated,
            commands: shell.then(Arc::default),
            max_run,
            max_file_bytes,
            destination,
        }
    }

    /// The provider as one whose workspaces all lie in `dir`, its commands given `environment`.
    fn sharing(self, dir: PathBuf, environment: Environment) -> Self {
        Self { placement: Placement::Shared(dir), environment, ..self }
    }

    fn place(&self, id: WorkspaceId) -> PathBuf {
        self.root.join(id.to_string())
    }

    /// The directory of workspace `id`, if it is there - for one in a shared directory that
    /// directory, as it is, never the link that names it, so that what a command or a model is
    /// shown is where it works; a place gone is logged.
    async fn found(&self, id: WorkspaceId) -> TerminalResult<PathBuf> {
        let dir = self.place(id);
        let dir = match self.placement {
            Placement::Own => dir,
            // One gone is told as below, by what its link names.
            Placement::Shared(_) => fs::canonicalize(&dir).await.unwrap_or(dir),
        };
        match fs::metadata(&dir).await {
            Ok(metadata) if metadata.is_dir() => Ok(dir),
            Ok(_) => Err(TerminalError::Unavailable("its place is not a directory".to_string())),
            Err(e) if e.kind() == ErrorKind::NotFound => {
                warn!(workspace = %id, "The place of a workspace is gone");
                Err(TerminalError::Unavailable("its place is gone".to_string()))
            }
            Err(e) => Err(unavailable(e)),
        }
    }
}

/// What failed with `error` at `path`, as the caller wrote it: never where
/// on the host.
fn failed(path: &WorkspacePath, error: Error) -> TerminalError {
    let shown = path.to_string();
    match error.kind() {
        ErrorKind::NotFound => TerminalError::NotFound(shown),
        ErrorKind::AlreadyExists => TerminalError::Exists(shown),
        ErrorKind::DirectoryNotEmpty => TerminalError::NotEmpty(shown),
        ErrorKind::IsADirectory => TerminalError::IsADirectory(shown),
        ErrorKind::NotADirectory => TerminalError::NotADirectory(shown),
        kind => TerminalError::Unavailable(kind.to_string()),
    }
}

/// What failed with `error` at the place of a workspace itself.
fn unavailable(error: Error) -> TerminalError {
    TerminalError::Unavailable(error.kind().to_string())
}

#[async_trait]
impl TerminalProvider for LocalProvider {
    async fn create(&self, id: WorkspaceId) -> TerminalResult<()> {
        match &self.placement {
            Placement::Own => fs::create_dir_all(self.place(id)).await.map_err(unavailable),
            Placement::Shared(dir) => {
                fs::create_dir_all(&self.root).await.map_err(unavailable)?;
                fs::symlink(dir, self.place(id)).await.map_err(unavailable)
            }
        }
    }

    /// Of a shared directory the link alone.
    async fn remove(&self, id: WorkspaceId) -> TerminalResult<()> {
        // Nothing of it goes on writing into what is being removed.
        self.end_commands_of(id).await?;
        let removed = match self.placement {
            Placement::Own => fs::remove_dir_all(self.place(id)).await,
            Placement::Shared(_) => fs::remove_file(self.place(id)).await,
        };
        match removed {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(unavailable(e)),
        }
    }

    async fn terminal_for(&self, id: WorkspaceId) -> TerminalResult<Arc<dyn Terminal>> {
        let dir = self.found(id).await?;
        Ok(Arc::new(LocalTerminal { dir, max_file_bytes: self.max_file_bytes }))
    }

    async fn shell_for(
        &self,
        id: WorkspaceId,
        scope: &str,
    ) -> TerminalResult<Option<Arc<dyn Shell>>> {
        let Some(commands) = self.commands.as_ref() else { return Ok(None) };
        Ok(Some(Arc::new(LocalShell {
            dir: self.found(id).await?,
            workspace: id,
            scope: scope.to_string(),
            commands: Arc::clone(commands),
            max_run: self.max_run,
            environment: self.environment,
        })))
    }

    fn has_shell(&self) -> bool {
        self.commands.is_some()
    }

    fn commands(&self) -> Vec<CommandKey> {
        self.commands.as_ref().map(|commands| commands.running()).unwrap_or_default()
    }

    async fn end_command(&self, (workspace, scope, id): &CommandKey) -> TerminalResult<()> {
        let Some(commands) = &self.commands else { return Ok(()) };
        let shell = LocalShell {
            dir: self.place(*workspace),
            workspace: *workspace,
            scope: scope.clone(),
            commands: Arc::clone(commands),
            max_run: self.max_run,
            environment: self.environment,
        };
        shell.end(id).await
    }

    /// Ends every command of its shells, all at once: their processes
    /// would outlive the process unknown to anybody.
    async fn shutdown(&self) {
        join_all(self.commands().into_iter().map(|key| async move {
            if let Err(e) = self.end_command(&key).await {
                warn!(workspace = %key.0, command = %key.2, "A command could not be ended as Caelumora ends: {e}");
            }
        })).await;
    }

    fn destination(&self) -> Option<&Classification> {
        self.destination.as_ref()
    }
}

/// The files of one workspace, in its directory, and its shell if the
/// storage has one.
#[derive(Debug)]
struct LocalTerminal {
    dir: PathBuf,
    max_file_bytes: u64,
}

impl LocalTerminal {
    /// Where `path` lies on the host, if that is inside the workspace: what
    /// of it exists, links followed, must lie in the workspace's directory.
    async fn inside(&self, path: &WorkspacePath) -> TerminalResult<PathBuf> {
        let full = on_host(&self.dir, path);
        let base = fs::canonicalize(&self.dir).await.map_err(unavailable)?;
        let outside = || TerminalError::InvalidPath(path.to_string());
        // A link that leads nowhere would be written through: refused.
        if let Ok(metadata) = fs::symlink_metadata(&full).await {
            if metadata.is_symlink() && fs::canonicalize(&full).await.is_err() {
                return Err(outside());
            }
        }
        let mut probe = full.clone();
        loop {
            match fs::canonicalize(&probe).await {
                Ok(real) => return if real.starts_with(&base) { Ok(full) } else { Err(outside()) },
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    if !probe.pop() || !probe.starts_with(&self.dir) {
                        return Ok(full);
                    }
                }
                Err(e) => return Err(failed(path, e)),
            }
        }
    }
}

fn entry(name: String, metadata: &Metadata) -> FileEntry {
    FileEntry {
        name,
        is_dir: metadata.is_dir(),
        size: if metadata.is_dir() { 0 } else { metadata.len() },
        modified: metadata.modified().ok().map(Into::into),
    }
}

/// A file beside `target` to write into before it is renamed over it, so
/// that a write that breaks off leaves the file as it was.
fn partial(target: &Path) -> PathBuf {
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    target.with_file_name(format!(".{name}.{}.part", Uuid::new_v4()))
}

/// Why writing a file whole failed (`write_whole`).
#[derive(Debug)]
pub enum WriteFailure {
    /// More than the limit came.
    TooLarge,
    /// What was to be written broke off.
    BrokeOff(ErrorKind),
    /// The file could not be written.
    Io(Error),
}

/// Writes `content` to `target` whole, at most `limit` bytes, or not at
/// all: into a file beside it, renamed over it once complete. The directory
/// it lies in must exist. Answers how many bytes were written. Shared by
/// the files of a local workspace and the local storage of content (issue
/// 282).
pub async fn write_whole(
    target: &Path,
    mut content: ByteStream,
    limit: u64,
) -> Result<u64, WriteFailure> {
    let part = partial(target);
    let written = async {
        let mut file = fs::File::create(&part).await.map_err(WriteFailure::Io)?;
        let mut written: u64 = 0;
        while let Some(chunk) = content.next().await {
            let chunk = chunk.map_err(|e| WriteFailure::BrokeOff(e.kind()))?;
            written += chunk.len() as u64;
            if written > limit {
                return Err(WriteFailure::TooLarge);
            }
            file.write_all(&chunk).await.map_err(WriteFailure::Io)?;
        }
        file.flush().await.map_err(WriteFailure::Io)?;
        Ok(written)
    }
    .await;
    let renamed = match written {
        Ok(written) => fs::rename(&part, target).await.map(|()| written).map_err(WriteFailure::Io),
        Err(e) => Err(e),
    };
    if renamed.is_err() {
        let _ = fs::remove_file(&part).await;
    }
    renamed
}

#[async_trait]
impl TerminalReader for LocalTerminal {
    async fn list(&self, dir: &WorkspacePath) -> TerminalResult<Vec<FileEntry>> {
        let full = self.inside(dir).await?;
        let mut entries = fs::read_dir(&full).await.map_err(|e| failed(dir, e))?;
        let mut listed = Vec::new();
        while let Some(found) = entries.next_entry().await.map_err(|e| failed(dir, e))? {
            let metadata = found.metadata().await.map_err(|e| failed(dir, e))?;
            listed.push(entry(found.file_name().to_string_lossy().into_owned(), &metadata));
        }
        listed.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(listed)
    }

    async fn stat(&self, path: &WorkspacePath) -> TerminalResult<FileEntry> {
        let full = self.inside(path).await?;
        let metadata = fs::metadata(&full).await.map_err(|e| failed(path, e))?;
        Ok(entry(path.file_name().unwrap_or_default().to_string(), &metadata))
    }

    async fn read(&self, path: &WorkspacePath) -> TerminalResult<(u64, ByteStream)> {
        let full = self.inside(path).await?;
        let metadata = fs::metadata(&full).await.map_err(|e| failed(path, e))?;
        if metadata.is_dir() {
            return Err(TerminalError::IsADirectory(path.to_string()));
        }
        let file = fs::File::open(&full).await.map_err(|e| failed(path, e))?;
        Ok((metadata.len(), Box::pin(ReaderStream::new(file))))
    }

    fn max_file_bytes(&self) -> u64 {
        self.max_file_bytes
    }
}

#[async_trait]
impl Terminal for LocalTerminal {
    async fn write(&self, path: &WorkspacePath, content: ByteStream) -> TerminalResult<u64> {
        let limit = self.max_file_bytes;
        if path.is_root() {
            return Err(TerminalError::IsADirectory(path.to_string()));
        }
        let full = self.inside(path).await?;
        if fs::metadata(&full).await.is_ok_and(|m| m.is_dir()) {
            return Err(TerminalError::IsADirectory(path.to_string()));
        }
        let parent = path.parent();
        fs::create_dir_all(on_host(&self.dir, &parent)).await.map_err(|e| failed(&parent, e))?;
        write_whole(&full, content, limit).await.map_err(|failure| match failure {
            WriteFailure::TooLarge => TerminalError::TooLarge { limit },
            WriteFailure::BrokeOff(kind) => {
                TerminalError::Unavailable(format!("what was to be written broke off: {kind}"))
            }
            WriteFailure::Io(e) => failed(path, e),
        })
    }

    async fn delete(&self, path: &WorkspacePath) -> TerminalResult<()> {
        if path.is_root() {
            return Err(TerminalError::InvalidPath(path.to_string()));
        }
        let full = self.inside(path).await?;
        let metadata = fs::symlink_metadata(&full).await.map_err(|e| failed(path, e))?;
        let removed = match metadata.is_dir() {
            true => fs::remove_dir(&full).await,
            false => fs::remove_file(&full).await,
        };
        removed.map_err(|e| failed(path, e))
    }

    async fn mkdir(&self, path: &WorkspacePath) -> TerminalResult<()> {
        let full = self.inside(path).await?;
        fs::create_dir_all(&full).await.map_err(|e| failed(path, e))
    }

    async fn move_entry(&self, from: &WorkspacePath, to: &WorkspacePath) -> TerminalResult<()> {
        if from.is_root() || to.is_root() {
            return Err(TerminalError::InvalidPath(
                if from.is_root() { from } else { to }.to_string(),
            ));
        }
        let (source, target) = (self.inside(from).await?, self.inside(to).await?);
        fs::symlink_metadata(&source).await.map_err(|e| failed(from, e))?;
        if fs::symlink_metadata(&target).await.is_ok() {
            return Err(TerminalError::Exists(to.to_string()));
        }
        let parent = to.parent();
        fs::create_dir_all(on_host(&self.dir, &parent)).await.map_err(|e| failed(&parent, e))?;
        fs::rename(&source, &target).await.map_err(|e| failed(to, e))
    }
}

/// Where `path` lies on the host, below the workspace's directory `dir`.
fn on_host(dir: &Path, path: &WorkspacePath) -> PathBuf {
    path.components().iter().fold(dir.to_path_buf(), |host, part| host.join(part))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use caelumora_api::component::Services;
    use caelumora_domain::config::BackendConfig;
    use caelumora_domain::workspace::CommandStatus;
    use futures::stream;
    use serde_json::{json, Value};
    use std::env;
    use std::os::unix::fs::symlink;
    use std::slice;
    use tempfile::TempDir;

    fn content(text: &'static str) -> ByteStream {
        Box::pin(stream::iter([Ok(Bytes::from_static(text.as_bytes()))]))
    }

    async fn text(terminal: &dyn TerminalReader, path: &str) -> String {
        let (_, stream) = terminal.read(&WorkspacePath::parse(path).unwrap()).await.expect(path);
        let chunks: Vec<_> = stream.collect().await;
        String::from_utf8(chunks.into_iter().flat_map(|c| c.unwrap().to_vec()).collect()).unwrap()
    }

    async fn workspace() -> (TempDir, LocalProvider, Arc<dyn Terminal>, WorkspaceId) {
        let root = tempfile::tempdir().unwrap();
        let provider = LocalProvider::new(
            root.path().to_path_buf(),
            false,
            Duration::from_secs(3600),
            100,
            None,
        );
        let id = WorkspaceId::new();
        provider.create(id).await.unwrap();
        let terminal = provider.terminal_for(id).await.unwrap();
        (root, provider, terminal, id)
    }

    fn path(text: &str) -> WorkspacePath {
        WorkspacePath::parse(text).unwrap()
    }

    #[tokio::test]
    async fn files_are_written_read_listed_moved_and_deleted() {
        let (_root, _, terminal, _) = workspace().await;
        assert_eq!(
            terminal.write(&path("docs/a.md"), content("hello")).await.unwrap(),
            5,
            "and its directory made"
        );
        assert_eq!(text(terminal.as_ref(), "docs/a.md").await, "hello");
        terminal.write(&path("docs/a.md"), content("again")).await.expect("replaced");
        assert_eq!(text(terminal.as_ref(), "docs/a.md").await, "again");

        let listed = terminal.list(&WorkspacePath::root()).await.unwrap();
        assert_eq!(
            listed.iter().map(|e| (e.name.as_str(), e.is_dir)).collect::<Vec<_>>(),
            [("docs", true)]
        );
        assert_eq!(terminal.stat(&path("docs/a.md")).await.unwrap().size, 5);

        terminal.move_entry(&path("docs/a.md"), &path("archive/a.md")).await.unwrap();
        assert!(
            matches!(terminal.stat(&path("docs/a.md")).await, Err(TerminalError::NotFound(p)) if p == "docs/a.md")
        );
        terminal.write(&path("docs/b.md"), content("b")).await.unwrap();
        assert!(
            matches!(
                terminal.move_entry(&path("docs/b.md"), &path("archive/a.md")).await,
                Err(TerminalError::Exists(_))
            ),
            "nothing is moved over"
        );

        assert!(matches!(terminal.delete(&path("docs")).await, Err(TerminalError::NotEmpty(_))));
        terminal.delete(&path("docs/b.md")).await.unwrap();
        terminal.delete(&path("docs")).await.expect("empty now");
        assert!(
            matches!(
                terminal.delete(&WorkspacePath::root()).await,
                Err(TerminalError::InvalidPath(_))
            ),
            "not the workspace itself"
        );
    }

    #[tokio::test]
    async fn a_write_beyond_the_limit_leaves_nothing() {
        let root = tempfile::tempdir().unwrap();
        let provider = LocalProvider::new(
            root.path().to_path_buf(),
            false,
            Duration::from_secs(3600),
            4,
            None,
        );
        let id = WorkspaceId::new();
        provider.create(id).await.unwrap();
        let terminal = provider.terminal_for(id).await.unwrap();
        terminal.write(&path("a.txt"), content("old")).await.unwrap();
        let refused = terminal.write(&path("a.txt"), content("far too long")).await;
        assert!(matches!(refused, Err(TerminalError::TooLarge { limit: 4 })), "{refused:?}");
        assert_eq!(text(terminal.as_ref(), "a.txt").await, "old", "the file as it was");
        let names: Vec<_> = terminal
            .list(&WorkspacePath::root())
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["a.txt"], "no part left behind");
    }

    #[tokio::test]
    async fn a_link_out_of_the_workspace_is_not_followed() {
        let (root, _, terminal, id) = workspace().await;
        let secret = root.path().join("secret.txt");
        std_fs::write(&secret, "secret").unwrap();
        let dir = root.path().join(id.to_string());
        symlink(&secret, dir.join("leak")).unwrap();
        symlink(root.path(), dir.join("up")).unwrap();
        symlink(root.path().join("missing"), dir.join("dangling")).unwrap();
        std_fs::write(dir.join("inner.txt"), "inner").unwrap();
        symlink(dir.join("inner.txt"), dir.join("alias")).unwrap();

        for refused in ["leak", "up/secret.txt", "dangling"] {
            let read = terminal.read(&path(refused)).await;
            assert!(
                matches!(&read, Err(TerminalError::InvalidPath(p)) if p == refused),
                "{refused}: {:?}",
                read.err()
            );
        }
        assert!(matches!(
            terminal.write(&path("up/new.txt"), content("x")).await,
            Err(TerminalError::InvalidPath(_))
        ));
        assert!(
            matches!(
                terminal.write(&path("dangling"), content("x")).await,
                Err(TerminalError::InvalidPath(_))
            ),
            "nor written through"
        );
        assert!(!root.path().join("new.txt").exists() && !root.path().join("missing").exists());
        assert_eq!(
            text(terminal.as_ref(), "alias").await,
            "inner",
            "a link that stays inside is followed"
        );
    }

    #[tokio::test]
    async fn a_place_is_made_and_removed_with_everything_in_it() {
        let (root, provider, terminal, id) = workspace().await;
        terminal.write(&path("a/b/c.txt"), content("c")).await.unwrap();
        provider.remove(id).await.unwrap();
        assert!(!root.path().join(id.to_string()).exists());
        provider.remove(id).await.expect("nothing there is no error");
        let gone = provider.terminal_for(id).await;
        assert!(matches!(gone, Err(TerminalError::Unavailable(_))), "{:?}", gone.err());
    }

    #[tokio::test]
    async fn the_commands_of_its_workspaces_are_listed_and_ended_when_told() {
        let root = tempfile::tempdir().unwrap();
        let provider = LocalProvider::new(
            root.path().to_path_buf(),
            true,
            Duration::from_secs(3600),
            100,
            None,
        );
        let id = WorkspaceId::new();
        provider.create(id).await.unwrap();
        let shell = provider.shell_for(id, "channel").await.unwrap().expect("a shell");
        let long = shell.start("sleep 30").await.unwrap();
        let short = shell.start("true").await.unwrap();
        shell.output(&short, 0, Duration::from_secs(10)).await.unwrap();

        let key = (id, "channel".to_string(), long.clone());
        assert_eq!(provider.commands(), slice::from_ref(&key), "what may still run");
        provider.end_command(&key).await.unwrap();
        assert_eq!(
            shell.output(&long, 0, Duration::ZERO).await.unwrap().status,
            CommandStatus::Killed
        );
        assert!(provider.commands().is_empty());
        provider.end_command(&key).await.expect("one that ended is no error");
        provider
            .end_command(&(id, "channel".to_string(), "nonesuch".to_string()))
            .await
            .expect("nor one not known");
    }

    #[tokio::test]
    async fn what_runs_in_a_place_is_ended_before_it_is_removed() {
        let root = tempfile::tempdir().unwrap();
        let provider = LocalProvider::new(
            root.path().to_path_buf(),
            true,
            Duration::from_secs(3600),
            100,
            None,
        );
        let (id, other) = (WorkspaceId::new(), WorkspaceId::new());
        provider.create(id).await.unwrap();
        provider.create(other).await.unwrap();
        let shell = provider.shell_for(id, "channel").await.unwrap().unwrap();
        let elsewhere = provider.shell_for(other, "channel").await.unwrap().unwrap();
        let writing = shell.start("while true; do date > out.txt; sleep 0.05; done").await.unwrap();
        let untouched = elsewhere.start("sleep 30").await.unwrap();

        provider.remove(id).await.expect("removed while a command writes into it");
        assert_eq!(
            shell.output(&writing, 0, Duration::ZERO).await.unwrap().status,
            CommandStatus::Killed
        );
        assert!(!root.path().join(id.to_string()).exists(), "and nothing written again");
        assert_eq!(
            provider.commands(),
            [(other, "channel".to_string(), untouched.clone())],
            "those of another workspace run on"
        );
        provider.remove(other).await.unwrap();
    }

    #[test]
    fn an_error_names_no_more_than_the_caller_wrote() {
        let error =
            failed(&path("docs/a.md"), Error::new(ErrorKind::PermissionDenied, "/srv/secret/path"));
        assert!(!error.to_string().contains("/srv"), "{error}");
    }

    #[test]
    fn its_parameters_are_a_root_and_what_it_takes_by_default() {
        let make = |params: Value| {
            let storages = [("dev".to_string(), BackendConfig::new("local", params))].into();
            Backends::built_in().make_named::<dyn TerminalProvider>(
                "storage of workspaces",
                &storages,
                &Services::empty(),
            )
        };
        let plain = make(json!({ "root": "x" })).expect("a root alone");
        assert!(
            !plain.get("dev").unwrap().has_shell()
                && plain.get("dev").unwrap().destination().is_none(),
            "no shell, no bound"
        );
        let shell = make(json!({ "root": "x", "shell": true })).expect("with a shell");
        assert_eq!(
            shell.get("dev").unwrap().destination(),
            Some(&Classification::PUBLIC),
            "public, where it has a shell"
        );
        for (params, why) in [
            (json!({ "root": "x", "max_run_secs": 0 }), "max_run_secs"),
            (json!({ "root": "x", "rot": "y" }), "rot"),
            (json!({}), "root"),
        ] {
            let refused = make(params).map(|_| ()).unwrap_err().to_string();
            assert!(
                refused.contains("the storage of workspaces 'dev' (local)")
                    && refused.contains(why),
                "{refused}"
            );
        }
    }

    /// A storage `directory` below `root`, every workspace in `dir`, with a shell given
    /// `environment`.
    fn sharing(root: &Path, dir: &Path, environment: Environment) -> LocalProvider {
        let destination = Some(Classification::PUBLIC);
        LocalProvider::new(root.to_path_buf(), true, Duration::from_secs(3600), 100, destination)
            .sharing(std_fs::canonicalize(dir).unwrap(), environment)
    }

    /// What `command` printed in the shell of workspace `id`.
    async fn printed(provider: &LocalProvider, id: WorkspaceId, command: &str) -> String {
        let shell = provider.shell_for(id, "channel").await.unwrap().unwrap();
        let started = shell.start(command).await.unwrap();
        shell.output(&started, 0, Duration::from_secs(10)).await.unwrap().output
    }

    /// A workspace of a shared directory works in it as it is - a command and its environment see
    /// the directory, never the link that names it -, and leaves it when it is removed.
    #[tokio::test]
    async fn a_workspace_of_a_shared_directory_works_in_it_and_leaves_it() {
        let (root, dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let real = std_fs::canonicalize(dir.path()).unwrap();
        let provider = sharing(root.path(), dir.path(), Environment::Isolated);
        let id = WorkspaceId::new();
        provider.create(id).await.unwrap();
        assert_eq!(std_fs::read_link(root.path().join(id.to_string())).unwrap(), real);

        let terminal = provider.terminal_for(id).await.unwrap();
        terminal.write(&path("a.txt"), content("hello")).await.unwrap();
        assert_eq!(std_fs::read_to_string(dir.path().join("a.txt")).unwrap(), "hello");
        let seen =
            printed(&provider, id, "pwd; echo $PWD; echo $HOME; echo $CAELUMORA_WORKSPACE").await;
        let real = real.display().to_string();
        assert_eq!(seen.lines().collect::<Vec<_>>(), [real.as_str(); 4], "{seen}");

        provider.remove(id).await.unwrap();
        assert!(std_fs::symlink_metadata(root.path().join(id.to_string())).is_err(), "its link");
        assert!(dir.path().join("a.txt").is_file(), "never the directory");
    }

    /// The link remembers the directory: a storage told another one finds a workspace where it
    /// was made.
    #[tokio::test]
    async fn a_workspace_keeps_its_directory_when_the_storage_names_another() {
        let root = tempfile::tempdir().unwrap();
        let (first, second) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let id = WorkspaceId::new();
        sharing(root.path(), first.path(), Environment::Isolated).create(id).await.unwrap();

        let elsewhere = sharing(root.path(), second.path(), Environment::Isolated);
        let terminal = elsewhere.terminal_for(id).await.unwrap();
        terminal.write(&path("b.txt"), content("kept")).await.unwrap();

        assert!(first.path().join("b.txt").is_file());
        assert!(!second.path().join("b.txt").exists());
    }

    /// An inherited environment is the program's, its `HOME` too, with the workspace beside it.
    #[tokio::test]
    async fn an_inherited_environment_is_the_program_s() {
        let (root, dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let provider = sharing(root.path(), dir.path(), Environment::Inherit);
        let id = WorkspaceId::new();
        provider.create(id).await.unwrap();

        let seen =
            printed(&provider, id, "echo \"$HOME\"; echo \"$CAELUMORA_WORKSPACE\"; pwd -L").await;

        let home = env::var("HOME").unwrap_or_default();
        let real = std_fs::canonicalize(dir.path()).unwrap().display().to_string();
        let expected = [home.as_str(), real.as_str(), real.as_str()];
        assert_eq!(seen.lines().collect::<Vec<_>>(), expected, "its own PWD, not the program's");
    }

    /// A workspace whose directory is gone is told so, and its link is removed all the same.
    #[tokio::test]
    async fn a_workspace_whose_directory_is_gone_is_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let provider = sharing(root.path(), dir.path(), Environment::Isolated);
        let id = WorkspaceId::new();
        provider.create(id).await.unwrap();
        dir.close().unwrap();

        let gone = provider.terminal_for(id).await.map(drop);
        assert!(matches!(gone, Err(TerminalError::Unavailable(ref why)) if why.contains("gone")));
        provider.remove(id).await.expect("its link goes");
        assert!(std_fs::symlink_metadata(root.path().join(id.to_string())).is_err());
    }

    /// What runs in a workspace of a shared directory is ended as it is removed.
    #[tokio::test]
    async fn what_runs_in_a_shared_directory_is_ended_as_its_workspace_is_removed() {
        let (root, dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let provider = sharing(root.path(), dir.path(), Environment::Isolated);
        let id = WorkspaceId::new();
        provider.create(id).await.unwrap();
        let shell = provider.shell_for(id, "channel").await.unwrap().unwrap();
        let running = shell.start("sleep 30").await.unwrap();

        provider.remove(id).await.unwrap();

        let ended = shell.output(&running, 0, Duration::ZERO).await.unwrap();
        assert_eq!(ended.status, CommandStatus::Killed);
    }

    /// A storage `directory` names its directory, which has to be one, and its destination.
    #[test]
    fn a_shared_directory_is_named_with_its_destination() {
        let dir = tempfile::tempdir().unwrap();
        let make = |params: Value| {
            let storages = [("here".to_string(), BackendConfig::new("directory", params))].into();
            Backends::built_in()
                .make_named::<dyn TerminalProvider>(
                    "storage of workspaces",
                    &storages,
                    &Services::empty(),
                )
                .map(drop)
                .map_err(|e| e.to_string())
        };
        let destination = json!({ "level": "internal" });
        make(json!({ "root": "x", "dir": dir.path(), "destination": destination }))
            .expect("its root, its directory and its destination");
        let file = dir.path().join("file");
        std_fs::write(&file, "").unwrap();
        for (params, why) in [
            (json!({ "root": "x", "dir": dir.path() }), "destination"),
            (
                json!({ "root": "x", "dir": "/no/such/dir", "destination": destination }),
                "dir cannot be reached",
            ),
            (
                json!({ "root": "x", "dir": file, "destination": destination }),
                "dir is no directory",
            ),
        ] {
            let refused = make(params).unwrap_err();
            assert!(refused.contains("(directory)") && refused.contains(why), "{refused}");
        }
    }
}
