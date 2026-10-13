//! Open Terminal (issue 251): an instance that has a storage's workspaces
//! at a path of its own, reached over its REST API - the files through
//! `/files/*`, the shell through `/execute`. One handle per workspace
//! serves both, over the same instance, so that what a command writes is
//! what the panel reads.
//!
//! What Open Terminal does differently from the `local` provider, and how it
//! is met:
//!
//! - It follows links without asking where they lead, and a command reaches
//!   the whole instance: there is no boundary between the workspaces of an
//!   instance, nor between a workspace and the instance (CONFIDENTIALITY.md).
//! - It has no `stat`: what a path names is read from its directory's list.
//!   It removes a directory with everything in it: emptiness is checked
//!   first, which a command between the check and the removal can undo.
//! - It writes a file in place, from memory: a write that breaks off may
//!   leave it half written, and what is written is held here up to the
//!   limit before it is sent.
//! - Its offsets count pieces of output, not bytes; the tool only hands
//!   them back. A piece is decoded on its own, so a character split between
//!   two is replaced.
//! - Its commands get its own environment, the instance's key among it: the
//!   key is blanked in what a command is given, but a command can read it
//!   from the server's process. Commands are told apart by `X-Session-Id`,
//!   which keeps the tool's channels apart, not a command from the API.
//! - It lists its commands only by `X-Session-Id`, and does not say which
//!   workspace one runs in (issue 254): the storage keeps the commands it
//!   started itself (`Commands`), and forgets them when Caelumora ends.
//! - It bounds no command: a command is started under `timeout`, which
//!   kills its process group once it ran for the storage's `max_run` - by
//!   the instance itself, also when Caelumora no longer knows the command.
//!   The instance tells that as a signal it was not sent, told here as
//!   killed.
//!
//! No message of the instance is passed on: they name paths inside it.

use crate::client::Client;
use crate::wire;
use async_trait::async_trait;
use caelumora_model::workspace::{CommandStatus, WorkspaceId};
use caelumora_runtime::remote::describe_request_error;
use caelumora_runtime::terminal::{ByteStream, CommandKey, CommandState, Commands, FileEntry, Kept, Shell, Terminal, TerminalError, TerminalResult, WorkspacePath};
use chrono::{DateTime, Utc};
use futures::{StreamExt, TryStreamExt};
use reqwest::multipart::{Form, Part};
use reqwest::{Method, StatusCode};
use std::collections::BTreeMap;
use std::io::Error as IoError;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// The most pieces of output one answer holds - with a pseudo-terminal's
/// pieces of at most 4 KiB about the 1 MiB the `local` shell keeps; older
/// ones are dropped, and the answer says so.
const KEPT_ENTRIES: u32 = 256;
/// The longest wait Open Terminal takes.
const MAX_WAIT: Duration = Duration::from_secs(300);
/// How long a kill waits for the command's last output and exit code.
const KILL_WAIT: Duration = Duration::from_secs(2);

/// A command as it was started here (issue 254): it may run until it is
/// seen to have ended, or for `max_run` at most.
#[derive(Debug)]
pub(crate) struct Started {
    at: Instant,
    max_run: Duration,
    /// When it was seen to have ended; set once.
    ended: OnceLock<Instant>,
}

impl Started {
    /// Whether it ran as long as it may.
    fn ran_out(&self) -> bool {
        self.at.elapsed() >= self.max_run
    }
}

impl Kept for Started {
    fn ended(&self) -> Option<Instant> {
        self.ended.get().copied().or_else(|| self.ran_out().then(|| self.at + self.max_run))
    }
}

/// The commands the workspaces of an Open Terminal storage were given.
pub(crate) type OpenCommands = Commands<Arc<Started>>;

/// One workspace in an instance: its directory there, `base`, and the
/// commands of its storage, which run for `max_run` at most.
#[derive(Debug, Clone)]
pub(crate) struct OpenTerminal {
    pub(crate) client: Arc<Client>,
    pub(crate) base: String,
    pub(crate) workspace: WorkspaceId,
    pub(crate) commands: Arc<OpenCommands>,
    pub(crate) max_run: Duration,
}

impl OpenTerminal {
    /// Where `path` lies in the instance.
    fn at(&self, path: &WorkspacePath) -> String {
        match path.is_root() {
            true => self.base.clone(),
            false => format!("{}/{path}", self.base),
        }
    }

    /// The entries of the directory `dir`; `None` where there is none.
    pub(crate) async fn listing(&self, dir: &WorkspacePath) -> TerminalResult<Option<Vec<FileEntry>>> {
        let answer = self.client.send(Method::GET, &["files", "list"], &[("directory", self.at(dir))], None, Duration::ZERO, |r| r).await?;
        if answer.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let listing: wire::Listing = self.client.parsed(answer).await?;
        let mut entries: Vec<FileEntry> = listing.entries.into_iter().map(|listed| {
            let is_dir = listed.kind == "directory";
            FileEntry {
                name: listed.name,
                is_dir,
                size: if is_dir { 0 } else { listed.size },
                modified: listed.modified.and_then(|secs| DateTime::<Utc>::from_timestamp_millis((secs * 1000.0) as i64)),
            }
        }).collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Some(entries))
    }

    /// What `path` names, if anything.
    async fn found(&self, path: &WorkspacePath) -> TerminalResult<Option<FileEntry>> {
        if path.is_root() {
            return match self.listing(path).await? {
                Some(_) => Ok(Some(FileEntry { name: String::new(), is_dir: true, size: 0, modified: None })),
                None => Err(TerminalError::Unavailable("its place is gone".to_string())),
            };
        }
        let name = path.file_name().unwrap_or_default();
        Ok(self.listing(&path.parent()).await?.and_then(|entries| entries.into_iter().find(|entry| entry.name == name)))
    }

    /// Why a request about `path` was refused with `status`, told as the
    /// `local` provider would: by what `path` and the directory it lies in
    /// are, asked again.
    async fn why(&self, path: &WorkspacePath, status: StatusCode) -> TerminalError {
        match self.found(&path.parent()).await {
            Ok(Some(parent)) if !parent.is_dir => TerminalError::NotADirectory(path.parent().to_string()),
            Ok(None) if !path.is_root() => TerminalError::NotFound(path.parent().to_string()),
            _ => self.client.refused(status),
        }
    }

    /// The state of command `id` from `process`, its output joined and its
    /// carriage returns - a pseudo-terminal's - dropped.
    fn state(id: &str, process: wire::Process) -> TerminalResult<CommandState> {
        let (status, exit_code) = match (process.status.as_str(), process.exit_code) {
            ("running", _) => (CommandStatus::Running, None),
            ("killed", code) => (CommandStatus::Killed, code),
            ("done", Some(code)) if code >= 0 => (CommandStatus::Completed, Some(code)),
            // A negative code is a signal the command was not sent here.
            ("done", _) => (CommandStatus::Failed, None),
            (other, _) => return Err(TerminalError::Unavailable(format!("Open Terminal answered the unknown status '{other}' for a command"))),
        };
        let output = process.output.into_iter()
            .filter(|entry| matches!(entry.kind.as_str(), "output" | "stdout" | "stderr"))
            .map(|entry| entry.data)
            .collect::<String>()
            .replace('\r', "");
        Ok(CommandState { id: id.to_string(), status, exit_code, output, next_offset: process.next_offset, truncated: process.truncated })
    }
}

#[async_trait]
impl Terminal for OpenTerminal {
    async fn list(&self, dir: &WorkspacePath) -> TerminalResult<Vec<FileEntry>> {
        match self.listing(dir).await? {
            Some(entries) => Ok(entries),
            None => match self.found(dir).await? {
                Some(entry) if !entry.is_dir => Err(TerminalError::NotADirectory(dir.to_string())),
                _ => Err(TerminalError::NotFound(dir.to_string())),
            },
        }
    }

    async fn stat(&self, path: &WorkspacePath) -> TerminalResult<FileEntry> {
        self.found(path).await?.ok_or_else(|| TerminalError::NotFound(path.to_string()))
    }

    async fn read(&self, path: &WorkspacePath) -> TerminalResult<(u64, ByteStream)> {
        let answer = self.client.send(Method::GET, &["files", "view"], &[("path", self.at(path))], None, Duration::ZERO, |r| r).await?;
        if answer.status() == StatusCode::NOT_FOUND {
            return Err(match self.found(path).await? {
                Some(entry) if entry.is_dir => TerminalError::IsADirectory(path.to_string()),
                _ => TerminalError::NotFound(path.to_string()),
            });
        }
        let answer = self.client.success(answer)?;
        let size = match answer.content_length() {
            Some(size) => size,
            None => self.stat(path).await?.size,
        };
        let content = answer.bytes_stream().map_err(|e| IoError::other(describe_request_error(e)));
        Ok((size, Box::pin(content)))
    }

    async fn write(&self, path: &WorkspacePath, mut content: ByteStream, limit: u64) -> TerminalResult<u64> {
        let Some(name) = path.file_name() else { return Err(TerminalError::IsADirectory(path.to_string())) };
        if self.found(path).await?.is_some_and(|entry| entry.is_dir) {
            return Err(TerminalError::IsADirectory(path.to_string()));
        }
        let mut held = Vec::new();
        while let Some(chunk) = content.next().await {
            let chunk = chunk.map_err(|e| TerminalError::Unavailable(format!("what was to be written broke off: {}", e.kind())))?;
            if held.len() as u64 + chunk.len() as u64 > limit {
                return Err(TerminalError::TooLarge { limit });
            }
            held.extend_from_slice(&chunk);
        }
        let written = held.len() as u64;
        let file = Part::bytes(held).file_name(name.to_string());
        let form = Form::new().part("file", file);
        let answer = self.client.send(Method::POST, &["files", "upload"], &[("directory", self.at(&path.parent()))], None, Duration::ZERO, |r| r.multipart(form)).await?;
        match answer.status() {
            status if status.is_success() => Ok(written),
            status => Err(self.why(path, status).await),
        }
    }

    async fn delete(&self, path: &WorkspacePath) -> TerminalResult<()> {
        if path.is_root() {
            return Err(TerminalError::InvalidPath(path.to_string()));
        }
        let entry = self.stat(path).await?;
        // Open Terminal would remove a directory with everything in it.
        if entry.is_dir && !self.list(path).await?.is_empty() {
            return Err(TerminalError::NotEmpty(path.to_string()));
        }
        let answer = self.client.send(Method::DELETE, &["files", "delete"], &[("path", self.at(path))], None, Duration::ZERO, |r| r).await?;
        match answer.status() {
            status if status.is_success() => Ok(()),
            StatusCode::NOT_FOUND => Err(TerminalError::NotFound(path.to_string())),
            status => Err(self.client.refused(status)),
        }
    }

    async fn mkdir(&self, path: &WorkspacePath) -> TerminalResult<()> {
        if let Some(entry) = self.found(path).await? {
            return match entry.is_dir {
                true => Ok(()),
                false => Err(TerminalError::Exists(path.to_string())),
            };
        }
        let at = self.at(path);
        let answer = self.client.send(Method::POST, &["files", "mkdir"], &[], None, Duration::ZERO, |r| r.json(&wire::PathRequest { path: &at })).await?;
        match answer.status() {
            status if status.is_success() => Ok(()),
            status => Err(self.why(path, status).await),
        }
    }

    async fn move_entry(&self, from: &WorkspacePath, to: &WorkspacePath) -> TerminalResult<()> {
        if from.is_root() || to.is_root() {
            return Err(TerminalError::InvalidPath(if from.is_root() { from } else { to }.to_string()));
        }
        self.stat(from).await?;
        if self.found(to).await?.is_some() {
            return Err(TerminalError::Exists(to.to_string()));
        }
        // As the `local` provider does: the directory it goes to is made.
        self.mkdir(&to.parent()).await?;
        let (source, destination) = (self.at(from), self.at(to));
        let request = wire::MoveRequest { source: &source, destination: &destination };
        let answer = self.client.send(Method::POST, &["files", "move"], &[], None, Duration::ZERO, |r| r.json(&request)).await?;
        match answer.status() {
            status if status.is_success() => Ok(()),
            StatusCode::NOT_FOUND => Err(TerminalError::NotFound(from.to_string())),
            StatusCode::CONFLICT => Err(TerminalError::Exists(to.to_string())),
            status => Err(self.client.refused(status)),
        }
    }

    fn shell(&self, scope: &str) -> Option<Arc<dyn Shell>> {
        Some(Arc::new(OpenTerminalShell { terminal: self.clone(), scope: scope.to_string() }))
    }
}

/// The shell of one workspace in an instance, as one scope sees it: the
/// scope goes with every request, and the instance shows the commands of
/// that scope alone.
#[derive(Debug)]
struct OpenTerminalShell {
    terminal: OpenTerminal,
    scope: String,
}

impl OpenTerminalShell {
    fn key(&self, id: &str) -> CommandKey {
        (self.terminal.workspace, self.scope.clone(), id.to_string())
    }

    /// `asked`, what the instance told of command `id`, as the storage
    /// keeps it: a command seen to have ended, or no longer known, has
    /// ended; one that ended by a signal once it ran as long as it may was
    /// killed by its `timeout`.
    fn seen(&self, id: &str, asked: TerminalResult<CommandState>) -> TerminalResult<CommandState> {
        let started = self.terminal.commands.get(&self.key(id)).ok();
        let ended = match &asked {
            Ok(state) => state.status != CommandStatus::Running,
            Err(error) => matches!(error, TerminalError::UnknownCommand(_)),
        };
        if let Some(started) = started.as_ref().filter(|_| ended) {
            started.ended.get_or_init(Instant::now);
        }
        asked.map(|mut state| {
            if state.status == CommandStatus::Failed && started.is_some_and(|started| started.ran_out()) {
                state.status = CommandStatus::Killed;
            }
            state
        })
    }
}

#[async_trait]
impl Shell for OpenTerminalShell {
    async fn start(&self, command: &str) -> TerminalResult<String> {
        let OpenTerminalShell { terminal, scope } = self;
        let env = BTreeMap::from([
            ("TERM", "dumb".to_string()),
            ("NO_COLOR", "1".to_string()),
            ("CAELUMORA_WORKSPACE", terminal.workspace.to_string()),
            // The instance's own key, which it hands every command else.
            ("OPEN_TERMINAL_API_KEY", String::new()),
            ("CAELUMORA_COMMAND", command.to_string()),
        ]);
        // Bounded by the instance itself; the command from the environment,
        // so that nothing of it needs quoting.
        let bounded = format!("exec timeout -s KILL {} bash -c \"$CAELUMORA_COMMAND\"", terminal.max_run.as_secs_f64());
        let request = wire::ExecRequest { command: &bounded, cwd: &terminal.base, env };
        let at = Instant::now();
        // `wait=0` answers at once, whatever the instance's own default.
        let answer = terminal.client.send(Method::POST, &["execute"], &[("wait", "0".to_string())], Some(scope), Duration::ZERO, |r| r.json(&request)).await?;
        let process: wire::Process = terminal.client.parsed(answer).await?;
        terminal.commands.insert(self.key(&process.id), Arc::new(Started { at, max_run: terminal.max_run, ended: OnceLock::new() }));
        Ok(process.id)
    }

    async fn output(&self, id: &str, offset: u64, wait: Duration) -> TerminalResult<CommandState> {
        let client = &self.terminal.client;
        let wait = wait.min(MAX_WAIT);
        let query = [("offset", offset.to_string()), ("wait", wait.as_secs_f64().to_string()), ("tail", KEPT_ENTRIES.to_string())];
        let answer = client.send(Method::GET, &["execute", id, "status"], &query, Some(&self.scope), wait, |r| r).await?;
        let asked = match answer.status() {
            StatusCode::NOT_FOUND => Err(TerminalError::UnknownCommand(id.to_string())),
            _ => client.parsed(answer).await.and_then(|process| OpenTerminal::state(id, process)),
        };
        self.seen(id, asked)
    }

    async fn kill(&self, id: &str, offset: u64) -> TerminalResult<CommandState> {
        let client = &self.terminal.client;
        // SIGKILL to its process group, as the `local` shell sends.
        let answer = client.send(Method::DELETE, &["execute", id], &[("force", "true".to_string())], Some(&self.scope), KILL_WAIT, |r| r).await?;
        if answer.status() == StatusCode::NOT_FOUND {
            return self.seen(id, Err(TerminalError::UnknownCommand(id.to_string())));
        }
        client.success(answer)?;
        self.output(id, offset, KILL_WAIT).await
    }
}
