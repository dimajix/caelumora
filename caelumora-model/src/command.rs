//! User commands (issue 28): input that addresses the system rather than the
//! agents, such as `/help` or `/wakeup`.
//!
//! Commands are recognised by the frontends, which send them to the channel
//! as commands instead of as messages; [`parse_input`] is the rule they
//! follow. What a command does is up to the `CommandProvider` component that
//! contributes it (see `caelumora-runtime`).

use crate::component::{ComponentConfig, ComponentDescriptor};
use serde::{Deserialize, Serialize};

// Unlike every other kind constant, these two live in the model, not next to
// their component in the runtime: `default_commands` - the serde default of
// `AppConfig::commands` - names them, and the model does not know the
// runtime (issue 101).

/// Component kind of the built-in `/help` command.
pub const HELP_COMMAND_KIND: &str = "help_command";
/// Component kind of the built-in `/wakeup` command.
pub const WAKEUP_COMMAND_KIND: &str = "wakeup_command";

/// A command as users see it: in `/help` and in the frontends' completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    /// The name, without the leading slash.
    pub name: String,
    /// What the command does, in one sentence.
    pub description: String,
    /// The parameters, e.g. `[agent]`, or `None` if it takes none.
    pub usage: Option<String>,
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
    /// A command: `name` without the slash (empty for a lone `/`), `args` the
    /// trimmed rest of the line, left to the command to parse.
    Command { name: String, args: String },
}

/// Classifies a line of user input.
///
/// Input starting with `/` is a command; its name runs up to the first
/// whitespace. `//` escapes a literal slash, so `//etc/hosts` is the message
/// `/etc/hosts`. A slash anywhere else is plain text.
pub fn parse_input(input: &str) -> UserInput {
    let Some(rest) = input.strip_prefix('/') else {
        return UserInput::Message(input.to_string());
    };
    if rest.starts_with('/') {
        return UserInput::Message(rest.to_string());
    }
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    UserInput::Command { name: name.to_string(), args: args.trim().to_string() }
}

/// The commands a deployment has unless its configuration says otherwise:
/// `/help` and `/wakeup`.
pub fn default_commands() -> ComponentConfig {
    ComponentConfig {
        component_nodes: vec![ComponentDescriptor::new("help", HELP_COMMAND_KIND), ComponentDescriptor::new("wakeup", WAKEUP_COMMAND_KIND)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str, args: &str) -> UserInput {
        UserInput::Command { name: name.to_string(), args: args.to_string() }
    }

    #[test]
    fn a_leading_slash_makes_a_command_with_the_rest_as_arguments() {
        assert_eq!(parse_input("/help"), command("help", ""));
        assert_eq!(parse_input("/wakeup sassy"), command("wakeup", "sassy"));
        assert_eq!(parse_input("/wakeup   sassy  "), command("wakeup", "sassy"));
        assert_eq!(parse_input("/help\twakeup"), command("help", "wakeup"));
    }

    #[test]
    fn a_double_slash_escapes_a_literal_slash() {
        assert_eq!(parse_input("//etc/hosts is missing"), UserInput::Message("/etc/hosts is missing".to_string()));
        assert_eq!(parse_input("//"), UserInput::Message("/".to_string()));
    }

    #[test]
    fn a_slash_elsewhere_is_text() {
        assert_eq!(parse_input("see /etc/hosts"), UserInput::Message("see /etc/hosts".to_string()));
        assert_eq!(parse_input(" /help"), UserInput::Message(" /help".to_string()));
    }

    #[test]
    fn a_lone_slash_is_an_empty_command() {
        assert_eq!(parse_input("/"), command("", ""));
        assert_eq!(parse_input("/ help"), command("", "help"));
    }

    #[test]
    fn the_default_commands_are_help_and_wakeup() {
        let kinds: Vec<String> = default_commands().component_nodes.into_iter().map(|n| n.kind).collect();
        assert_eq!(kinds, vec![HELP_COMMAND_KIND, WAKEUP_COMMAND_KIND]);
    }
}
