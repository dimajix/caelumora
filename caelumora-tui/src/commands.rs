//! The commands the terminal carries out itself (issue 570): their names, by the rules of the
//! commands of a channel, and what `/help` tells of them.

use caelumora_domain::command::Command;
use caelumora_domain::value::command_name::CommandName;

/// A command the terminal carries out itself, named by the rules of the commands of a channel
/// (`docs/COMMANDS.md`, issue 570): grouped by what it acts on, with short forms for what is typed
/// often, no name or short form the beginning of another - nor of a command of the channel, which a
/// line goes to where it begins none of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalCommand {
    /// Ends the program.
    Exit,
    /// Talks in another channel the user takes part in.
    ChannelSwitch,
    /// Lists the sessions of the directory.
    SessionList,
    /// Starts a session and talks in it.
    SessionNew,
    /// Continues a session of the directory and talks in it.
    SessionResume,
    /// Deletes a session of the directory.
    SessionDelete,
    /// Asks an agent for its answer again.
    AgentRetry,
    /// Approves a call waiting for approval.
    ApprovalApprove,
    /// Denies a call waiting for approval.
    ApprovalDeny,
    /// Begins to answer questions put to the user.
    QuestionAnswer,
    /// Declines questions put to the user.
    QuestionDecline,
}

impl TerminalCommand {
    /// Every command of the terminal, in the order `/help` lists them.
    const ALL: [TerminalCommand; 11] = [
        TerminalCommand::Exit,
        TerminalCommand::ChannelSwitch,
        TerminalCommand::SessionList,
        TerminalCommand::SessionNew,
        TerminalCommand::SessionResume,
        TerminalCommand::SessionDelete,
        TerminalCommand::AgentRetry,
        TerminalCommand::ApprovalApprove,
        TerminalCommand::ApprovalDeny,
        TerminalCommand::QuestionAnswer,
        TerminalCommand::QuestionDecline,
    ];

    /// The command as the commands of a channel are told: its name, short forms, parameters and
    /// what it does.
    pub(crate) fn command(self) -> Command {
        match self {
            // Of no group, as `/help`: ending the program acts on no entity of a channel.
            TerminalCommand::Exit => Command {
                name: CommandName::fixed("exit"),
                aliases: vec![CommandName::fixed("quit")],
                description: "Ends the program, the session shut down in order.".to_string(),
                usage: None,
                owners_only: false,
            },
            TerminalCommand::ChannelSwitch => Command {
                name: CommandName::fixed("channel switch"),
                aliases: vec![CommandName::fixed("switch")],
                description: "Talks in another channel of the session you take part in.".to_string(),
                usage: Some("<channel>".to_string()),
                owners_only: false,
            },
            TerminalCommand::SessionList => Command {
                name: CommandName::fixed("session list"),
                aliases: Vec::new(),
                description: "Lists the sessions of this directory, the one spoken in last first.".to_string(),
                usage: None,
                owners_only: false,
            },
            TerminalCommand::SessionNew => Command {
                name: CommandName::fixed("session new"),
                aliases: vec![CommandName::fixed("new")],
                description: "Starts a session from the template named, or the program's, and talks in it.".to_string(),
                usage: Some("[template]".to_string()),
                owners_only: false,
            },
            TerminalCommand::SessionResume => Command {
                name: CommandName::fixed("session resume"),
                aliases: vec![CommandName::fixed("resume")],
                description: "Continues the session of this directory whose id begins so, and talks in it.".to_string(),
                usage: Some("<id>".to_string()),
                owners_only: false,
            },
            TerminalCommand::SessionDelete => Command {
                name: CommandName::fixed("session delete"),
                aliases: Vec::new(),
                description: "Deletes the session of this directory whose id begins so, not the one talked in; it can be restored.".to_string(),
                usage: Some("<id>".to_string()),
                owners_only: false,
            },
            TerminalCommand::AgentRetry => Command {
                name: CommandName::fixed("agent retry"),
                aliases: vec![CommandName::fixed("retry")],
                description: "Asks for the answer under way, the turn given up, or the last answer of an agent again.".to_string(),
                usage: None,
                owners_only: false,
            },
            TerminalCommand::ApprovalApprove => Command {
                name: CommandName::fixed("approval approve"),
                aliases: vec![CommandName::fixed("approve")],
                description: "Approves the call waiting for approval of that id.".to_string(),
                usage: Some("<id>".to_string()),
                owners_only: false,
            },
            TerminalCommand::ApprovalDeny => Command {
                name: CommandName::fixed("approval deny"),
                aliases: vec![CommandName::fixed("deny")],
                description: "Denies the call waiting for approval of that id.".to_string(),
                usage: Some("<id>".to_string()),
                owners_only: false,
            },
            TerminalCommand::QuestionAnswer => Command {
                name: CommandName::fixed("question answer"),
                aliases: vec![CommandName::fixed("answer")],
                description: "Begins to answer the questions put to you of that id, or the last, one by one.".to_string(),
                usage: Some("[id]".to_string()),
                owners_only: false,
            },
            TerminalCommand::QuestionDecline => Command {
                name: CommandName::fixed("question decline"),
                aliases: vec![CommandName::fixed("decline")],
                description: "Declines the questions put to you of that id, or the last.".to_string(),
                usage: Some("[id]".to_string()),
                owners_only: false,
            },
        }
    }

    /// The command of the terminal the command line `line` - without its slash (`parse_input`) -
    /// begins with, and its arguments; `None` for a command of the channel.
    pub(crate) fn find(line: &str) -> Option<(TerminalCommand, &str)> {
        TerminalCommand::ALL
            .into_iter()
            .find_map(|terminal| terminal.command().strip_from(line).map(|args| (terminal, args)))
    }
}

/// What `/help` with `args` tells of the commands of the terminal: all of them without one, the one
/// named by its name or a short form, or those of the group named; `None` if it names none of them.
pub(crate) fn terminal_help(args: &str) -> Option<String> {
    let wanted = args.trim().trim_start_matches('/');
    let commands = TerminalCommand::ALL.map(TerminalCommand::command);
    if let Some(command) = commands.iter().find(|command| command.strip_from(wanted) == Some("")) {
        return Some(command.help_line());
    }
    let group = match wanted {
        "" => None,
        _ => Some(CommandName::new(wanted.split_whitespace().collect::<Vec<_>>().join(" ")).ok()?),
    };
    let in_group = |command: &&Command| group.as_ref().is_none_or(|g| g.begins(&command.name));
    let listed: Vec<String> = commands
        .iter()
        .filter(in_group)
        .map(|command| format!("- {}", command.help_line()))
        .collect();
    (!listed.is_empty()).then(|| format!("Commands of this terminal:\n\n{}", listed.join("\n")))
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
        let switch = Some((TerminalCommand::ChannelSwitch, "side"));
        assert_eq!(TerminalCommand::find("channel   switch side"), switch);
        assert_eq!(TerminalCommand::find("switch side"), switch, "its short form");
        assert_eq!(TerminalCommand::find("quit"), Some((TerminalCommand::Exit, "")));
        assert_eq!(TerminalCommand::find("retry"), Some((TerminalCommand::AgentRetry, "")));
        assert_eq!(TerminalCommand::find("approvals"), None, "a word of its own");
        assert_eq!(TerminalCommand::find("channel list"), None, "the channel's");
    }

    /// No name or short form of the terminal's commands begins another, nor one of the commands
    /// of the channel the user may see.
    #[tokio::test]
    async fn the_terminal_s_commands_collide_with_none() {
        let commands = TerminalCommand::ALL.map(TerminalCommand::command);
        for (index, command) in commands.iter().enumerate() {
            let names: Vec<_> = command.names().collect();
            for name in &names {
                assert!(CommandName::new(name.as_str()).is_ok(), "/{name} is a command name");
            }
            for (at, name) in names.iter().enumerate() {
                assert!(
                    names[at + 1..].iter().all(|other| !name.begins(other) && !other.begins(name)),
                    "/{name} beside the other names of /{}",
                    command.name
                );
            }
            for other in &commands[index + 1..] {
                assert!(!command.collides_with(other), "/{} and /{}", command.name, other.name);
            }
        }
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
