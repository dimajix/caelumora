//! What an agent's commands are made of (issues 28, 269): the commands a component provides
//! (`ProvidesCommands`), the set they are found in (`CommandSet`), and how one fails
//! (`CommandError`); how a session's commands are run is the kernel's
//! (`caelumora_kernel::command`).

use crate::component::{ComponentError, ComponentImpl, ComponentResult};
use caelumora_domain::command::Command;
use caelumora_domain::security::Refused;
use caelumora_domain::value::command_name::CommandName;
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

/// The commands one set of providers contributes - the session's, from the global ones and the
/// session's own; an agent's (`caelumora_agents::command`, issue 370) -, each found by the line it
/// begins (issue 560), no two that a line could not tell apart. Generic over the facet that
/// provides the commands, since what a command is given to run with is the facet's; finding one and
/// refusing two that collide are the same for every facet. Who may see and run one is asked by
/// whoever runs it: the `CommandExecutor` for the channel's, the channel's door for an agent's
/// (issue 516).
#[derive(Debug)]
pub struct CommandSet<P: ?Sized> {
    entries: Vec<CommandEntry<P>>,
}

impl<P: ?Sized + ProvidesCommands> CommandSet<P> {
    /// The commands `providers` contribute.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if a name or alias of them is no command name - one taken
    /// unchecked (`CommandName::fixed`) -, if two names of one of them begin one another, or if
    /// two of them collide (`Command::collides_with`): that is a mistake in the configuration, and
    /// resolving it by order or by the longer name would silently hide one.
    pub fn new(providers: Vec<ComponentImpl<P>>) -> ComponentResult<Self> {
        let mut entries: Vec<CommandEntry<P>> = Vec::new();
        for provider in providers {
            for command in provider.commands() {
                if let Some(name) =
                    command.names().find(|name| CommandName::new(name.as_str()).is_err())
                {
                    return Err(ComponentError::InvalidConfig(format!(
                        "component '{}' provides a command named '{name}', which is no command name",
                        provider.id
                    )));
                }
                let names: Vec<&CommandName> = command.names().collect();
                if let Some((_, name)) = names.iter().enumerate().find(|(i, name)| {
                    names[i + 1..].iter().any(|other| name.begins(other) || other.begins(name))
                }) {
                    return Err(ComponentError::InvalidConfig(format!(
                        "command /{} of component '{}' has names a line cannot tell apart: '{name}' \
                         and another of its names begin one another",
                        command.name, provider.id
                    )));
                }
                if let Some(existing) = entries.iter().find(|e| e.command.collides_with(&command)) {
                    return Err(ComponentError::InvalidConfig(format!(
                        "command /{} of component '{}' and command /{} of component '{}' cannot \
                         be told apart: a name of one begins a name of the other",
                        command.name, provider.id, existing.command.name, existing.component
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

    /// The command `line` begins with - by its name or an alias - with what carries it out, and
    /// the rest of the line, its arguments (`Command::strip_from`); at most one, since none of
    /// the set collide.
    pub fn find<'a>(&self, line: &'a str) -> Option<(&CommandEntry<P>, &'a str)> {
        self.entries.iter().find_map(|e| e.command.strip_from(line).map(|args| (e, args)))
    }

    /// The command named `name` - by its name, not an alias - with what carries it out, if the set
    /// has it: for a command found before, by `find`, and named since by its name alone.
    pub fn entry(&self, name: &str) -> Option<&CommandEntry<P>> {
        self.entries.iter().find(|e| e.command.name.as_str() == name)
    }

    /// The command of the set that `command` collides with (`Command::collides_with`), if any.
    pub fn colliding(&self, command: &Command) -> Option<&Command> {
        self.entries.iter().map(|e| &e.command).find(|c| c.collides_with(command))
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

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::component::ComponentFailPolicy;

    #[derive(Debug)]
    struct Offering(Vec<Command>);

    impl ProvidesCommands for Offering {
        fn commands(&self) -> Vec<Command> {
            self.0.clone()
        }
    }

    fn command(name: &'static str, aliases: &[&'static str]) -> Command {
        Command {
            name: CommandName::fixed(name),
            aliases: aliases.iter().map(|alias| CommandName::fixed(alias)).collect(),
            description: String::new(),
            usage: None,
            owners_only: false,
        }
    }

    fn provider(id: &str, commands: Vec<Command>) -> ComponentImpl<dyn ProvidesCommands> {
        ComponentImpl {
            id: id.to_string(),
            fail_policy: ComponentFailPolicy::FailOpen,
            facet: Arc::new(Offering(commands)),
        }
    }

    fn set(
        providers: Vec<ComponentImpl<dyn ProvidesCommands>>,
    ) -> CommandSet<dyn ProvidesCommands> {
        CommandSet::new(providers).expect("no two collide")
    }

    #[test]
    fn a_line_finds_the_command_it_begins_with_by_its_name_or_an_alias() {
        let commands = set(vec![
            provider("participant", vec![command("participant add", &["add"])]),
            provider("help", vec![command("help", &[])]),
        ]);
        let found =
            |line| commands.find(line).map(|(e, args)| (e.command().name.to_string(), args));
        let add = Some(("participant add".to_string(), "Bob Smith"));
        assert_eq!(found("participant  add Bob Smith"), add);
        assert_eq!(found("add Bob Smith"), add);
        assert_eq!(found("help"), Some(("help".to_string(), "")));
        assert_eq!(found("participant"), None);
        assert_eq!(found("adder"), None);
        assert!(commands.entry("participant add").is_some(), "by its name");
        assert!(commands.entry("add").is_none(), "not by an alias");
    }

    #[test]
    fn commands_a_line_could_not_tell_apart_are_a_configuration_error() {
        for (left, right) in [
            (command("help", &[]), command("help", &[])),
            (command("channel", &[]), command("channel list", &[])),
            (command("participant add", &["add"]), command("add", &[])),
            (command("participant add", &["add"]), command("nudge", &["add more"])),
        ] {
            let (left_name, right_name) = (left.name.to_string(), right.name.to_string());
            let error =
                CommandSet::new(vec![provider("one", vec![left]), provider("two", vec![right])])
                    .expect_err("one would hide the other");
            let message = error.to_string();
            assert!(
                message.contains(&format!("/{left_name}"))
                    && message.contains(&format!("/{right_name}")),
                "{message}"
            );
            assert!(message.contains("'one'") && message.contains("'two'"), "{message}");
        }
        for alone in [
            command("participant add", &["participant"]),
            command("add more", &["add"]),
            command("add", &["add"]),
        ] {
            let error = CommandSet::new(vec![provider("one", vec![alone])])
                .expect_err("its own names cannot be told apart");
            assert!(error.to_string().contains("'one'"), "{error}");
        }
        let commands = set(vec![provider("help", vec![command("help", &[])])]);
        assert!(commands.colliding(&command("help me", &[])).is_some());
        assert!(commands.colliding(&command("helper", &[])).is_none());
    }

    #[test]
    fn a_name_taken_unchecked_is_checked_with_the_set() {
        for (name, aliases) in [("Help", &[][..]), ("channel  list", &[]), ("add", &["a b "][..])] {
            let error = CommandSet::new(vec![provider("odd", vec![command(name, aliases)])])
                .expect_err("no command name");
            assert!(error.to_string().contains("'odd'"), "{error}");
        }
    }
}
