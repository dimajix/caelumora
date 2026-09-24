//! User commands (issue 28): `CommandProvider` components contribute
//! commands, and a session's `CommandExecutor` finds and runs them on behalf of a
//! user.
//!
//! Commands are for people - agents act through tools. They are run by
//! `UserActor::execute_command`, on behalf of the user the actor stands
//! for, and never enter the conversation.

use crate::channel::ChannelLoop;
use crate::component::{ComponentError, ComponentImpl, ComponentResult};
use async_trait::async_trait;
use caelumora_model::command::{Command, CommandOutput, CommandVisibility};
use caelumora_model::security::{ActionType, SecurityContext, SecurityManager};
use std::fmt::Debug;
use std::sync::Arc;
use tracing::info;

/// Why a command did not run. The messages are meant for the user who
/// issued it (see `crate::error::user_message`).
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("No command given. Type /help for the list of commands, or start with // to send a message that begins with a slash.")]
    Empty,
    /// Also the answer for a command the user may not see, so that hidden
    /// commands cannot be found by trying names.
    #[error("Unknown command /{0}. Type /help for the list of commands.")]
    Unknown(String),
    #[error("You are not permitted to run /{0}.")]
    Forbidden(String),
    #[error("/{command}: {reason}")]
    InvalidArguments { command: String, reason: String },
    #[error("/{command} failed: {reason}")]
    Failed { command: String, reason: String },
}

pub type CommandResult<T> = Result<T, CommandError>;

/// What running a command answered (see `UserActor::execute_command`).
#[derive(Debug, Clone, PartialEq)]
pub struct CommandReply {
    /// The command's name, without the slash.
    pub command: String,
    pub output: String,
    /// `Channel` if the answer was also announced to the channel as a
    /// `CommandExecuted`. The issuer gets it here either way: the channel
    /// does not route an envelope back to its sender.
    pub visibility: CommandVisibility,
}

/// What a command runs with: who issued it, in which channel, and the
/// session's other commands. Deliberately no `TurnContext` - a command runs
/// outside of any agent's turn, often because none is running.
pub struct CommandContext<'a> {
    /// The issuing user's security context.
    pub issuer: &'a SecurityContext,
    pub channel: &'a ChannelLoop,
    pub executor: &'a CommandExecutor,
}

/// A component facet contributing user commands, as `ToolProvider`
/// contributes tools.
#[async_trait]
pub trait CommandProvider: Send + Sync + Debug {
    /// The commands this component provides.
    fn commands(&self) -> Vec<Command>;

    /// Runs the command `name` with `args`, the rest of the input line after
    /// the name, which the command parses itself.
    ///
    /// # Errors
    /// `CommandError::InvalidArguments` for arguments it cannot use,
    /// `CommandError::Forbidden` if the action it performs is refused, and
    /// `CommandError::Failed` otherwise.
    async fn execute(&self, name: &str, args: &str, context: &CommandContext<'_>) -> CommandResult<CommandOutput>;
}

#[derive(Debug)]
struct CommandEntry {
    command: Command,
    component: String,
    provider: Arc<dyn CommandProvider>,
}

/// Finds and runs the commands of one session - the global ones and the
/// session's own - with the permission checks for seeing and running them,
/// as `ToolExecutor` does for an agent's tools.
#[derive(Debug)]
pub struct CommandExecutor {
    security_manager: Arc<dyn SecurityManager>,
    entries: Vec<CommandEntry>,
}

impl CommandExecutor {
    /// An executor for the commands `providers` contribute. Building them is
    /// up to whoever owns their configuration - for a session's commands,
    /// `RuntimeManager::new_command_executor`.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if two of them provide the same name:
    /// that is a mistake in the configuration, and resolving it by order
    /// would silently hide one.
    pub fn new(security_manager: Arc<dyn SecurityManager>, providers: Vec<ComponentImpl<dyn CommandProvider>>) -> ComponentResult<Self> {
        let mut entries: Vec<CommandEntry> = Vec::new();
        for provider in providers {
            for command in provider.facet.commands() {
                if let Some(existing) = entries.iter().find(|e| e.command.name == command.name) {
                    return Err(ComponentError::InvalidConfig(format!(
                        "command /{} is provided by both component '{}' and component '{}'",
                        command.name, existing.component, provider.id
                    )));
                }
                entries.push(CommandEntry { command, component: provider.id.clone(), provider: provider.facet.clone() });
            }
        }
        Ok(Self { security_manager, entries })
    }

    /// An executor without commands.
    pub fn empty(security_manager: Arc<dyn SecurityManager>) -> Self {
        Self { security_manager, entries: Vec::new() }
    }

    /// The commands `issuer` may see (`ShowCommand`), sorted by name - the
    /// order components are built in follows their dependencies, not the
    /// configuration, so it is no order to show.
    pub async fn visible(&self, issuer: &SecurityContext) -> Vec<Command> {
        let mut visible = Vec::new();
        for entry in &self.entries {
            if self.security_manager.check(issuer, ActionType::ShowCommand(&entry.command.name)).await {
                visible.push(entry.command.clone());
            }
        }
        visible.sort_by(|l, r| l.name.cmp(&r.name));
        visible
    }

    /// Runs the command `name` for `context.issuer`.
    ///
    /// # Errors
    /// `CommandError::Empty` for an empty name; `Unknown` if there is no such
    /// command or the issuer may not see it; `Forbidden` if they may see but
    /// not run it; otherwise whatever the command reports.
    pub async fn execute(&self, name: &str, args: &str, context: &CommandContext<'_>) -> CommandResult<CommandOutput> {
        if name.is_empty() {
            return Err(CommandError::Empty);
        }
        let entry = self.entries.iter().find(|e| e.command.name == name);
        let Some(entry) = entry else {
            return Err(CommandError::Unknown(name.to_string()));
        };
        if !self.security_manager.check(context.issuer, ActionType::ShowCommand(name)).await {
            return Err(CommandError::Unknown(name.to_string()));
        }
        if !self.security_manager.check(context.issuer, ActionType::ExecuteCommand(name)).await {
            info!("Refused command /{} for {}", name, context.issuer.initiator_id);
            return Err(CommandError::Forbidden(name.to_string()));
        }
        entry.provider.execute(name, args, context).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::component::ComponentFailPolicy;
    use caelumora_model::security::DummySecurityManager;

    #[derive(Debug)]
    struct Named(&'static str);

    #[async_trait]
    impl CommandProvider for Named {
        fn commands(&self) -> Vec<Command> {
            vec![Command { name: self.0.to_string(), description: String::new(), usage: None }]
        }

        async fn execute(&self, _name: &str, _args: &str, _context: &CommandContext<'_>) -> CommandResult<CommandOutput> {
            Ok(CommandOutput::private(""))
        }
    }

    fn provider(id: &str, command: &'static str) -> ComponentImpl<dyn CommandProvider> {
        ComponentImpl { id: id.to_string(), fail_policy: ComponentFailPolicy::FailOpen, facet: Arc::new(Named(command)) }
    }

    #[tokio::test]
    async fn two_components_with_the_same_command_are_a_configuration_error() {
        let security = Arc::new(DummySecurityManager::new());
        let error = CommandExecutor::new(security.clone(), vec![provider("help", "help"), provider("my-help", "help")])
            .expect_err("the second /help would be hidden by the first");
        let message = error.to_string();
        assert!(message.contains("/help") && message.contains("'help'") && message.contains("'my-help'"), "{message}");

        let executor = CommandExecutor::new(security, vec![provider("wakeup", "wakeup"), provider("help", "help")]).unwrap();
        let names: Vec<String> = executor.visible(&SecurityContext::default()).await.into_iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["help", "wakeup"], "sorted by name");
    }
}
