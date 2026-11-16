//! What the user can type, how a line typed is read, and what completes a line from it (issue 579):
//! a snapshot of the commands, handles, channels, personalities, sessions and templates the talk
//! knew when it was last read, which both ways of drawing complete what is typed from.
//!
//! After `/` a command is completed by its name or a short form, word by word as its group and it
//! are named (`CommandName::is_begun_by`); after the name of an agent's command, the handle of an
//! agent that carries it out; after a command's name - and an agent's handle -, its first parameter
//! by its kind (`ParameterKind`), the rest of the line being that argument, until the keyword of a
//! later parameter is typed. Anywhere else after `@`, the handle of a participant of the channel,
//! or a group handle (`mention::handle_typed_at`).
//!
//! A line is read as `parse_input` reads it, but for one beginning with `!`, the short form of
//! `/workspace run` in the terminal (issue 587, `read_input`): its shell command is completed by
//! nothing.

use caelumora_domain::command::{
    addressed, parse_input, CommandDescription, ParameterKind, UserInput,
};
use caelumora_domain::mention::{handle_of, handle_typed_at, GROUP_HANDLES};
use caelumora_kernel::command::RUN_COMMAND_NAME;
use std::collections::{BTreeSet, HashMap};

/// What the user can type now, as the talk last read it.
#[derive(Debug, Default)]
pub(crate) struct Typeable {
    /// The commands the terminal and the channel talked in carry out that the user may see.
    commands: Vec<CommandDescription>,
    /// The commands the agents of the channel talked in carry out that the user may see, each with
    /// the handles of the agents that do, as typed (`agents_commands`).
    agent_commands: Vec<(CommandDescription, Vec<String>)>,
    /// What a parameter of each kind may be given, as it is typed - a handle with its `@`.
    named: HashMap<ParameterKind, Vec<String>>,
}

/// What completes a line: where in it the text completed begins, and what may take its place up
/// to the cursor; no candidates where nothing completes it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Completion {
    /// The byte offset in the line where the text completed begins.
    pub(crate) start: usize,
    /// What may stand from `start` to the cursor, in order.
    pub(crate) candidates: Vec<String>,
}

impl Typeable {
    /// Adds `commands` of the terminal or the channel, which the user may see.
    pub(crate) fn add_commands(&mut self, commands: impl IntoIterator<Item = CommandDescription>) {
        self.commands.extend(commands);
    }

    /// Adds the commands the agents of the channel carry out, each with the handles of the agents
    /// that do, as typed (`command::agents_commands`).
    pub(crate) fn add_agents_commands(&mut self, commands: Vec<(CommandDescription, Vec<String>)>) {
        self.agent_commands.extend(commands);
    }

    /// Adds `names`, what a parameter of `kind` may be given, as it is typed - a handle with its
    /// `@`.
    pub(crate) fn add_named(
        &mut self,
        kind: ParameterKind,
        names: impl IntoIterator<Item = String>,
    ) {
        self.named.entry(kind).or_default().extend(names);
    }

    /// What completes `line` up to `cursor`, a byte offset: a command, an agent's handle after an
    /// agent's command, a command's first parameter, or a handle after `@` - as the module tells.
    /// A shell command after `!` completes nothing.
    pub(crate) fn complete(&self, line: &str, cursor: usize) -> Completion {
        let Some(before) = line.get(..cursor) else { return Completion::default() };
        if shell_command(before).is_some() {
            return Completion::default();
        }
        match parse_input(before) {
            UserInput::Command(_) => self.complete_command(&before[1..], 1),
            UserInput::Message(_) => self.complete_handle(before),
        }
    }

    /// What completes `typed` of a command, which begins at `offset` in the line.
    fn complete_command(&self, typed: &str, offset: usize) -> Completion {
        let end = offset + typed.len();
        for (command, handles) in self.with_their_agents() {
            let Some(rest) = command.strip_from(typed) else { continue };
            if rest.is_empty() && !typed.ends_with(char::is_whitespace) {
                // The name is typed whole, but not yet ended.
                continue;
            }
            let args_at =
                if rest.is_empty() { end } else { offset + typed.trim_end().len() - rest.len() };
            let args = &typed[args_at - offset..];
            return match handles {
                Some(handles) => self.complete_addressed(command, handles, args, args_at),
                None => self.complete_parameter(command, args, args_at),
            };
        }
        let leading = typed.len() - typed.trim_start().len();
        let names = self.with_their_agents().flat_map(|(command, _)| command.names());
        let begun: BTreeSet<&str> =
            names.filter(|name| name.is_begun_by(typed)).map(|name| name.as_str()).collect();
        Completion {
            start: offset + leading,
            candidates: begun.into_iter().map(String::from).collect(),
        }
    }

    /// The commands of the terminal and the channel without agents, then those of the agents, each
    /// with the handles of the agents that carry it out.
    fn with_their_agents(&self) -> impl Iterator<Item = (&CommandDescription, Option<&[String]>)> {
        let own = self.commands.iter().map(|command| (command, None));
        let agents = self
            .agent_commands
            .iter()
            .map(|(command, handles)| (command, Some(handles.as_slice())));
        own.chain(agents)
    }

    /// What completes `args`, at `args_at`, of the agent's command `command`, carried out by the
    /// agents of `handles`: while the first word is typed, the handle of one of them or a group
    /// handle; after it, the command's first parameter.
    fn complete_addressed(
        &self,
        command: &CommandDescription,
        handles: &[String],
        args: &str,
        args_at: usize,
    ) -> Completion {
        match addressed(args) {
            (whom, None) => {
                let offered: Vec<String> = handles.iter().cloned().chain(group_handles()).collect();
                Completion { start: args_at, candidates: beginning(&offered, whom) }
            }
            (_, Some(rest)) => {
                self.complete_parameter(command, rest, args_at + args.len() - rest.len())
            }
        }
    }

    /// What completes `args`, at `args_at`, as the first parameter of `command`: what its kind may
    /// be given; nothing once the keyword of a later parameter is typed, or for text.
    fn complete_parameter(
        &self,
        command: &CommandDescription,
        args: &str,
        args_at: usize,
    ) -> Completion {
        let Some(first) = command.parameters.first() else { return Completion::default() };
        let later = command.parameters.iter().skip(1).filter_map(|p| p.keyword.as_deref());
        if later.into_iter().any(|keyword| args.split_whitespace().any(|word| word == keyword)) {
            return Completion::default();
        }
        let offered = match first.kind {
            ParameterKind::Command => self.topics(),
            kind => self.named.get(&kind).cloned().unwrap_or_default(),
        };
        Completion { start: args_at, candidates: beginning(&offered, args) }
    }

    /// What `/help` may be asked about: every command's name and short forms, and the groups they
    /// stand in.
    fn topics(&self) -> Vec<String> {
        let mut topics = BTreeSet::new();
        for (command, _) in self.with_their_agents() {
            for name in command.names() {
                topics.insert(name.as_str().to_string());
                if let Some((group, _)) = name.as_str().split_once(' ') {
                    topics.insert(group.to_string());
                }
            }
        }
        topics.into_iter().collect()
    }

    /// What completes the handle typed in `before`, the line up to the cursor: a participant's, or
    /// a group handle.
    fn complete_handle(&self, before: &str) -> Completion {
        let Some(at) = handle_typed_at(before, before.len()) else { return Completion::default() };
        let participants =
            self.named.get(&ParameterKind::Participant).into_iter().flatten().cloned();
        let offered: Vec<String> = participants.chain(group_handles()).collect();
        Completion { start: at, candidates: beginning(&offered, &before[at..]) }
    }
}

/// What the user typed, `input`, as the terminal reads it (issue 587): a line beginning with `!`
/// the command `/workspace run` with its shell command (`shell_command`), one beginning with `!!` a
/// message beginning with `!`, any other as `parse_input` reads it.
pub(crate) fn read_input(input: &str) -> UserInput {
    if let Some(command) = shell_command(input) {
        return UserInput::Command(format!("{RUN_COMMAND_NAME} {command}"));
    }
    match input.strip_prefix("!!") {
        Some(text) => UserInput::Message(format!("!{text}")),
        None => parse_input(input),
    }
}

/// The shell command a line beginning with `!` runs: the rest of it; `None` for any other line,
/// also one beginning with `!!`, which writes a literal `!`.
fn shell_command(input: &str) -> Option<&str> {
    input.strip_prefix('!').filter(|command| !command.starts_with('!'))
}

/// The group handles, as typed (`@all`, `@channel`).
fn group_handles() -> impl Iterator<Item = String> {
    GROUP_HANDLES.iter().map(|group| format!("@{group}"))
}

/// Those of `offered` that `typed` begins, in any case - a handle with or without its `@`, as
/// handles are made of what is typed (`handle_of`), and none where whitespace was typed, which no
/// handle holds -, sorted, each once.
fn beginning(offered: &[String], typed: &str) -> Vec<String> {
    let typed_handle = handle_of(typed.strip_prefix('@').unwrap_or(typed));
    let in_words = typed.contains(char::is_whitespace);
    let lower = typed.to_lowercase();
    let begun = offered.iter().filter(|candidate| match candidate.strip_prefix('@') {
        Some(handle) => !in_words && handle.starts_with(&typed_handle),
        None => candidate.to_lowercase().starts_with(&lower),
    });
    begun.cloned().collect::<BTreeSet<_>>().into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::command::CommandParameter;
    use caelumora_domain::value::command_name::CommandName;

    /// The command `name`, also called `aliases`, taking `parameters`.
    fn command(
        name: &'static str,
        aliases: &[&'static str],
        parameters: Vec<CommandParameter>,
    ) -> CommandDescription {
        CommandDescription {
            name: CommandName::fixed(name),
            aliases: aliases.iter().map(|alias| CommandName::fixed(alias)).collect(),
            description: String::new(),
            parameters,
            owners_only: false,
        }
    }

    /// The parameter `name` of `kind`, required.
    fn taking(name: &str, kind: ParameterKind) -> Vec<CommandParameter> {
        vec![CommandParameter::required(name).of_kind(kind)]
    }

    /// Names, as given.
    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    /// What can be typed in a channel with Alice, Zoë and the agents Bob and Rex, of whom Bob
    /// compacts, in a session with an agent Old besides, taking part in the channels `main`,
    /// `Mailroom`, `Mädchen` and `dev` of those of the session, beside which lies `side`.
    fn typeable() -> Typeable {
        let personality =
            CommandParameter::required("personality").of_kind(ParameterKind::Personality);
        let mut typeable = Typeable::default();
        typeable.add_commands([
            command("channel switch", &["switch"], taking("channel", ParameterKind::JoinedChannel)),
            command("channel join", &[], taking("channel", ParameterKind::Channel)),
            command("channel list", &[], Vec::new()),
            command(
                "agent create",
                &[],
                vec![personality, CommandParameter::optional("name").introduced_by("as")],
            ),
            command("agent wakeup", &[], taking("agent", ParameterKind::Agent)),
            command("agent delete", &[], taking("agent", ParameterKind::SessionAgent)),
            command("participant remove", &[], taking("participant", ParameterKind::Participant)),
            command("session resume", &[], taking("id", ParameterKind::Session)),
            command("session new", &[], taking("template", ParameterKind::Template)),
            command("help", &[], taking("group or command", ParameterKind::Command)),
        ]);
        typeable.add_agents_commands(vec![(command("compact", &[], Vec::new()), names(&["@bob"]))]);
        typeable.add_named(ParameterKind::Participant, names(&["@alice", "@zoe", "@bob", "@rex"]));
        typeable.add_named(ParameterKind::Agent, names(&["@bob", "@rex"]));
        typeable.add_named(ParameterKind::SessionAgent, names(&["@bob", "@rex", "@old"]));
        typeable.add_named(
            ParameterKind::JoinedChannel,
            names(&["main", "Mailroom", "Mädchen", "dev"]),
        );
        typeable.add_named(
            ParameterKind::Channel,
            names(&["main", "Mailroom", "Mädchen", "dev", "side"]),
        );
        typeable.add_named(ParameterKind::Personality, names(&["Code Reviewer", "Coder"]));
        typeable.add_named(ParameterKind::Session, names(&["1a2b3c4d"]));
        typeable.add_named(ParameterKind::Template, names(&["default", "review"]));
        typeable
    }

    /// What completes `line`, its cursor at its end.
    fn completed(line: &str) -> Completion {
        typeable().complete(line, line.len())
    }

    fn candidates(line: &str) -> Vec<String> {
        completed(line).candidates
    }

    #[test]
    fn a_command_is_completed_by_its_name_and_its_short_forms_word_by_word() {
        assert_eq!(candidates("/chan"), ["channel join", "channel list", "channel switch"]);
        assert_eq!(candidates("/channel  sw"), ["channel switch"]);
        assert_eq!(candidates("/sw"), ["switch"]);
        assert_eq!(candidates("/co"), ["compact"], "an agent's too");
        assert_eq!(completed("/  chan").start, 3, "from the name typed on");
        assert!(candidates("/chan ").is_empty(), "a word ended is no word of the name");
        assert!(candidates("/x").is_empty());
        assert!(candidates("//chan").is_empty(), "a message that begins with a slash");
        let line = "/chan and more";
        assert_eq!(
            typeable().complete(line, 5).candidates,
            ["channel join", "channel list", "channel switch"],
            "up to the cursor"
        );
    }

    #[test]
    fn a_parameter_is_completed_by_its_kind_the_rest_of_the_line_its_argument() {
        assert_eq!(candidates("/channel switch ma"), ["Mailroom", "main"]);
        assert_eq!(completed("/switch ma").start, 8);
        assert_eq!(candidates("/switch "), ["Mailroom", "Mädchen", "dev", "main"]);
        assert_eq!(candidates("/switch Mä"), ["Mädchen"], "in letters beyond ASCII");
        assert_eq!(completed("/switch Mä").start, 8);
        assert_eq!(candidates("/agent create Code R"), ["Code Reviewer"], "a name of words");
        assert!(candidates("/agent create Coder as Bo").is_empty(), "after a later keyword");
        assert!(candidates("/channel list ").is_empty(), "it takes nothing");
        assert_eq!(
            candidates("/help chan"),
            ["channel", "channel join", "channel list", "channel switch"]
        );
        assert_eq!(candidates("/channel join s"), ["side"], "of the session");
        assert!(candidates("/switch s").is_empty(), "only those taken part in");
        assert_eq!(candidates("/session resume 1a"), ["1a2b3c4d"]);
        assert_eq!(candidates("/session new r"), ["review"]);
    }

    #[test]
    fn someone_is_completed_by_handle_among_whom_the_command_acts_on() {
        assert_eq!(candidates("/participant remove @"), ["@alice", "@bob", "@rex", "@zoe"]);
        assert_eq!(candidates("/agent wakeup "), ["@bob", "@rex"], "the agents taking part");
        assert_eq!(candidates("/agent delete @"), ["@bob", "@old", "@rex"], "those of the session");
        assert_eq!(candidates("/agent wakeup r"), ["@rex"], "a handle typed without its @");
        assert!(candidates("/agent wakeup @rex ").is_empty(), "no handle holds a space");
    }

    #[test]
    fn an_agent_s_command_is_completed_with_the_agents_that_carry_it_out() {
        assert_eq!(candidates("/compact "), ["@all", "@bob", "@channel"]);
        assert_eq!(candidates("/compact @b"), ["@bob"]);
        assert_eq!(candidates("/compact b"), ["@bob"], "a handle typed without its @");
        assert!(candidates("/compact @bob ").is_empty(), "it takes nothing after");
    }

    #[test]
    fn a_handle_is_completed_after_an_at_in_a_message() {
        assert_eq!(candidates("hi @"), ["@alice", "@all", "@bob", "@channel", "@rex", "@zoe"]);
        assert_eq!(candidates("hi @Al"), ["@alice", "@all"], "in any case");
        assert_eq!(candidates("hi @zoë"), ["@zoe"], "as its handle is made");
        assert_eq!(completed("hi @al").start, 3);
        assert!(candidates("mail bob@ex").is_empty(), "not in an address");
        assert_eq!(typeable().complete("@b and more", 2).candidates, ["@bob"], "up to the cursor");
        assert_eq!(typeable().complete("ä", 1), Completion::default(), "not within a character");
    }

    /// Issue 587: a line beginning with `!` runs the rest of it as `/workspace run`, completed by
    /// nothing; `!!` writes a literal `!`, completed as a message.
    #[test]
    fn a_bang_runs_a_shell_command_and_two_write_one() {
        let command = |line: &str| UserInput::Command(line.to_string());
        let message = |text: &str| UserInput::Message(text.to_string());
        assert_eq!(read_input("!ls -la"), command("workspace run ls -la"));
        assert_eq!(read_input("!echo \"a   b\""), command("workspace run echo \"a   b\""));
        assert_eq!(read_input("!!ls"), message("!ls"), "a literal bang");
        assert_eq!(read_input("!!/ls"), message("!/ls"), "and what follows it as written");
        assert_eq!(read_input("hi!"), message("hi!"), "a bang anywhere else is plain text");
        assert_eq!(read_input("/help"), command("help"), "else as `parse_input` reads it");
        assert!(candidates("!cat @").is_empty(), "a shell command");
        assert_eq!(candidates("!!hi @b"), ["@bob"], "a message");
    }
}
