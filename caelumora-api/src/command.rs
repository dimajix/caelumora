//! What an agent's commands are made of (issues 28, 269): the commands a component provides
//! (`ProvidesCommands`), the set they are found in (`CommandSet`), and how one fails
//! (`CommandError`); how a session's commands are run is the kernel's
//! (`caelumora_kernel::command`).

use crate::component::{ComponentError, ComponentImpl, ComponentResult};
use caelumora_model::command::Command;
use caelumora_model::security::Refused;
use std::fmt::Debug;
use std::sync::Arc;

/// Why a command did not run. The messages are meant for the user who
/// issued it (see `crate::error::user_message_with`).
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("No command given. Type /help for the list of commands, or start with // to send a message that begins with a slash.")]
    Empty,
    /// Also the answer for a command the user may not see, so that hidden
    /// commands cannot be found by trying names.
    #[error("Unknown command /{0}. Type /help for the list of commands.")]
    Unknown(String),
    #[error("You are not permitted to run /{name}.")]
    Forbidden {
        name: String,
        #[source]
        refused: Refused,
    },
    #[error("/{command}: {reason}")]
    InvalidArguments { command: String, reason: String },
    #[error("/{command} failed: {reason}")]
    Failed { command: String, reason: String },
}

pub type CommandResult<T> = Result<T, CommandError>;

/// What every facet contributing commands says of them, whoever runs them:
/// the part of a provider the `CommandExecutor` needs to find a command.
pub trait ProvidesCommands: Send + Sync + Debug {
    /// The commands this component provides.
    fn commands(&self) -> Vec<Command>;
}

/// A command of a set, with the component that provides it and the facet
/// that carries it out.
#[derive(Debug)]
pub struct CommandEntry<P: ?Sized> {
    command: Command,
    component: String,
    provider: Arc<P>,
}

impl<P: ?Sized> CommandEntry<P> {
    /// The command, as its component provides it.
    pub fn command(&self) -> &Command {
        &self.command
    }

    /// The facet that carries it out - to be called once the command was
    /// admitted, by whoever runs it.
    pub fn provider(&self) -> &Arc<P> {
        &self.provider
    }
}

/// The commands one set of providers contributes - the session's, from the
/// global ones and the session's own; an agent's (`caelumora_agents::command`,
/// issue 370) -, each found by its name, no two of one name. Generic over the
/// facet that provides the commands, since what a command is given to run
/// with is the facet's; finding one and refusing two of one name are the same
/// for every facet. Who may see and run one is asked by whoever runs it: the
/// `CommandExecutor` for the channel's, the channel's door for an agent's
/// (issue 516).
#[derive(Debug)]
pub struct CommandSet<P: ?Sized> {
    entries: Vec<CommandEntry<P>>,
}

impl<P: ?Sized + ProvidesCommands> CommandSet<P> {
    /// The commands `providers` contribute.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if two of them provide the same name:
    /// that is a mistake in the configuration, and resolving it by order
    /// would silently hide one.
    pub fn new(providers: Vec<ComponentImpl<P>>) -> ComponentResult<Self> {
        let mut entries: Vec<CommandEntry<P>> = Vec::new();
        for provider in providers {
            for command in provider.commands() {
                if let Some(existing) = entries.iter().find(|e| e.command.name == command.name) {
                    return Err(ComponentError::InvalidConfig(format!(
                        "command /{} is provided by both component '{}' and component '{}'",
                        command.name, existing.component, provider.id
                    )));
                }
                entries.push(CommandEntry {
                    command,
                    component: provider.id.clone(),
                    provider: Arc::clone(&provider.facet),
                });
            }
        }
        Ok(Self { entries })
    }

    /// A set without commands.
    pub fn empty() -> Self {
        Self { entries: Vec::new() }
    }

    /// Whether it has a command `name`.
    pub fn has(&self, name: &str) -> bool {
        self.entry(name).is_some()
    }

    /// The command `name` with what carries it out, if the set has it.
    pub fn entry(&self, name: &str) -> Option<&CommandEntry<P>> {
        self.entries.iter().find(|e| e.command.name == name)
    }

    /// The commands, sorted by name: the order components are built in
    /// follows their dependencies, not the configuration, so it is no order
    /// to show.
    pub fn commands(&self) -> Vec<Command> {
        let mut commands: Vec<Command> = self.entries.iter().map(|e| e.command.clone()).collect();
        commands.sort_by(|l, r| l.name.cmp(&r.name));
        commands
    }
}
