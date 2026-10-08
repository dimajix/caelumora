//! The shell of the `local` provider (issue 251): commands run with `bash
//! -c` in the workspace's directory on Caelumora's host, as its user - so it
//! is opt-in (`shell` of the storage) and meant for development and tests.
//!
//! - Each command runs in a process group of its own, and killing it kills
//!   the group: what it started goes with it.
//! - Its environment is cleared but for `PATH`, `HOME` (the workspace),
//!   `LANG`, `TERM=dumb`, `NO_COLOR` and `CAELUMORA_WORKSPACE`: nothing of
//!   Caelumora's own environment - no secret - reaches a command.
//! - What it prints is kept up to `KEPT_OUTPUT`, the oldest dropped first,
//!   and the command for `KEPT_AFTER_END` after it ended, so that its
//!   output can still be asked for - as Open Terminal keeps its processes.
//! - Commands are kept by the provider, not by a call, so that one outlives
//!   the call that started it; a command still running when Caelumora ends
//!   is not killed by it.

use super::{CommandState, CommandStatus, Shell, TerminalError, TerminalResult};
use async_trait::async_trait;
use caelumora_model::workspace::WorkspaceId;
use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::watch;
use tokio_util::task::AbortOnDropHandle;

/// How much of a command's output is kept; older output is dropped.
const KEPT_OUTPUT: usize = 1024 * 1024;
/// How long a command is kept after it ended, for its output to be asked for.
const KEPT_AFTER_END: Duration = Duration::from_secs(300);
/// How long output is still read once the command ended - what it left
/// running may hold its output open.
const OUTPUT_AFTER_END: Duration = Duration::from_millis(500);
/// How long a kill waits for the command to end.
const KILL_WAIT: Duration = Duration::from_secs(2);

/// A lock of these few, never held across an await, taken also if a holder
/// panicked: nothing under it is left half changed.
fn locked<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, Copy)]
struct End {
    status: CommandStatus,
    exit_code: Option<i32>,
    at: Instant,
}

/// What a command printed: the newest `KEPT_OUTPUT` bytes, after `dropped`
/// older ones.
#[derive(Debug, Default)]
struct Output {
    bytes: Vec<u8>,
    dropped: u64,
}

/// What the command's task and its callers share.
#[derive(Debug)]
struct Running {
    output: Mutex<Output>,
    end: watch::Sender<Option<End>>,
    killed: AtomicBool,
}

#[derive(Debug)]
struct Command {
    running: Arc<Running>,
    /// Its process group, the command's own process id.
    group: Option<Pid>,
    /// Waits for it and reads its output; aborted - and the process killed
    /// - when the command is forgotten.
    _task: AbortOnDropHandle<()>,
}

/// Where a command is kept: its workspace, the scope it was started with,
/// its id.
type Key = (WorkspaceId, String, String);

/// The commands of the workspaces of one storage.
#[derive(Debug, Default)]
pub(super) struct LocalCommands {
    commands: Mutex<HashMap<Key, Arc<Command>>>,
}

impl LocalCommands {
    /// The commands, those ended longer ago than `KEPT_AFTER_END` forgotten.
    fn kept(&self) -> MutexGuard<'_, HashMap<Key, Arc<Command>>> {
        let mut commands = locked(&self.commands);
        commands.retain(|_, command| command.running.end.borrow().is_none_or(|end| end.at.elapsed() < KEPT_AFTER_END));
        commands
    }

    fn get(&self, key: &Key) -> TerminalResult<Arc<Command>> {
        self.kept().get(key).cloned().ok_or_else(|| TerminalError::UnknownCommand(key.2.clone()))
    }
}

/// The shell of one workspace.
#[derive(Debug)]
pub(super) struct LocalShell {
    pub(super) dir: PathBuf,
    pub(super) workspace: WorkspaceId,
    pub(super) commands: Arc<LocalCommands>,
}

impl LocalShell {
    fn key(&self, scope: &str, id: &str) -> Key {
        (self.workspace, scope.to_string(), id.to_string())
    }
}

/// Reads `from` into the output until it ends.
async fn pump(from: Option<impl AsyncRead + Unpin>, running: &Running) {
    let Some(mut from) = from else { return };
    let mut buffer = [0u8; 8192];
    while let Ok(read) = from.read(&mut buffer).await {
        if read == 0 {
            break;
        }
        let mut output = locked(&running.output);
        output.bytes.extend_from_slice(&buffer[..read]);
        if output.bytes.len() > KEPT_OUTPUT {
            let excess = output.bytes.len() - KEPT_OUTPUT;
            output.bytes.drain(..excess);
            output.dropped += excess as u64;
        }
    }
}

/// How much of `bytes` is complete text: all of it, but for a character
/// cut off at its end, which the next read completes.
fn complete(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => bytes.len(),
    }
}

/// Command `id` as it stands, with what it printed since `offset`.
fn state(id: &str, running: &Running, offset: u64) -> CommandState {
    let end = *running.end.borrow();
    let output = locked(&running.output);
    let truncated = offset < output.dropped;
    let start = (offset.saturating_sub(output.dropped) as usize).min(output.bytes.len());
    let rest = &output.bytes[start..];
    // A character cut off is given once it is whole - or as it is once the
    // command ended and nothing completes it.
    let taken = if end.is_some() { rest.len() } else { complete(rest) };
    CommandState {
        id: id.to_string(),
        status: end.map_or(CommandStatus::Running, |end| end.status),
        exit_code: end.and_then(|end| end.exit_code),
        output: String::from_utf8_lossy(&rest[..taken]).replace('\r', ""),
        next_offset: output.dropped + (start + taken) as u64,
        truncated,
    }
}

#[async_trait]
impl Shell for LocalShell {
    async fn start(&self, scope: &str, command: &str) -> TerminalResult<String> {
        let mut child = tokio::process::Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&self.dir)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into()))
            .env("HOME", &self.dir)
            .env("LANG", "C.UTF-8")
            .env("TERM", "dumb")
            .env("NO_COLOR", "1")
            .env("CAELUMORA_WORKSPACE", &self.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| TerminalError::Unavailable(format!("the shell could not be started: {}", e.kind())))?;
        let group = child.id().and_then(|pid| i32::try_from(pid).ok()).map(Pid::from_raw);
        let running = Arc::new(Running { output: Mutex::default(), end: watch::channel(None).0, killed: AtomicBool::new(false) });

        let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
        let shared = Arc::clone(&running);
        let task = tokio::spawn(async move {
            let reading = Arc::clone(&shared);
            let mut pumps = AbortOnDropHandle::new(tokio::spawn(async move {
                tokio::join!(pump(stdout, &reading), pump(stderr, &reading));
            }));
            let exit = child.wait().await;
            let _ = tokio::time::timeout(OUTPUT_AFTER_END, &mut pumps).await;
            let killed = shared.killed.load(Ordering::SeqCst);
            let (status, exit_code) = match exit {
                Ok(_) if killed => (CommandStatus::Killed, None),
                Ok(exit) => match exit.code() {
                    Some(code) => (CommandStatus::Completed, Some(code)),
                    None => (CommandStatus::Failed, None),
                },
                Err(_) => (CommandStatus::Failed, None),
            };
            shared.end.send_replace(Some(End { status, exit_code, at: Instant::now() }));
        });

        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let command = Arc::new(Command { running, group, _task: AbortOnDropHandle::new(task) });
        self.commands.kept().insert(self.key(scope, &id), command);
        Ok(id)
    }

    async fn output(&self, scope: &str, id: &str, offset: u64, wait: Duration) -> TerminalResult<CommandState> {
        let command = self.commands.get(&self.key(scope, id))?;
        let mut end = command.running.end.subscribe();
        let _ = tokio::time::timeout(wait, end.wait_for(Option::is_some)).await;
        Ok(state(id, &command.running, offset))
    }

    async fn kill(&self, scope: &str, id: &str, offset: u64) -> TerminalResult<CommandState> {
        let command = self.commands.get(&self.key(scope, id))?;
        if command.running.end.borrow().is_none() {
            command.running.killed.store(true, Ordering::SeqCst);
            if let Some(group) = command.group {
                // Gone already is no error: it ended meanwhile.
                let _ = killpg(group, Signal::SIGKILL);
            }
            let mut end = command.running.end.subscribe();
            let _ = tokio::time::timeout(KILL_WAIT, end.wait_for(Option::is_some)).await;
        }
        Ok(state(id, &command.running, offset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(dir: &std::path::Path) -> LocalShell {
        LocalShell { dir: dir.to_path_buf(), workspace: WorkspaceId::new(), commands: Arc::default() }
    }

    const LONG: Duration = Duration::from_secs(10);

    #[tokio::test]
    async fn a_command_runs_in_the_workspace_and_its_output_is_followed() {
        let dir = tempfile::tempdir().unwrap();
        let shell = shell(dir.path());
        let id = shell.start("channel", "echo hello; pwd; echo oops >&2; exit 3").await.unwrap();
        let done = shell.output("channel", &id, 0, LONG).await.unwrap();
        assert_eq!((done.status, done.exit_code), (CommandStatus::Completed, Some(3)));
        let real = std::fs::canonicalize(dir.path()).unwrap();
        assert!(done.output.contains("hello") && done.output.contains(&real.display().to_string()) && done.output.contains("oops"), "{:?}", done.output);
        let again = shell.output("channel", &id, done.next_offset, Duration::ZERO).await.unwrap();
        assert_eq!(again.output, "", "nothing new past the offset");
    }

    #[tokio::test]
    async fn a_command_sees_nothing_of_caelumora_s_environment() {
        let dir = tempfile::tempdir().unwrap();
        let shell = shell(dir.path());
        let id = shell.start("channel", "env | sort").await.unwrap();
        let done = shell.output("channel", &id, 0, LONG).await.unwrap();
        let names: Vec<&str> = done.output.lines().filter_map(|line| line.split('=').next()).filter(|name| !["PWD", "SHLVL", "_"].contains(name)).collect();
        assert_eq!(names, ["CAELUMORA_WORKSPACE", "HOME", "LANG", "NO_COLOR", "PATH", "TERM"], "{}", done.output);
    }

    #[tokio::test]
    async fn a_command_is_seen_under_its_scope_alone() {
        let dir = tempfile::tempdir().unwrap();
        let shell = shell(dir.path());
        let id = shell.start("one", "true").await.unwrap();
        let elsewhere = shell.output("other", &id, 0, Duration::ZERO).await;
        assert!(matches!(elsewhere, Err(TerminalError::UnknownCommand(_))), "{elsewhere:?}");
    }

    #[tokio::test]
    async fn a_long_command_is_handed_back_running_and_killed_with_what_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let shell = shell(dir.path());
        let id = shell.start("channel", "echo started; sleep 30 & sleep 30").await.unwrap();
        let running = shell.output("channel", &id, 0, Duration::from_millis(300)).await.unwrap();
        assert_eq!(running.status, CommandStatus::Running);
        assert!(running.output.contains("started"), "{:?}", running.output);
        let killed = shell.kill("channel", &id, running.next_offset).await.unwrap();
        assert_eq!(killed.status, CommandStatus::Killed);
        let after = std::process::Command::new("pgrep").args(["-g", &shell.commands.get(&shell.key("channel", &id)).unwrap().group.unwrap().to_string()]).output().unwrap();
        assert!(after.stdout.is_empty(), "nothing of its group runs on: {:?}", String::from_utf8_lossy(&after.stdout));
    }

    #[test]
    fn output_is_given_from_the_offset_and_says_what_was_dropped() {
        let running = Running { output: Mutex::new(Output { bytes: "héllo".as_bytes().to_vec(), dropped: 10 }), end: watch::channel(None).0, killed: AtomicBool::new(false) };
        let all = state("x", &running, 10);
        assert_eq!((all.output.as_str(), all.next_offset, all.truncated), ("héllo", 16, false));
        assert!(state("x", &running, 4).truncated, "what was before the kept output is dropped");
        locked(&running.output).bytes.extend_from_slice(&[0xc3]);
        let cut = state("x", &running, 10);
        assert_eq!((cut.output.as_str(), cut.next_offset), ("héllo", 16), "a character cut off waits to be whole");
    }
}
