//! User commands (issue 28): input that addresses the system rather than the
//! agents, such as `/help` or `/wakeup`.
//!
//! Commands are recognised by the frontends, which send them to the channel as commands instead of
//! as messages; [`parse_input`] is the rule they follow. Which command a line is, and where its
//! arguments begin, is decided by the commands it is run among (issue 560). What a command does is
//! up to the command itself (`caelumora_api::command::Command`, issue 583), which a component
//! contributes; this is what it says of itself (`CommandDescription`).

use crate::component::{ComponentConfig, ComponentDescriptor};
use crate::value::command_name::CommandName;
use crate::value::ValueError;
use serde::{Deserialize, Serialize};
use std::iter;

// Unlike every other kind constant, these live in the domain, not next to their component in the
// kernel: `default_commands` - the serde default of `AppConfig::commands` - names them, and the
// domain does not know the kernel (issue 101).

/// Component kind of the built-in `/help` command.
pub const HELP_KIND: &str = "help";
/// Component kind of the built-in command group `participant` (issue 560).
pub const PARTICIPANT_GROUP_KIND: &str = "participant";
/// Component kind of the built-in command group `agent` (issue 560).
pub const AGENT_GROUP_KIND: &str = "agent";
/// Component kind of the built-in command group `channel` (issue 560).
pub const CHANNEL_GROUP_KIND: &str = "channel";

/// A command as users see it: in `/help` and in the frontends' completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandDescription {
    /// The name, without the leading slash: its words, from its group to itself.
    pub name: CommandName,
    /// The other names it is found by, its short forms (issue 560); none unless named. What is
    /// recorded of it is its `name`, whichever it was issued by.
    #[serde(default)]
    pub aliases: Vec<CommandName>,
    /// What the command does, in one sentence.
    pub description: String,
    /// What it takes, in the order it takes them (issue 583); none unless named.
    #[serde(default)]
    pub parameters: Vec<CommandParameter>,
    /// Of a command an agent carries out itself (issue 269): whether it is
    /// for whoever answers for the agent alone - as approving its calls
    /// is - rather than for everyone who may write in the channel. `false`
    /// unless named; meaningless for a command the session runs.
    #[serde(default)]
    pub owners_only: bool,
}

impl CommandDescription {
    /// Its parameters as they are written in a list of commands - `<personality> [as <name>]`
    /// (`CommandParameter::written`) -, `None` if it takes none.
    pub fn usage(&self) -> Option<String> {
        let written: Vec<String> = self.parameters.iter().map(CommandParameter::written).collect();
        (!written.is_empty()).then(|| written.join(" "))
    }

    /// The rest of `line` - its arguments, trimmed - if `line` begins with its name or one of its
    /// aliases (`CommandName::strip_from`).
    pub fn strip_from<'a>(&self, line: &'a str) -> Option<&'a str> {
        self.names().find_map(|name| name.strip_from(line))
    }

    /// How the command is told in a list of commands: its signature, its short forms and what it
    /// does - "`/participant add <name>` (also `/add`) — Brings the user or agent named ...".
    pub fn help_line(&self) -> String {
        self.told(None)
    }

    /// How a command an agent carries out (issue 269) is told in a list of commands: as
    /// `help_line`, the agent named - "`/compact @agent` — Compacts the agent's context ...".
    pub fn agent_help_line(&self) -> String {
        self.told(Some("@agent"))
    }

    /// How the command is told, `addressee` named after its name where it has one: its signature,
    /// its short forms - " (also `/add`)" - and what it does.
    fn told(&self, addressee: Option<&str>) -> String {
        let usage = self.usage();
        let words: Vec<&str> = addressee.into_iter().chain(usage.as_deref()).collect();
        let signature = match words.is_empty() {
            true => format!("/{}", self.name),
            false => format!("/{} {}", self.name, words.join(" ")),
        };
        let aliases: Vec<String> = self.aliases.iter().map(|alias| format!("`/{alias}`")).collect();
        let also = match aliases.is_empty() {
            true => String::new(),
            false => format!(" (also {})", aliases.join(", ")),
        };
        format!("`{signature}`{also} — {}", self.description)
    }

    /// Whether a name of one - its name or an alias - begins a name of the other
    /// (`CommandName::begins`): a line beginning with the longer could not be told from one
    /// beginning with the shorter.
    pub fn collides_with(&self, other: &CommandDescription) -> bool {
        self.names().any(|mine| other.names().any(|its| mine.begins(its) || its.begins(mine)))
    }

    /// Its name and its aliases.
    pub fn names(&self) -> impl Iterator<Item = &CommandName> {
        iter::once(&self.name).chain(&self.aliases)
    }

    /// Whether `name` is its name or one of its aliases.
    pub fn is_named(&self, name: &CommandName) -> bool {
        self.names().any(|own| own == name)
    }
}

/// What `/help` is asked about by `args`, the words after it - a leading slash ignored -, wherever
/// it is answered (issue 560): `None` for every command; else the name its words make, of one
/// command - by its name or a short form (`CommandDescription::is_named`) - or of a group, the
/// commands whose names it begins (`CommandName::begins`).
///
/// # Errors
/// `ValueError` if its words make no command name: no command or group is named so.
pub fn help_topic(args: &str) -> Result<Option<CommandName>, ValueError> {
    let words: Vec<&str> = args.trim().trim_start_matches('/').split_whitespace().collect();
    match words.is_empty() {
        true => Ok(None),
        false => CommandName::new(words.join(" ")).map(Some),
    }
}

/// A parameter of a command (issue 583): what it is called, whether it must be given, and the word
/// that introduces it, if one does - `as` of `/agent create <personality> [as <name>]`. The last a
/// command takes is the rest of the line, since what it names may hold spaces. What value it takes
/// is the command's to read (issue 579 will describe it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandParameter {
    /// What it is called, in a list of commands and where it is missing.
    pub name: String,
    /// Whether the command cannot do without it.
    pub required: bool,
    /// The word it is introduced by, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
}

impl CommandParameter {
    /// The parameter `name` a command cannot do without.
    pub fn required(name: &str) -> Self {
        Self { name: name.to_string(), required: true, keyword: None }
    }

    /// The parameter `name` a command does without.
    pub fn optional(name: &str) -> Self {
        Self { name: name.to_string(), required: false, keyword: None }
    }

    /// The parameter, introduced by `keyword`.
    pub fn introduced_by(self, keyword: &str) -> Self {
        Self { keyword: Some(keyword.to_string()), ..self }
    }

    /// How it is written in a list of commands: `<name>` if it is required, `[name]` if not; with
    /// its keyword before it - `as <name>`, `[as <name>]`.
    fn written(&self) -> String {
        match (&self.keyword, self.required) {
            (None, true) => format!("<{}>", self.name),
            (None, false) => format!("[{}]", self.name),
            (Some(keyword), true) => format!("{keyword} <{}>", self.name),
            (Some(keyword), false) => format!("[{keyword} <{}>]", self.name),
        }
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
/// command groups `participant`, `agent` and `channel`.
pub fn default_commands() -> ComponentConfig {
    ComponentConfig {
        component_nodes: vec![
            ComponentDescriptor::new("help", HELP_KIND),
            ComponentDescriptor::new("participant", PARTICIPANT_GROUP_KIND),
            ComponentDescriptor::new("agent", AGENT_GROUP_KIND),
            ComponentDescriptor::new("channel", CHANNEL_GROUP_KIND),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(line: &str) -> UserInput {
        UserInput::Command(line.to_string())
    }

    /// A command is told in a list by its signature, its short forms and what it does; one an
    /// agent carries out with the agent named.
    #[test]
    fn a_command_is_told_by_its_signature_short_forms_and_description() {
        let described = |parameters: Vec<CommandParameter>, aliases: &[&str]| CommandDescription {
            description: "Brings someone in.".to_string(),
            parameters,
            ..named("participant add", aliases)
        };
        let plain = described(Vec::new(), &[]);
        assert_eq!(plain.help_line(), "`/participant add` — Brings someone in.");
        assert_eq!(plain.agent_help_line(), "`/participant add @agent` — Brings someone in.");
        let full = described(vec![CommandParameter::required("name")], &["add", "in"]);
        let also = "(also `/add`, `/in`) — Brings someone in.";
        assert_eq!(full.help_line(), format!("`/participant add <name>` {also}"));
        assert_eq!(full.agent_help_line(), format!("`/participant add @agent <name>` {also}"));
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
        let groups = [PARTICIPANT_GROUP_KIND, AGENT_GROUP_KIND, CHANNEL_GROUP_KIND];
        assert_eq!(kinds, [&[HELP_KIND][..], &groups].concat());
    }

    fn named(name: &str, aliases: &[&str]) -> CommandDescription {
        CommandDescription {
            name: CommandName::new(name).unwrap(),
            aliases: aliases.iter().map(|a| CommandName::new(a).unwrap()).collect(),
            description: String::new(),
            parameters: Vec::new(),
            owners_only: false,
        }
    }

    /// `/help` asks about every command without words, else about the name its words make,
    /// however they are separated; words no command name is made of name nothing.
    #[test]
    fn help_is_asked_about_the_name_its_words_make() {
        let topic = |args| help_topic(args).map(|t| t.map(|name| name.to_string()));
        assert_eq!(topic("  ").ok(), Some(None));
        assert_eq!(topic(" /participant   add ").ok(), Some(Some("participant add".to_string())));
        assert!(topic("Participant").is_err());
    }

    /// A parameter is written as it is required or not, its keyword before it.
    #[test]
    fn parameters_are_written_as_they_are_taken() {
        let create = CommandDescription {
            parameters: vec![
                CommandParameter::required("personality"),
                CommandParameter::optional("name").introduced_by("as"),
            ],
            ..named("agent create", &[])
        };
        assert_eq!(create.usage().as_deref(), Some("<personality> [as <name>]"));
        let written = |parameter: CommandParameter| parameter.written();
        assert_eq!(written(CommandParameter::optional("agent")), "[agent]");
        assert_eq!(written(CommandParameter::required("to").introduced_by("as")), "as <to>");
        assert_eq!(named("help", &[]).usage(), None, "none taken");
    }

    #[test]
    fn a_command_is_found_by_its_name_and_its_aliases() {
        let add = named("participant add", &["add"]);
        assert_eq!(add.strip_from("participant add Bob Smith"), Some("Bob Smith"));
        assert_eq!(add.strip_from("add  Bob"), Some("Bob"));
        assert_eq!(add.strip_from("participant"), None);
        assert_eq!(add.strip_from("adds Bob"), None);
        assert!(add.is_named(&CommandName::new("add").unwrap()), "by a short form");
        assert!(!add.is_named(&CommandName::new("participant").unwrap()), "not by its group");
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
