//! User commands (issue 28): input that addresses the system rather than the
//! agents, such as `/help` or `/wakeup`.
//!
//! Commands are recognised by the frontends, which send them to the channel as commands instead of
//! as messages; [`parse_input`] is the rule they follow. Which command a line is, and where its
//! arguments begin, is decided by the commands it is run among (issue 560). What a command does is
//! up to the `ChannelCommandProvider` component that contributes it (see `caelumora-kernel`).

use crate::component::{ComponentConfig, ComponentDescriptor};
use crate::value::command_name::CommandName;
use serde::{Deserialize, Serialize};
use std::iter;

// Unlike every other kind constant, these live in the domain, not next to their component in the
// kernel: `default_commands` - the serde default of `AppConfig::commands` - names them, and the
// domain does not know the kernel (issue 101).

/// Component kind of the built-in `/help` command.
pub const HELP_KIND: &str = "help";
/// Component kind of the built-in command group `participant` (issue 560): `/participant add`
/// and `/participant nudge`.
pub const PARTICIPANT_GROUP_KIND: &str = "participant";
/// Component kind of the built-in command group `agent` (issue 560): `/agent create` and
/// `/agent wakeup`.
pub const AGENT_GROUP_KIND: &str = "agent";

/// A command as users see it: in `/help` and in the frontends' completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    /// The name, without the leading slash: its words, from its group to itself.
    pub name: CommandName,
    /// The other names it is found by, its short forms (issue 560); none unless named. What is
    /// recorded of it is its `name`, whichever it was issued by.
    #[serde(default)]
    pub aliases: Vec<CommandName>,
    /// What the command does, in one sentence.
    pub description: String,
    /// The parameters, e.g. `[agent]`, or `None` if it takes none.
    pub usage: Option<String>,
    /// Of a command an agent carries out itself (issue 269): whether it is
    /// for whoever answers for the agent alone - as approving its calls
    /// is - rather than for everyone who may write in the channel. `false`
    /// unless named; meaningless for a command the session runs.
    #[serde(default)]
    pub owners_only: bool,
}

impl Command {
    /// The rest of `line` - its arguments, trimmed - if `line` begins with its name or one of its
    /// aliases (`CommandName::strip_from`).
    pub fn strip_from<'a>(&self, line: &'a str) -> Option<&'a str> {
        self.names().find_map(|name| name.strip_from(line))
    }

    /// Whether a name of one - its name or an alias - begins a name of the other
    /// (`CommandName::begins`): a line beginning with the longer could not be told from one
    /// beginning with the shorter.
    pub fn collides_with(&self, other: &Command) -> bool {
        self.names().any(|mine| other.names().any(|its| mine.begins(its) || its.begins(mine)))
    }

    /// Its name and its aliases.
    pub fn names(&self) -> impl Iterator<Item = &CommandName> {
        iter::once(&self.name).chain(&self.aliases)
    }
}

/// Who sees what a command answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandVisibility {
    /// Only the user who issued the command, e.g. `/help`.
    Private,
    /// Everybody in the channel, e.g. `/wakeup`, whose effect concerns all.
    Channel,
}

/// What a command answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub text: String,
    pub visibility: CommandVisibility,
}

impl CommandOutput {
    pub fn private(text: impl Into<String>) -> Self {
        Self { text: text.into(), visibility: CommandVisibility::Private }
    }

    pub fn channel(text: impl Into<String>) -> Self {
        Self { text: text.into(), visibility: CommandVisibility::Channel }
    }
}

/// A line of user input, classified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserInput {
    /// Text for the conversation.
    Message(String),
    /// A command: the line after the slash, trimmed - empty for a lone `/`. Which command it is,
    /// and where its arguments begin, is decided by the commands it is run among (issue 560).
    Command(String),
}

/// Classifies a line of user input.
///
/// Input starting with `/` is a command, the rest of the line after the slash. `//` escapes a
/// literal slash, so `//etc/hosts` is the message `/etc/hosts`. A slash anywhere else is plain
/// text.
pub fn parse_input(input: &str) -> UserInput {
    let Some(rest) = input.strip_prefix('/') else {
        return UserInput::Message(input.to_string());
    };
    if rest.starts_with('/') {
        return UserInput::Message(rest.to_string());
    }
    UserInput::Command(rest.trim().to_string())
}

/// The commands a deployment has unless its configuration says otherwise: `/help` and the
/// command groups `participant` and `agent`.
pub fn default_commands() -> ComponentConfig {
    ComponentConfig {
        component_nodes: vec![
            ComponentDescriptor::new("help", HELP_KIND),
            ComponentDescriptor::new("participant", PARTICIPANT_GROUP_KIND),
            ComponentDescriptor::new("agent", AGENT_GROUP_KIND),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(line: &str) -> UserInput {
        UserInput::Command(line.to_string())
    }

    #[test]
    fn a_leading_slash_makes_a_command_of_the_rest_of_the_line() {
        assert_eq!(parse_input("/help"), command("help"));
        assert_eq!(parse_input("/wakeup sassy"), command("wakeup sassy"));
        assert_eq!(parse_input("/channel   list  "), command("channel   list"));
        assert_eq!(parse_input("/help\twakeup"), command("help\twakeup"));
    }

    #[test]
    fn a_double_slash_escapes_a_literal_slash() {
        assert_eq!(
            parse_input("//etc/hosts is missing"),
            UserInput::Message("/etc/hosts is missing".to_string())
        );
        assert_eq!(parse_input("//"), UserInput::Message("/".to_string()));
    }

    #[test]
    fn a_slash_elsewhere_is_text() {
        assert_eq!(parse_input("see /etc/hosts"), UserInput::Message("see /etc/hosts".to_string()));
        assert_eq!(parse_input(" /help"), UserInput::Message(" /help".to_string()));
    }

    #[test]
    fn a_lone_slash_is_an_empty_command() {
        assert_eq!(parse_input("/"), command(""));
        assert_eq!(parse_input("/  "), command(""));
    }

    #[test]
    fn the_default_commands_are_the_built_in_ones() {
        let kinds: Vec<String> =
            default_commands().component_nodes.into_iter().map(|n| n.kind).collect();
        assert_eq!(kinds, vec![HELP_KIND, PARTICIPANT_GROUP_KIND, AGENT_GROUP_KIND]);
    }

    fn named(name: &str, aliases: &[&str]) -> Command {
        Command {
            name: CommandName::new(name).unwrap(),
            aliases: aliases.iter().map(|a| CommandName::new(a).unwrap()).collect(),
            description: String::new(),
            usage: None,
            owners_only: false,
        }
    }

    #[test]
    fn a_command_is_found_by_its_name_and_its_aliases() {
        let add = named("participant add", &["add"]);
        assert_eq!(add.strip_from("participant add Bob Smith"), Some("Bob Smith"));
        assert_eq!(add.strip_from("add  Bob"), Some("Bob"));
        assert_eq!(add.strip_from("participant"), None);
        assert_eq!(add.strip_from("adds Bob"), None);
    }

    #[test]
    fn commands_collide_where_a_name_of_one_begins_a_name_of_the_other() {
        let add = named("participant add", &["add"]);
        assert!(add.collides_with(&named("participant", &[])));
        assert!(add.collides_with(&named("participant add", &[])));
        assert!(add.collides_with(&named("nudge", &["add"])));
        assert!(add.collides_with(&named("add more", &[])));
        assert!(!add.collides_with(&named("participant nudge", &["nudge"])));
        assert!(!add.collides_with(&named("adder", &[])));
    }
}
