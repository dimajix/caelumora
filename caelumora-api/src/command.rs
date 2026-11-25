//! What commands are made of, whoever runs them (issues 28, 269, 583): a command, which gives its
//! description (`Command`); the facet of a component that contributes commands (`CommandProvider`);
//! the set they are found in (`CommandSet`); which arguments one takes (`check_arguments`, issue
//! 574) and how one fails (`CommandError`). Each kind of command adds how it is carried out: a
//! session's the kernel's `ChannelCommand`, an agent's the agents' `AgentCommand`, the terminal's
//! its `TerminalCommand`.

use crate::component::{ComponentError, ComponentImpl, ComponentResult};
use caelumora_domain::command::CommandDescription;
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
    #[error("You are not permitted to run /{command}.")]
    Forbidden {
        command: String,
        #[source]
        refused: Refused,
    },
    #[error("/{command}: {reason}")]
    InvalidArguments { command: String, reason: String },
    /// What the command acts on was refused, as whoever refused it words it - not by which rule:
    /// the refusal of a command that holds no `Refused`, a plugin's (issue 596), as a tool answers
    /// it (`ToolError::PermissionDenied`).
    #[error("/{command} refused: {reason}")]
    PermissionDenied { command: String, reason: String },
    #[error("/{command} failed: {reason}")]
    Failed { command: String, reason: String },
}

pub type CommandResult<T> = Result<T, CommandError>;

/// Whether `args` may be given to `command` (issues 574, 583): a command that names no parameters
/// (`CommandDescription::parameters`) takes no arguments, and one with a required parameter is not
/// run without any. Whoever runs a command - the `CommandExecutor`, an agent's actor, the
/// terminal - checks it once the command may be run, so that no command checks it itself; what
/// the arguments given name is the command's to read.
///
/// # Errors
/// `CommandError::InvalidArguments` if `command` takes no arguments and `args` holds some, or it
/// requires one and `args` holds none.
pub fn check_arguments(command: &CommandDescription, args: &str) -> CommandResult<()> {
    let invalid = |reason: String| {
        Err(CommandError::InvalidArguments { command: command.name.to_string(), reason })
    };
    let given = !args.trim().is_empty();
    match command.usage() {
        None if given => invalid("it takes no arguments".to_string()),
        Some(usage) if !given && command.parameters.iter().any(|p| p.required) => {
            invalid(format!("it takes {usage}"))
        }
        _ => Ok(()),
    }
}

/// A command that can be carried out (issue 583), of whichever kind: what every command says of
/// itself, by which its set finds it. How it is carried out is its kind's - `ChannelCommand`,
/// `AgentCommand`, `TerminalCommand` -, since what it is given to run with differs.
pub trait Command: Send + Sync + Debug {
    /// The command as users see it: its names, what it does, what it takes - held by the command.
    fn description(&self) -> &CommandDescription;
}

/// The facet of a component that contributes commands of the kind `C` (issue 583): `dyn
/// ChannelCommand` for a session's, `dyn AgentCommand` for an agent's.
pub trait CommandProvider<C: ?Sized>: Send + Sync + Debug {
    /// The commands it contributes.
    fn commands(&self) -> Vec<Arc<C>>;
}

/// The commands a component made as it was built, which it contributes as they are.
impl<C: ?Sized + Command> CommandProvider<C> for Vec<Arc<C>> {
    fn commands(&self) -> Vec<Arc<C>> {
        self.clone()
    }
}

/// The commands of whoever runs them - a session's, from the global ones and the session's own; an
/// agent's (`caelumora_agents::command`, issue 370); the terminal's, of no component -, each found
/// by the line it begins (issue 560), no two that a line could not tell apart. Generic over the
/// kind of command, since what a command is given to run with is its kind's; finding one and
/// refusing two that collide are the same for every kind. Who may see and run one is asked by
/// whoever runs it: the `CommandExecutor` for the channel's, the channel's door for an agent's
/// (issue 516).
#[derive(Debug)]
pub struct CommandSet<C: ?Sized> {
    commands: Vec<Arc<C>>,
}

impl<C: ?Sized + Command> CommandSet<C> {
    /// The commands `providers` contribute.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if a name or alias of them is no command name - one taken
    /// unchecked (`CommandName::fixed`) -, if two names of one of them begin one another, or if
    /// two of them collide (`CommandDescription::collides_with`): that is a mistake in the
    /// configuration, and resolving it by order or by the longer name would silently hide one.
    pub fn new(providers: Vec<ComponentImpl<dyn CommandProvider<C>>>) -> ComponentResult<Self> {
        Self::checked(
            providers
                .into_iter()
                .map(|provider| (format!("component '{}'", provider.id), provider.commands()))
                .collect(),
        )
    }

    /// The commands `commands`, which no component contributes - the terminal's -, named
    /// `origin` should they not be told apart.
    ///
    /// # Errors
    /// As `new`.
    pub fn of(origin: &str, commands: Vec<Arc<C>>) -> ComponentResult<Self> {
        Self::checked(vec![(origin.to_string(), commands)])
    }

    /// The commands of `origins`, each with what it came from, to name it should one of them not
    /// be a command name or another collide with it (`new`).
    fn checked(origins: Vec<(String, Vec<Arc<C>>)>) -> ComponentResult<Self> {
        let mut found: Vec<(Arc<C>, String)> = Vec::new();
        for (origin, commands) in origins {
            for command in commands {
                let described = command.description();
                if let Some(name) =
                    described.names().find(|name| CommandName::new(name.as_str()).is_err())
                {
                    return Err(ComponentError::InvalidConfig(format!(
                        "{origin} provides a command named '{name}', which is no command name"
                    )));
                }
                let names: Vec<&CommandName> = described.names().collect();
                if let Some((_, name)) = names.iter().enumerate().find(|(i, name)| {
                    names[i + 1..].iter().any(|other| name.begins(other) || other.begins(name))
                }) {
                    return Err(ComponentError::InvalidConfig(format!(
                        "command /{} of {origin} has names a line cannot tell apart: '{name}' and \
                         another of its names begin one another",
                        described.name
                    )));
                }
                if let Some((existing, other)) =
                    found.iter().find(|(c, _)| c.description().collides_with(described))
                {
                    return Err(ComponentError::InvalidConfig(format!(
                        "command /{} of {origin} and command /{} of {other} cannot be told apart: \
                         a name of one begins a name of the other",
                        described.name,
                        existing.description().name
                    )));
                }
                found.push((command, origin.clone()));
            }
        }
        Ok(Self { commands: found.into_iter().map(|(command, _)| command).collect() })
    }

    /// A set without commands.
    pub fn empty() -> Self {
        Self { commands: Vec::new() }
    }

    /// The command `line` begins with - by its name or an alias - and the rest of the line, its
    /// arguments (`CommandDescription::strip_from`); at most one, since none of the set collide.
    pub fn find<'a>(&self, line: &'a str) -> Option<(&Arc<C>, &'a str)> {
        self.commands.iter().find_map(|c| c.description().strip_from(line).map(|args| (c, args)))
    }

    /// The command named `name` - by its name, not an alias -, if the set has it: for a command
    /// named by its name alone, as a request from another actor names it (`CommandRequested`).
    pub fn named(&self, name: &str) -> Option<&Arc<C>> {
        self.commands.iter().find(|c| c.description().name.as_str() == name)
    }

    /// The command of the set that `command` collides with (`CommandDescription::collides_with`),
    /// if any.
    pub fn colliding(&self, command: &CommandDescription) -> Option<&CommandDescription> {
        self.commands.iter().map(|c| c.description()).find(|c| c.collides_with(command))
    }

    /// The descriptions of the commands, sorted by name: the order components are built in follows
    /// their dependencies, not the configuration, so it is no order to show.
    pub fn descriptions(&self) -> Vec<CommandDescription> {
        let mut commands: Vec<CommandDescription> =
            self.commands.iter().map(|c| c.description().clone()).collect();
        commands.sort_by(|l, r| l.name.cmp(&r.name));
        commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::command::CommandParameter;
    use caelumora_domain::component::ComponentFailPolicy;

    /// A command of the tests, which does nothing but describe itself.
    #[derive(Debug)]
    struct Described(CommandDescription);

    impl Command for Described {
        fn description(&self) -> &CommandDescription {
            &self.0
        }
    }

    /// A provider of the tests, offering what it is given.
    #[derive(Debug)]
    struct Offering(Vec<Arc<dyn Command>>);

    impl CommandProvider<dyn Command> for Offering {
        fn commands(&self) -> Vec<Arc<dyn Command>> {
            self.0.clone()
        }
    }

    fn command(name: &'static str, aliases: &[&'static str]) -> CommandDescription {
        CommandDescription {
            name: CommandName::fixed(name),
            aliases: aliases.iter().map(|alias| CommandName::fixed(alias)).collect(),
            description: String::new(),
            parameters: Vec::new(),
            owners_only: false,
        }
    }

    fn provider(
        id: &str,
        commands: Vec<CommandDescription>,
    ) -> ComponentImpl<dyn CommandProvider<dyn Command>> {
        let commands =
            commands.into_iter().map(|c| Arc::new(Described(c)) as Arc<dyn Command>).collect();
        ComponentImpl {
            id: id.to_string(),
            fail_policy: ComponentFailPolicy::FailOpen,
            facet: Arc::new(Offering(commands)),
        }
    }

    fn set(
        providers: Vec<ComponentImpl<dyn CommandProvider<dyn Command>>>,
    ) -> CommandSet<dyn Command> {
        CommandSet::new(providers).expect("no two collide")
    }

    /// A command that names no parameters takes no arguments (issue 574), one with a required
    /// parameter is not run without any (issue 583); what it is given beside that it reads itself.
    #[test]
    fn a_command_takes_the_arguments_its_parameters_allow() {
        let list = command("participant list", &[]);
        assert!(check_arguments(&list, "").is_ok());
        assert!(check_arguments(&list, "  ").is_ok(), "whitespace is none");
        assert!(
            matches!(
                check_arguments(&list, "everyone"),
                Err(CommandError::InvalidArguments { ref command, ref reason })
                    if command == "participant list" && reason == "it takes no arguments"
            ),
            "refused"
        );
        let add = CommandDescription {
            parameters: vec![CommandParameter::required("name")],
            ..command("participant add", &[])
        };
        assert!(check_arguments(&add, "Bob Smith").is_ok(), "its own to check");
        assert!(
            matches!(
                check_arguments(&add, " "),
                Err(CommandError::InvalidArguments { ref reason, .. })
                    if reason == "it takes <name>"
            ),
            "required"
        );
        let wakeup = CommandDescription {
            parameters: vec![CommandParameter::optional("agent")],
            ..command("agent wakeup", &[])
        };
        assert!(check_arguments(&wakeup, "").is_ok(), "not required");
    }

    #[test]
    fn a_line_finds_the_command_it_begins_with_by_its_name_or_an_alias() {
        let commands = set(vec![
            provider("participant", vec![command("participant add", &["add"])]),
            provider("help", vec![command("help", &[])]),
        ]);
        let found =
            |line| commands.find(line).map(|(c, args)| (c.description().name.to_string(), args));
        let add = Some(("participant add".to_string(), "Bob Smith"));
        assert_eq!(found("participant  add Bob Smith"), add);
        assert_eq!(found("add Bob Smith"), add);
        assert_eq!(found("help"), Some(("help".to_string(), "")));
        assert_eq!(found("participant"), None);
        assert_eq!(found("adder"), None);
        assert!(commands.named("participant add").is_some(), "by its name");
        assert!(commands.named("add").is_none(), "not by an alias");
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

    /// Commands no component contributes are checked as those of components are, named by
    /// where they came from.
    #[test]
    fn commands_of_no_component_are_checked_alike() {
        let of = |commands: Vec<CommandDescription>| {
            let commands =
                commands.into_iter().map(|c| Arc::new(Described(c)) as Arc<dyn Command>).collect();
            CommandSet::of("the terminal", commands)
        };
        let commands = of(vec![command("exit", &["quit"]), command("help", &[])]).expect("apart");
        assert!(commands.find("quit").is_some(), "by an alias");
        let error = of(vec![command("exit", &["quit"]), command("quit now", &[])])
            .expect_err("one would hide the other");
        assert!(error.to_string().contains("of the terminal"), "{error}");
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
