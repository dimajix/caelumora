//! User commands (issue 28): `ChannelCommandProvider` components contribute
//! commands, and a session's `CommandExecutor` finds and runs them on behalf
//! of a user. `AgentCommandProvider` components of a personality contribute
//! the commands an agent carries out itself (issue 269), which its actor's
//! executor finds and the agent runs in a turn of its own.
//!
//! Commands are for people - agents act through tools. They are issued
//! through `UserActor::execute_command`, on behalf of the user the actor
//! stands for, and never enter the conversation.
//!
//! The built-in command components live here too, one file per command.

pub mod help_command;
pub mod nudge_command;
pub mod wakeup_command;

use crate::actor::user::UserActor;
use crate::agent::turn::Turn;
use crate::channel::{ChannelLoop, ChannelMember};
use crate::component::{ComponentError, ComponentImpl, ComponentRegistry, ComponentResult};
use async_trait::async_trait;
use caelumora_model::command::{Command, CommandOutput, CommandVisibility};
use caelumora_model::security::{ActionType, Enforcement, Refused, SecurityAttributes, SecurityContext, SecurityManager};
use std::fmt::Debug;
use std::sync::Arc;

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
    #[error("You are not permitted to run /{name}.")]
    Forbidden { name: String, #[source] refused: Refused },
    #[error("/{command}: {reason}")]
    InvalidArguments { command: String, reason: String },
    #[error("/{command} failed: {reason}")]
    Failed { command: String, reason: String },
}

pub type CommandResult<T> = Result<T, CommandError>;

/// What issuing a command answered at once (see
/// `UserActor::execute_command`).
#[derive(Debug, Clone, PartialEq)]
pub struct CommandReply {
    /// The command's name, without the slash.
    pub command: String,
    /// What it answered: the answer of a command of the channel; for one
    /// sent to agents, which of them were not asked, if any were left out.
    pub output: Option<String>,
    /// `Channel` if the answer was also announced to the channel as a
    /// `CommandExecuted`. The issuer gets it here either way: the channel
    /// does not route an envelope back to its sender.
    pub visibility: CommandVisibility,
    /// For a command sent to agents (issue 269), the span of the requests:
    /// their answers come later, as `CommandExecuted` or `CommandFailed`
    /// on the issuer's stream, in this span.
    pub span_id: Option<uuid::Uuid>,
}

/// What a command of the channel runs with: who issued it, the channel,
/// and the session's other commands. Deliberately no `Turn` - such a
/// command runs outside of any agent's turn, often because none is
/// running.
pub struct CommandContext<'a> {
    /// The issuing user's actor in the channel: their security context, and
    /// what a command sends in their name it sends through it - an actor
    /// sends only in its own name (issue 228).
    pub issuer: &'a UserActor,
    pub channel: &'a ChannelLoop,
    pub executor: &'a CommandExecutor<dyn ChannelCommandProvider>,
}

/// What every facet contributing commands says of them, whoever runs them:
/// the part of a provider the `CommandExecutor` needs to find a command.
pub trait ProvidesCommands: Send + Sync + Debug {
    /// The commands this component provides.
    fn commands(&self) -> Vec<Command>;
}

/// A component facet contributing user commands run on the channel -
/// `/help`, `/wakeup` -, as `ToolProvider` contributes tools: given the
/// channel, not an agent's turn.
#[async_trait]
pub trait ChannelCommandProvider: ProvidesCommands {
    /// Runs the command `name` with `args`, the rest of the input line after
    /// the name, which the command parses itself.
    ///
    /// # Errors
    /// `CommandError::InvalidArguments` for arguments it cannot use,
    /// `CommandError::Forbidden` if the action it performs is refused, and
    /// `CommandError::Failed` otherwise.
    async fn execute(&self, name: &str, args: &str, context: &CommandContext<'_>) -> CommandResult<CommandOutput>;
}

/// A component facet contributing commands an agent carries out itself
/// (issue 269) - `/skill`, `/compact` -, of agents run here and elsewhere
/// alike: given the turn the agent carries the command out in, as a tool is
/// given the turn it is called in. What the command changes it stages on
/// the turn, which is taken over when the command succeeded and dropped
/// when it did not. Whom it was asked by is the turn's initiator.
#[async_trait]
pub trait AgentCommandProvider: ProvidesCommands {
    /// Carries out the command `name` with `args`, the rest of the input
    /// line after the name and the agent's handle, which the command parses
    /// itself.
    ///
    /// # Errors
    /// As `ChannelCommandProvider::execute`.
    async fn execute(&self, name: &str, args: &str, turn: &mut Turn<'_>) -> CommandResult<CommandOutput>;
}

#[derive(Debug)]
struct CommandEntry<P: ?Sized> {
    command: Command,
    component: String,
    provider: Arc<P>,
}

/// Finds and runs the commands of one set - the session's, from the global
/// ones and the session's own - with the permission checks for seeing and
/// running them, as the `ToolExecutor` does for an agent's tools. Like it,
/// it is no component: how a command runs is the command's own business,
/// and there is nothing about running them to try out. Generic over the
/// facet that provides the commands, since what a
/// command is given to run with is the facet's; finding one, refusing two
/// of one name and asking the policy are the same for every facet.
#[derive(Debug)]
pub struct CommandExecutor<P: ?Sized> {
    security_manager: Arc<dyn SecurityManager>,
    entries: Vec<CommandEntry<P>>,
}

impl<P: ?Sized + ProvidesCommands> CommandExecutor<P> {
    /// An executor for the commands `providers` contribute. Building them is
    /// up to whoever owns their configuration - for a session's commands,
    /// `RuntimeManager::new_command_executor`.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if two of them provide the same name:
    /// that is a mistake in the configuration, and resolving it by order
    /// would silently hide one.
    pub fn new(security_manager: Arc<dyn SecurityManager>, providers: Vec<ComponentImpl<P>>) -> ComponentResult<Self> {
        let mut entries: Vec<CommandEntry<P>> = Vec::new();
        for provider in providers {
            for command in provider.commands() {
                if let Some(existing) = entries.iter().find(|e| e.command.name == command.name) {
                    return Err(ComponentError::InvalidConfig(format!(
                        "command /{} is provided by both component '{}' and component '{}'",
                        command.name, existing.component, provider.id
                    )));
                }
                entries.push(CommandEntry { command, component: provider.id.clone(), provider: Arc::clone(&provider.facet) });
            }
        }
        Ok(Self { security_manager, entries })
    }

    /// An executor without commands.
    pub fn empty(security_manager: Arc<dyn SecurityManager>) -> Self {
        Self { security_manager, entries: Vec::new() }
    }

    /// Whether it has a command `name`, whoever may see it.
    pub fn has(&self, name: &str) -> bool {
        self.entries.iter().any(|e| e.command.name == name)
    }

    /// The commands `issuer` may see - those `show` of them is granted -,
    /// sorted by name: the order components are built in follows their
    /// dependencies, not the configuration, so it is no order to show.
    async fn visible_by<'c>(&'c self, issuer: &SecurityContext, show: impl Fn(&'c Command) -> ActionType<'c>) -> Vec<Command> {
        let mut visible = Vec::new();
        for entry in &self.entries {
            if self.security_manager.check(issuer, show(&entry.command)).await {
                visible.push(entry.command.clone());
            }
        }
        visible.sort_by(|l, r| l.name.cmp(&r.name));
        visible
    }

    /// The command `name`, if `issuer` may run it: `Empty` for an empty
    /// name; `Unknown` if there is no such command or `show` of it is not
    /// granted them - one they may not see is unknown to them, not refused;
    /// `Forbidden` if `execute` of it is not.
    async fn admit<'c>(
        &'c self,
        name: &str,
        issuer: &SecurityContext,
        show: impl Fn(&'c Command) -> ActionType<'c>,
        execute: impl Fn(&'c Command) -> ActionType<'c>,
    ) -> CommandResult<&'c CommandEntry<P>> {
        if name.is_empty() {
            return Err(CommandError::Empty);
        }
        let entry = self.entries.iter().find(|e| e.command.name == name);
        let Some(entry) = entry else {
            return Err(CommandError::Unknown(name.to_string()));
        };
        self.security_manager.require(issuer, show(&entry.command), |_| CommandError::Unknown(name.to_string())).await?;
        self.security_manager.require(issuer, execute(&entry.command), |refused| CommandError::Forbidden { name: name.to_string(), refused }).await?;
        Ok(entry)
    }
}

/// The commands of the channel - the session's set, run in each of its
/// channels: seen and run by `ShowCommand` and `ExecuteCommand`, with the
/// attributes of the channel they are run in - as the commands of an agent
/// are, with the agent's besides (issue 304).
impl CommandExecutor<dyn ChannelCommandProvider> {
    /// The commands `issuer` may see in the channel with `channel`
    /// attributes (`ShowCommand`), sorted by name.
    pub async fn visible(&self, issuer: &SecurityContext, channel: &SecurityAttributes) -> Vec<Command> {
        self.visible_by(issuer, |command| ActionType::ShowCommand { command, channel }).await
    }

    /// Runs the command `name` for `context.issuer`.
    ///
    /// # Errors
    /// `CommandError::Empty` for an empty name; `Unknown` if there is no such
    /// command or the issuer may not see it; `Forbidden` if they may see but
    /// not run it; otherwise whatever the command reports.
    pub async fn execute(&self, name: &str, args: &str, context: &CommandContext<'_>) -> CommandResult<CommandOutput> {
        let channel = context.channel.security_attributes();
        let entry = self.admit(
            name,
            &context.issuer.security_context(),
            |command| ActionType::ShowCommand { command, channel: &channel },
            |command| ActionType::ExecuteCommand { command, channel: &channel },
        ).await?;
        entry.provider.execute(name, args, context).await
    }
}

/// The commands of an agent (issue 269): seen and asked for by
/// `ShowAgentCommand` and `ExecuteAgentCommand`, with the agent's
/// attributes and those of the channel it is asked in.
impl CommandExecutor<dyn AgentCommandProvider> {
    /// The commands `issuer` may see of the agent with `agent` attributes in
    /// the channel with `channel` attributes, sorted by name.
    pub async fn visible(&self, issuer: &SecurityContext, agent: &SecurityAttributes, channel: &SecurityAttributes) -> Vec<Command> {
        self.visible_by(issuer, |command| ActionType::ShowAgentCommand { command, agent, channel }).await
    }

    /// The command `name`, if `issuer` - who asked, in the context the
    /// request came with - may ask the agent with `agent` attributes for it
    /// in the channel with `channel` attributes: ready to be carried out,
    /// and only so.
    ///
    /// # Errors
    /// `CommandError::Empty` for an empty name; `Unknown` if the agent has
    /// no such command or the issuer may not see it; `Forbidden` if they
    /// may see but not ask for it.
    pub async fn admitted<'c>(&'c self, name: &str, issuer: &SecurityContext, agent: &'c SecurityAttributes, channel: &'c SecurityAttributes) -> CommandResult<AdmittedCommand<'c>> {
        let entry = self.admit(
            name,
            issuer,
            |command| ActionType::ShowAgentCommand { command, agent, channel },
            |command| ActionType::ExecuteAgentCommand { command, agent, channel },
        ).await?;
        Ok(AdmittedCommand(entry))
    }
}

/// A command of an agent its issuer may ask for (`CommandExecutor::admitted`):
/// what it is, and carrying it out - which can only be done once it was
/// admitted, so that the agent can tell the issuer it began in between
/// (issue 299).
#[derive(Debug)]
pub struct AdmittedCommand<'c>(&'c CommandEntry<dyn AgentCommandProvider>);

impl AdmittedCommand<'_> {
    pub fn command(&self) -> &Command {
        &self.0.command
    }

    /// Carries the command out with `args` in `turn`.
    ///
    /// # Errors
    /// Whatever the command reports.
    pub async fn execute(self, args: &str, turn: &mut Turn<'_>) -> CommandResult<CommandOutput> {
        self.0.provider.execute(&self.0.command.name, args, turn).await
    }
}

/// The participant of `channel` - agent, user or any other actor - that the
/// argument `wanted` of the command `command` names
/// (`ChannelLoop::participants_named`, issues 277, 278): the one way the
/// commands of the channel that take someone, `/wakeup` and `/nudge`, find
/// them and word that they did not.
///
/// # Errors
/// `CommandError::InvalidArguments` naming the handles of the participants,
/// if `wanted` names none, or those it names, if it names several: nobody
/// is picked.
pub(crate) fn named_participant(command: &str, wanted: &str, channel: &ChannelLoop) -> CommandResult<ChannelMember> {
    let invalid = |reason: String| CommandError::InvalidArguments { command: command.to_string(), reason };
    match channel.participants_named(wanted).as_slice() {
        [one] => Ok(one.clone()),
        [] => {
            let mut handles: Vec<String> = channel.members().into_iter().filter_map(|m| m.handle).map(|h| format!("@{h}")).collect();
            handles.sort();
            Err(invalid(format!("nobody '{}' in this channel; the participants are {}", wanted, names(&handles))))
        }
        several => {
            let several: Vec<String> = several.iter().map(|m| m.name.to_string()).collect();
            Err(invalid(format!("'{}' names several participants: {}", wanted, names(&several))))
        }
    }
}

/// `names` joined as a sentence: "A, B and C".
pub(crate) fn names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {}", init.join(", "), last),
    }
}

/// Registers the command kinds with `registry`.
pub fn register_all(registry: &mut ComponentRegistry) {
    help_command::register(registry);
    nudge_command::register(registry);
    wakeup_command::register(registry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::component::ComponentFailPolicy;
    use caelumora_model::security::DummySecurityManager;

    #[derive(Debug)]
    struct Named(&'static str);

    impl ProvidesCommands for Named {
        fn commands(&self) -> Vec<Command> {
            vec![Command { name: self.0.to_string(), description: String::new(), usage: None, owners_only: false }]
        }
    }

    #[async_trait]
    impl ChannelCommandProvider for Named {
        async fn execute(&self, _name: &str, _args: &str, _context: &CommandContext<'_>) -> CommandResult<CommandOutput> {
            Ok(CommandOutput::private(""))
        }
    }

    fn provider(id: &str, command: &'static str) -> ComponentImpl<dyn ChannelCommandProvider> {
        ComponentImpl { id: id.to_string(), fail_policy: ComponentFailPolicy::FailOpen, facet: Arc::new(Named(command)) }
    }

    #[tokio::test]
    async fn two_components_with_the_same_command_are_a_configuration_error() {
        let security = Arc::new(DummySecurityManager::new());
        let error = CommandExecutor::new(Arc::<DummySecurityManager>::clone(&security), vec![provider("help", "help"), provider("my-help", "help")])
            .expect_err("the second /help would be hidden by the first");
        let message = error.to_string();
        assert!(message.contains("/help") && message.contains("'help'") && message.contains("'my-help'"), "{message}");

        let executor = CommandExecutor::new(security, vec![provider("wakeup", "wakeup"), provider("help", "help")]).unwrap();
        let names: Vec<String> = executor.visible(&SecurityContext::default(), &SecurityAttributes::default()).await.into_iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["help", "wakeup"], "sorted by name");
    }

    #[test]
    fn names_are_joined_as_a_sentence() {
        assert_eq!(names(&[]), "");
        assert_eq!(names(&["Sassy".to_string()]), "Sassy");
        assert_eq!(names(&["A".to_string(), "B".to_string(), "C".to_string()]), "A, B and C");
    }
}
