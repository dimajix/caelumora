//! The commands of the workspaces of one storage (issue 254): each by its
//! workspace, the scope it was started with and its id. A storage's shells
//! enter a command as they start it, so that none is started past it, and
//! its provider lists those that may still run - for the runtime, which
//! ends what may no longer run where it was started. What a shell keeps
//! of a command besides is its own (`C`).
//!
//! A command is kept while it may run and for `KEPT_AFTER_END` after it
//! ended, so that its output can still be asked for - as Open Terminal
//! keeps its processes.
//!
//! `Commands` and `Kept` are public, so that a provider outside the core
//! keeps its commands as the core's do (issue 327).

use super::{TerminalError, TerminalResult};
use caelumora_model::workspace::WorkspaceId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How long a command is kept after it ended.
pub(super) const KEPT_AFTER_END: Duration = Duration::from_secs(300);

/// A command, as its storage knows it: its workspace, the scope it was
/// started with, its id.
pub type CommandKey = (WorkspaceId, String, String);

/// A lock of these few, never held across an await, taken also if a holder
/// panicked: nothing under it is left half changed.
pub(super) fn locked<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What a shell keeps of a command: when it ended, as far as it knows.
pub trait Kept {
    /// When it ended; `None` while it may run.
    fn ended(&self) -> Option<Instant>;
}

/// A command shared between the storage and the shells that follow it.
impl<T: Kept + ?Sized> Kept for Arc<T> {
    fn ended(&self) -> Option<Instant> {
        T::ended(self)
    }
}

/// The commands of the workspaces of one storage.
#[derive(Debug)]
pub struct Commands<C> {
    commands: Mutex<HashMap<CommandKey, C>>,
}

impl<C> Default for Commands<C> {
    fn default() -> Self {
        Self { commands: Mutex::default() }
    }
}

impl<C: Kept + Clone> Commands<C> {
    /// The commands, those ended longer ago than `KEPT_AFTER_END` forgotten.
    fn kept(&self) -> MutexGuard<'_, HashMap<CommandKey, C>> {
        let mut commands = locked(&self.commands);
        commands.retain(|_, command| command.ended().is_none_or(|at| at.elapsed() < KEPT_AFTER_END));
        commands
    }

    pub fn insert(&self, key: CommandKey, command: C) {
        self.kept().insert(key, command);
    }

    /// Command `key`.
    ///
    /// # Errors
    /// `UnknownCommand` if it was never started, or ended a while ago.
    pub fn get(&self, key: &CommandKey) -> TerminalResult<C> {
        self.kept().get(key).cloned().ok_or_else(|| TerminalError::UnknownCommand(key.2.clone()))
    }

    /// The commands that may still run.
    pub fn running(&self) -> Vec<CommandKey> {
        self.kept().iter().filter(|(_, command)| command.ended().is_none()).map(|(key, _)| key.clone()).collect()
    }

    /// Forgets the commands of workspace `id`: they ended with its place.
    pub fn forget(&self, id: WorkspaceId) {
        self.kept().retain(|(workspace, _, _), _| *workspace != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default)]
    struct Fake(Mutex<Option<Instant>>);

    impl Kept for Fake {
        fn ended(&self) -> Option<Instant> {
            *locked(&self.0)
        }
    }

    fn key(workspace: WorkspaceId, id: &str) -> CommandKey {
        (workspace, "channel".to_string(), id.to_string())
    }

    #[test]
    fn a_command_is_listed_while_it_may_run_and_kept_a_while_after() {
        let commands = Commands::default();
        let (workspace, other) = (WorkspaceId::new(), WorkspaceId::new());
        let (running, ended, long_ended) = (Arc::new(Fake::default()), Arc::new(Fake::default()), Arc::new(Fake::default()));
        commands.insert(key(workspace, "running"), Arc::clone(&running));
        commands.insert(key(workspace, "ended"), Arc::clone(&ended));
        commands.insert(key(other, "long ended"), Arc::clone(&long_ended));
        *locked(&ended.0) = Some(Instant::now());
        *locked(&long_ended.0) = Instant::now().checked_sub(KEPT_AFTER_END);

        assert_eq!(commands.running(), [key(workspace, "running")], "only what may still run");
        assert!(commands.get(&key(workspace, "ended")).is_ok(), "kept a while after it ended");
        assert!(matches!(commands.get(&key(other, "long ended")), Err(TerminalError::UnknownCommand(id)) if id == "long ended"), "forgotten after that");
        assert!(matches!(commands.get(&(workspace, "elsewhere".to_string(), "running".to_string())), Err(TerminalError::UnknownCommand(_))), "under its scope alone");

        commands.forget(workspace);
        assert!(commands.running().is_empty() && commands.get(&key(workspace, "ended")).is_err(), "gone with its workspace");
    }
}
