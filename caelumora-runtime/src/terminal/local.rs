//! The `local` provider (issue 251, part A): a directory per workspace
//! under a root on Caelumora's host, read and written by Caelumora itself.
//! A symbolic link inside a workspace is followed only where its target
//! stays inside: it is checked when a path is used, so a link made between
//! the check and the use - by a command running in the workspace - is the
//! one gap, closed only by a provider that keeps commands elsewhere.

use super::local_shell::{LocalCommands, LocalShell};
use super::{
    default_max_run_secs, max_run, ByteStream, CommandKey, FileEntry, Shell, Terminal,
    TerminalError, TerminalProvider, TerminalResult, WorkspacePath,
};
use crate::backend::Backends;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::workspace::WorkspaceId;
use futures::future::join_all;
use futures::StreamExt;
use serde::Deserialize;
use std::fs::Metadata;
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
}

/// Registers the storage `local` with `backends`.
pub(super) fn register_backend(backends: &mut Backends) {
    backends.register::<dyn TerminalProvider, _, _>("local", |_, config: LocalStorageConfig, _| {
        Ok(Arc::new(LocalProvider::new(
            config.root,
            config.shell,
            max_run(config.max_run_secs)?,
            config.destination,
        )))
    });
}

#[derive(Debug)]
pub(crate) struct LocalProvider {
    root: PathBuf,
    /// The commands of its workspaces, if it has a shell.
    commands: Option<Arc<LocalCommands>>,
    /// How long a command may run.
    max_run: Duration,
    destination: Option<Classification>,
}

impl LocalProvider {
    /// The provider of a storage whose workspaces lie under `root`, with a
    /// shell if `shell`; what may be put into it at most `destination`, or
    /// `public` with a shell and no bound without one.
    pub(crate) fn new(
        root: PathBuf,
        shell: bool,
        max_run: Duration,
        destination: Option<Classification>,
    ) -> Self {
        let destination = destination.or_else(|| shell.then_some(Classification::PUBLIC));
        Self { root, commands: shell.then(Arc::default), max_run, destination }
    }

    fn place(&self, id: WorkspaceId) -> PathBuf {
        self.root.join(id.to_string())
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
        fs::create_dir_all(self.place(id)).await.map_err(unavailable)
    }

    async fn remove(&self, id: WorkspaceId) -> TerminalResult<()> {
        // Nothing of it goes on writing into what is being removed.
        self.end_commands_of(id).await?;
        match fs::remove_dir_all(self.place(id)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(unavailable(e)),
        }
    }

    async fn terminal_for(&self, id: WorkspaceId) -> TerminalResult<Arc<dyn Terminal>> {
        let dir = self.place(id);
        match fs::metadata(&dir).await {
            Ok(metadata) if metadata.is_dir() => Ok(Arc::new(LocalTerminal {
                dir,
                workspace: id,
                commands: self.commands.clone(),
                max_run: self.max_run,
            })),
            Ok(_) => Err(TerminalError::Unavailable("its place is not a directory".to_string())),
            Err(e) if e.kind() == ErrorKind::NotFound => {
                warn!(workspace = %id, "The place of a workspace is gone");
                Err(TerminalError::Unavailable("its place is gone".to_string()))
            }
            Err(e) => Err(unavailable(e)),
        }
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
    workspace: WorkspaceId,
    commands: Option<Arc<LocalCommands>>,
    max_run: Duration,
}

impl LocalTerminal {
    /// Where `path` lies on the host, if that is inside the workspace: what
    /// of it exists, links followed, must lie in the workspace's directory.
    async fn inside(&self, path: &WorkspacePath) -> TerminalResult<PathBuf> {
        let full = path.under(&self.dir);
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
impl Terminal for LocalTerminal {
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

    async fn write(
        &self,
        path: &WorkspacePath,
        content: ByteStream,
        limit: u64,
    ) -> TerminalResult<u64> {
        if path.is_root() {
            return Err(TerminalError::IsADirectory(path.to_string()));
        }
        let full = self.inside(path).await?;
        if fs::metadata(&full).await.is_ok_and(|m| m.is_dir()) {
            return Err(TerminalError::IsADirectory(path.to_string()));
        }
        let parent = path.parent();
        fs::create_dir_all(parent.under(&self.dir)).await.map_err(|e| failed(&parent, e))?;
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
        fs::create_dir_all(parent.under(&self.dir)).await.map_err(|e| failed(&parent, e))?;
        fs::rename(&source, &target).await.map_err(|e| failed(to, e))
    }

    fn shell(&self, scope: &str) -> Option<Arc<dyn Shell>> {
        let commands = Arc::clone(self.commands.as_ref()?);
        Some(Arc::new(LocalShell {
            dir: self.dir.clone(),
            workspace: self.workspace,
            scope: scope.to_string(),
            commands,
            max_run: self.max_run,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::Services;
    use bytes::Bytes;
    use caelumora_model::config::BackendConfig;
    use caelumora_model::workspace::CommandStatus;
    use futures::stream;
    use serde_json::{json, Value};
    use std::fs as std_fs;
    use std::os::unix::fs::symlink;
    use std::slice;
    use tempfile::TempDir;

    fn content(text: &'static str) -> ByteStream {
        Box::pin(stream::iter([Ok(Bytes::from_static(text.as_bytes()))]))
    }

    async fn text(terminal: &dyn Terminal, path: &str) -> String {
        let (_, stream) = terminal.read(&WorkspacePath::parse(path).unwrap()).await.expect(path);
        let chunks: Vec<_> = stream.collect().await;
        String::from_utf8(chunks.into_iter().flat_map(|c| c.unwrap().to_vec()).collect()).unwrap()
    }

    async fn workspace() -> (TempDir, LocalProvider, Arc<dyn Terminal>, WorkspaceId) {
        let root = tempfile::tempdir().unwrap();
        let provider =
            LocalProvider::new(root.path().to_path_buf(), false, Duration::from_secs(3600), None);
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
            terminal.write(&path("docs/a.md"), content("hello"), 100).await.unwrap(),
            5,
            "and its directory made"
        );
        assert_eq!(text(terminal.as_ref(), "docs/a.md").await, "hello");
        terminal.write(&path("docs/a.md"), content("again"), 100).await.expect("replaced");
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
        terminal.write(&path("docs/b.md"), content("b"), 100).await.unwrap();
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
        let (_root, _, terminal, _) = workspace().await;
        terminal.write(&path("a.txt"), content("old"), 100).await.unwrap();
        let refused = terminal.write(&path("a.txt"), content("far too long"), 4).await;
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
            terminal.write(&path("up/new.txt"), content("x"), 10).await,
            Err(TerminalError::InvalidPath(_))
        ));
        assert!(
            matches!(
                terminal.write(&path("dangling"), content("x"), 10).await,
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
        terminal.write(&path("a/b/c.txt"), content("c"), 10).await.unwrap();
        provider.remove(id).await.unwrap();
        assert!(!root.path().join(id.to_string()).exists());
        provider.remove(id).await.expect("nothing there is no error");
        let gone = provider.terminal_for(id).await;
        assert!(matches!(gone, Err(TerminalError::Unavailable(_))), "{:?}", gone.err());
    }

    #[tokio::test]
    async fn the_commands_of_its_workspaces_are_listed_and_ended_when_told() {
        let root = tempfile::tempdir().unwrap();
        let provider =
            LocalProvider::new(root.path().to_path_buf(), true, Duration::from_secs(3600), None);
        let id = WorkspaceId::new();
        provider.create(id).await.unwrap();
        let shell = provider.terminal_for(id).await.unwrap().shell("channel").expect("a shell");
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
        let provider =
            LocalProvider::new(root.path().to_path_buf(), true, Duration::from_secs(3600), None);
        let (id, other) = (WorkspaceId::new(), WorkspaceId::new());
        provider.create(id).await.unwrap();
        provider.create(other).await.unwrap();
        let shell = provider.terminal_for(id).await.unwrap().shell("channel").unwrap();
        let elsewhere = provider.terminal_for(other).await.unwrap().shell("channel").unwrap();
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
}
