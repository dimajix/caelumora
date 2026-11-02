//! How a workspace is reached (issue 251, part A): every workspace lies in a storage, served by one
//! `TerminalProvider`, which makes and removes the place of a workspace there and hands out its
//! `Terminal` - the one handle through which its files are read and written, writing no file larger
//! than its storage takes (`max_file_bytes`, a parameter of the storage). What only reads holds its
//! reading part, a `TerminalReader`, which changes nothing (issue 516). Whatever works on a
//! workspace - the panel, uploads and downloads, the tools of agents - holds one of them; nothing
//! else reaches the files.
//!
//! What a caller writes as a path is made a `WorkspacePath` before any provider sees it: relative,
//! without `..`, so that no provider is ever handed one that leaves the workspace. An error never
//! names more of a path than the caller wrote.
//!
//! A storage may have a `Shell`, a capability its provider hands out for a scope
//! (`TerminalProvider::shell_for`), not its terminal: commands run in the workspace's directory,
//! each seen only under the scope it was started with - a channel - and kept a while after it
//! ended. A storage knows its commands (`commands.rs`, issue 254): its provider lists those that
//! may still run and ends one when told, and none runs longer than the storage's `max_run`.
//!
//! A storage is a backend (issue 324): its provider is made by the factory registered for its
//! type - `local` here (`register_backends`), others by a plugin, such as the Open Terminal
//! storages of `caelumora-open-terminal` (issue 327) -, from its parameters, which that factory
//! reads and checks. What a provider outside the kernel has to offer is this module's public API: a
//! `TerminalProvider` with its `Terminal`, `TerminalReader` and `Shell` - the handles it hands out,
//! which are the API's (`caelumora_api::terminal`, issue 516) -, the commands of its storage kept
//! in `Commands`, and its `max_run_secs` and `max_file_bytes` checked as `check_max_run_secs` and
//! `check_max_file_bytes` check them.

mod commands;
mod local;
mod local_shell;

pub use commands::{CommandKey, Commands, Kept};
pub use local::{write_whole, WriteFailure};

use crate::backend::Backends;
use async_trait::async_trait;
use caelumora_api::component::{ComponentError, ComponentResult};
use caelumora_api::terminal::{Shell, Terminal, TerminalResult};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::config::DEFAULT_MAX_FILE_BYTES;
use caelumora_domain::workspace::WorkspaceId;
use std::fmt::Debug;
use std::sync::Arc;

/// What makes and reaches the places of the workspaces of one storage
/// (issue 251, part A): the service a storage of workspaces is a backend
/// of (issue 324).
#[async_trait]
pub trait TerminalProvider: Send + Sync + Debug {
    /// Makes the place of the workspace `id`: empty.
    async fn create(&self, id: WorkspaceId) -> TerminalResult<()>;
    /// Removes the place of the workspace `id`, with everything in it - the
    /// commands running there ended first (issue 254); nothing there is no
    /// error.
    async fn remove(&self, id: WorkspaceId) -> TerminalResult<()>;
    /// The terminal of the workspace `id`, which writes no file larger than the storage takes.
    async fn terminal_for(&self, id: WorkspaceId) -> TerminalResult<Arc<dyn Terminal>>;
    /// The shell of the workspace `id` as `scope` sees it - the commands started under that scope
    /// alone (issue 296) -, `None` where the storage has none.
    async fn shell_for(
        &self,
        id: WorkspaceId,
        scope: &str,
    ) -> TerminalResult<Option<Arc<dyn Shell>>>;
    /// Whether its storage has a shell - asked without reaching one.
    fn has_shell(&self) -> bool;
    /// The commands of its workspaces that may still run (issue 254) -
    /// listed without reaching any place.
    fn commands(&self) -> Vec<CommandKey>;
    /// Ends command `key` with everything it started, without asking anything (issue 254): for what
    /// the kernel decided may no longer run. One that ended meanwhile, or is no longer known, is no
    /// error; a place that is not running is not started for it.
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
    /// Ends, as the process ends (issue 297), what it ran that does not
    /// outlive the process - the `local` shell's commands, whose processes
    /// nothing would know of after a restart. A storage whose commands
    /// outlive the process keeps them, as an Open Terminal instance does:
    /// the model can follow them by their id after a restart, and their
    /// bound ends them.
    async fn shutdown(&self) {}
    /// What may be put into this storage: the classification a workspace
    /// in it may have at most; `None` for no bound (issue 251, part B2).
    fn destination(&self) -> Option<&Classification>;
}

/// How long a command in a workspace may run unless its storage says
/// otherwise (issue 254): an hour.
pub const DEFAULT_MAX_RUN_SECS: u64 = 3600;

/// The `max_file_bytes` of a storage that names none - the largest file a person may put into a
/// workspace of it -, for its parameters' `#[serde(default = ...)]`.
pub fn default_max_file_bytes() -> u64 {
    DEFAULT_MAX_FILE_BYTES
}

/// Checks `bytes`, a storage's `max_file_bytes`, for the `Params::check` of its parameters.
///
/// # Errors
/// `ComponentError::InvalidConfig` for none at all.
pub fn check_max_file_bytes(bytes: u64) -> ComponentResult<()> {
    if bytes == 0 {
        return Err(ComponentError::InvalidConfig(
            "max_file_bytes must be more than 0".to_string(),
        ));
    }
    Ok(())
}

/// The `max_run_secs` of a storage that names none - for its parameters'
/// `#[serde(default = ...)]`.
pub fn default_max_run_secs() -> u64 {
    DEFAULT_MAX_RUN_SECS
}

/// Checks `secs`, a storage's `max_run_secs` - how long a command in one of its workspaces may run
/// (issue 254) -, for the `Params::check` of its parameters.
///
/// # Errors
/// `ComponentError::InvalidConfig` for none at all.
pub fn check_max_run_secs(secs: u64) -> ComponentResult<()> {
    if secs == 0 {
        return Err(ComponentError::InvalidConfig("max_run_secs must be more than 0".to_string()));
    }
    Ok(())
}

/// Registers the storage of workspaces the kernel brings with `backends`: `local`.
pub(crate) fn register_backends(backends: &mut Backends) {
    local::register_backend(backends);
}
