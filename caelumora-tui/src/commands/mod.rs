//! The commands the terminal carries out itself (issues 570, 583): each a type of its own, named by
//! the rules of the commands of a channel, and what `/help` tells of them.
//!
//! One file per group, not per command as in the kernel: each command does no more than call the
//! method of `Talk` that does what it asks, a body of a line or a few.

mod agent;
mod approval;
mod channel;
mod exit;
mod question;
mod session;
mod thread;

use crate::talk::{Handled, Talk};
use async_trait::async_trait;
use caelumora_api::command::{Command, CommandSet};
use caelumora_api::component::ComponentResult;
use caelumora_domain::command::{help_topic, CommandDescription};
use caelumora_kernel::command::RUN_COMMAND_NAME;
use std::sync::Arc;

/// A command the terminal carries out itself (issues 570, 583), in the user's talk: named by the
/// rules of the commands of a channel (`docs/COMMANDS.md`) - grouped by what it acts on, with short
/// forms for what is typed often, no name or short form the beginning of another, nor of a command
/// of the channel, which a line goes to where it begins none of these.
#[async_trait]
pub(crate) trait TerminalCommand: Command {
    /// Carries out the command with `args`, the rest of the command line after its name, trimmed,
    /// in `talk`: what the line came to. A command that names no parameters is never given any,
    /// one with a required parameter never none (`check_arguments`). Unlike a command of the
    /// channel or of an agent, it answers no `CommandResult`: what failed is printed as what it
    /// did, by the method of `Talk` it calls, which tells it as the terminal tells every failure
    /// (`error_printed`).
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
        Arc::new(thread::ListCommand::new()),
        Arc::new(thread::FollowCommand::new()),
        Arc::new(thread::LineCommand::new()),
    ];
    CommandSet::of("the terminal", commands)
}

/// What `/help` with `args` tells of `commands`, the terminal's: all of them without one - and
/// `!`, its short form of `/workspace run` (issue 587) -, the one named by its name or a short
/// form, or those of the group named; `None` if it names none of them.
pub(crate) fn terminal_help(
    commands: &CommandSet<dyn TerminalCommand>,
    args: &str,
) -> Option<String> {
    let topic = help_topic(args).ok()?;
    let commands = commands.descriptions();
    if let Some(name) = &topic {
        if let Some(command) = commands.iter().find(|command| command.is_named(name)) {
            return Some(command.help_line());
        }
    }
    let in_group =
        |command: &&CommandDescription| topic.as_ref().is_none_or(|g| g.begins(&command.name));
    let mut listed: Vec<String> = commands
        .iter()
        .filter(in_group)
        .map(|command| format!("- {}", command.help_line()))
        .collect();
    if topic.is_none() {
        let bang = "`!!` begins a message with `!`";
        listed.push(format!("- `!<command>` — Short for `/{RUN_COMMAND_NAME} <command>`; {bang}."));
    }
    (!listed.is_empty()).then(|| format!("Commands of this terminal:\n\n{}", listed.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::testing::in_directory_running_commands;
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

    /// No name or short form of the terminal's commands begins another (`commands`), nor one of
    /// the commands of the channel the user may see.
    #[tokio::test]
    async fn the_terminal_s_commands_collide_with_none() {
        let commands = commands().expect("none of them collide").descriptions();
        let directory = in_directory_running_commands().await;
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
