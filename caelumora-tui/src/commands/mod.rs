//! The commands the terminal carries out itself (issues 570, 583): each a type of its own, named by
//! the rules of the commands of a channel; what `/help` tells of them, and how they name what they
//! act on by the beginning of its id (issue 571).
//!
//! One file per group, not per command as in the kernel: each command does no more than call the
//! method of `Talk` that does what it asks, a body of a line or a few.

mod agent;
mod approval;
mod channel;
mod exit;
mod question;
mod session;

use crate::talk::{Handled, Talk};
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandSet};
use caelumora_api::component::ComponentResult;
use caelumora_domain::command::CommandDescription;
use caelumora_domain::value::command_name::CommandName;
use std::sync::Arc;

/// A command the terminal carries out itself (issues 570, 583), in the user's talk: named by the
/// rules of the commands of a channel (`docs/COMMANDS.md`) - grouped by what it acts on, with short
/// forms for what is typed often, no name or short form the beginning of another, nor of a command
/// of the channel, which a line goes to where it begins none of these.
#[async_trait]
pub(crate) trait TerminalCommand: Command {
    /// Carries out the command with `args`, the rest of the command line after its name, in
    /// `talk`: what the line came to. A command that names no parameters is never given any
    /// (`check_arguments`). Unlike a command of the channel or of an agent, it answers no
    /// `CommandResult`: what failed is printed as what it did, by the method of `Talk` it calls,
    /// which tells it as the terminal tells every failure (`error_printed`).
    async fn execute(&self, args: &str, talk: &mut Talk) -> Handled;
}

/// The commands of the terminal, as a set (`CommandSet::of`).
///
/// # Errors
/// As `CommandSet::of`, which a test keeps from happening.
pub(crate) fn commands() -> ComponentResult<CommandSet<dyn TerminalCommand>> {
    let commands: Vec<Arc<dyn TerminalCommand>> = vec![
        Arc::new(exit::ExitCommand::new()),
        Arc::new(channel::SwitchCommand::new()),
        Arc::new(session::ListCommand::new()),
        Arc::new(session::NewCommand::new()),
        Arc::new(session::ResumeCommand::new()),
        Arc::new(session::DeleteCommand::new()),
        Arc::new(agent::RetryCommand::new()),
        Arc::new(approval::ApprovalCommand::approve()),
        Arc::new(approval::ApprovalCommand::deny()),
        Arc::new(question::AnswerCommand::new()),
        Arc::new(question::DeclineCommand::new()),
    ];
    CommandSet::of("the terminal", commands)
}

/// What `/help` with `args` tells of `commands`, the terminal's: all of them without one, the one
/// named by its name or a short form, or those of the group named; `None` if it names none of them.
pub(crate) fn terminal_help(
    commands: &CommandSet<dyn TerminalCommand>,
    args: &str,
) -> Option<String> {
    let wanted = args.trim().trim_start_matches('/');
    let commands = commands.descriptions();
    if let Some(command) = commands.iter().find(|command| command.strip_from(wanted) == Some("")) {
        return Some(command.help_line());
    }
    let group = match wanted {
        "" => None,
        _ => Some(CommandName::new(wanted.split_whitespace().collect::<Vec<_>>().join(" ")).ok()?),
    };
    let in_group =
        |command: &&CommandDescription| group.as_ref().is_none_or(|g| g.begins(&command.name));
    let listed: Vec<String> = commands
        .iter()
        .filter(in_group)
        .map(|command| format!("- {}", command.help_line()))
        .collect();
    (!listed.is_empty()).then(|| format!("Commands of this terminal:\n\n{}", listed.join("\n")))
}

/// Why the beginning of an id given to a command names none of what it looks among
/// (`beginning_with`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotNamed {
    /// No beginning was given.
    Blank,
    /// None begins with it, the beginning given.
    None(String),
    /// Several begin with it, the beginning given.
    Several(String),
}

impl NotNamed {
    /// What tells why, of what the command looks among, `among` - in the plural.
    pub(crate) fn told(&self, among: &str) -> String {
        match self {
            NotNamed::Blank => format!("None of the {among} named: give the beginning of its id"),
            NotNamed::None(prefix) => format!("None of the {among} begins with '{prefix}'"),
            NotNamed::Several(prefix) => format!("Several of the {among} begin with '{prefix}'"),
        }
    }
}

/// The one of `items` whose id - as `id_of` writes it - begins with `prefix`, as the commands of
/// the terminal name a session, an approval or questions: no id typed in full (issue 571).
///
/// # Errors
/// If `prefix` is blank, none of them begins so, or several do.
pub(crate) fn beginning_with<T>(
    items: impl IntoIterator<Item = T>,
    id_of: impl Fn(&T) -> String,
    prefix: &str,
) -> Result<T, NotNamed> {
    let prefix = prefix.trim().to_lowercase();
    if prefix.is_empty() {
        return Err(NotNamed::Blank);
    }
    let mut found = items.into_iter().filter(|item| id_of(item).starts_with(&prefix));
    match (found.next(), found.next()) {
        (Some(item), None) => Ok(item),
        (Some(_), Some(_)) => Err(NotNamed::Several(prefix)),
        (None, _) => Err(NotNamed::None(prefix)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::testing::in_directory;
    use crate::sessions::Opening;
    use caelumora_domain::actor::ActorId;

    /// The commands of the terminal are found in a command line by their names and short forms,
    /// as a command of the channel is; a command of the channel begins none of them.
    #[test]
    fn a_command_of_the_terminal_is_found_by_its_names() {
        let commands = commands().expect("none collide");
        let found =
            |line| commands.find(line).map(|(c, args)| (c.description().name.to_string(), args));
        let switch = Some(("channel switch".to_string(), "side"));
        assert_eq!(found("channel   switch side"), switch);
        assert_eq!(found("switch side"), switch, "its short form");
        assert_eq!(found("quit"), Some(("exit".to_string(), "")));
        assert_eq!(found("retry"), Some(("agent retry".to_string(), "")));
        assert_eq!(found("approvals"), None, "a word of its own");
        assert_eq!(found("channel list"), None, "the channel's");
    }

    /// What a command names by the beginning of an id is the one alone that begins so, whatever
    /// the case of what is typed.
    #[test]
    fn the_one_beginning_so_is_named() {
        let ids = ["ab12", "ab34", "cd56"];
        let named = |prefix| beginning_with(ids, |id| id.to_string(), prefix);
        assert_eq!(named(" AB1 "), Ok("ab12"));
        assert_eq!(named("ab"), Err(NotNamed::Several("ab".to_string())));
        assert_eq!(named("ef"), Err(NotNamed::None("ef".to_string())));
        assert_eq!(named(" "), Err(NotNamed::Blank));
    }

    /// No name or short form of the terminal's commands begins another (`commands`), nor one of
    /// the commands of the channel the user may see.
    #[tokio::test]
    async fn the_terminal_s_commands_collide_with_none() {
        let commands = commands().expect("none of them collide").descriptions();
        let directory = in_directory().await;
        let start = Opening::Start { template: None };
        let (session_loop, _) = directory.sessions.open(&start).await.expect("started");
        let channel = session_loop.channels().into_iter().next().expect("its channel");
        let ActorId::User(user) = directory.owner.caller_id else { unreachable!("a user") };
        let actor = channel.user_actor(user).expect("the owner's actor runs");
        let of_channel = actor.visible_commands().await;
        assert!(!of_channel.is_empty(), "the channel's commands");
        for command in &commands {
            let colliding = of_channel.iter().find(|theirs| theirs.collides_with(command));
            assert!(colliding.is_none(), "/{} and the channel's {colliding:?}", command.name);
        }
    }
}
