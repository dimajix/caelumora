# 587 — A person cannot run a command in the workspace

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** Only agents start commands in a session's workspace, so whoever wants the agents of a channel to see what `cargo test` or `git status` says has to run it outside `caelumora-tui` and paste its output, or ask an agent to run it - a turn and an approval for what the person could have typed.

## The problem

A command runs in a workspace through the shell of a channel (`WorkspaceController::shell_in`,
[WORKSPACE.md](../docs/WORKSPACE.md#commands)), and the only one who starts one there is an
agent, through the shell tool of `workspace`
([TOOLS.md](../docs/TOOLS.md#the-workspace)). A person can end a command of a channel (the REST
route `kill_command`) but start none - not in the web UI, not in `caelumora-tui`.

In the terminal program this is felt most: its workspace is the directory it was started in
(storage `directory`, with a shell by default, issue
[554](554-the-terminal-program-keeps-its-data-like-a-server.md)), so a command there is a
command in the project the person works on. To show an agent the output of a build, a test or
`git diff`, they run it in another terminal and paste what it printed, which the agents then
hear as the person's words, or they ask an agent to run it, which costs a turn and an approval.

Claude Code offers a line beginning with `!`: the rest of it runs as a shell command, and the
command with its output goes into the conversation, so that the model sees both when it is next
asked. The maintainer wants the same for `caelumora-tui`.

## Why it matters

What a command printed is often the very thing a person wants to talk about with an agent. A
way around it - pasting, asking the agent - either loses that it was a command and what it
ended with, or spends an agent's turn on what the person could do in a second.

## Considerations

### "For the terminal" does not stay in the terminal

What the agents are to see has to enter the channel's history: through the user's actor, admitted
by the channel, recorded, and perceived by the agents (see
[CHANNELS.md](../docs/CHANNELS.md#acting-in-a-channel)). The terminal cannot do any of that
itself, so the kernel needs a kind of its own for a person's command and its record, whatever
offers it at first. The web UI shows the history too, so it has to show such a record, even if
`!` is typed only in the terminal.

### Where the command runs

- **A. In the session's workspace, through the channel's shell**, as an agent's command runs:
  `WorkspaceController::shell_in` asks what it asks of an agent - reading the session and the
  channel, the workspace usable in the channel, `WriteWorkspace` - in the person's name. The
  command is then the channel's ([WORKSPACE.md](../docs/WORKSPACE.md#commands)): bounded by
  the storage's `max_run_secs`, ended by the `CommandReaper` when nobody may run it any longer,
  ended when the session stops. Its output is classified as the workspace, which flows into
  every channel that uses it. In the terminal program this is the directory it was started in,
  which is what the person expects.
- **B. On the terminal's host, by the terminal itself.** Passes by every permission, by the
  classification and by the bounds of the commands, and could not be offered by any other
  frontend. Rejected.

Recommended: A. Where the channel uses no workspace, or its storage has no shell, the command is
refused, saying why.

### How it is issued

- **A. A command of the channel**, a `ChannelCommand` of a component, as the groups
  `participant`, `agent` and `channel` are (`default_commands`,
  [COMMANDS.md](../docs/COMMANDS.md#commands-are-components)) - for instance a group `workspace`
  with `/workspace run <command>` - and `!` a lexical short form of it in
  `caelumora_domain::command::parse_input`, beside `/`. That follows the pattern of commands: a
  `CommandProvider` facet, run by the `CommandExecutor`, which asks `ExecuteCommand`; the command
  asks for the rest what `shell_in` asks. A deployment that does not want people to run commands
  leaves the component out of `AppConfig::commands`.
- **B. A command of the terminal** (`TerminalCommand`). It would still have to send the record
  through the user's actor, and no other frontend could offer it. Rejected unless A does not fit.

Recommended: A. To be decided with it:

- whether the web UI accepts `!` as well - `parse_input` is mirrored there, and a lexical rule
  of one frontend alone would be a second rule;
- how a message beginning with a literal `!` is written - `!!`, as `//` writes a literal `/`,
  or no escape at all;
- the name of the command and of its group, and whether it has a short form - no name of the
  channel's may begin, or be the beginning of, one of the terminal's.

### What enters the history, and what the agents see

What an agent takes in of others is decided in one place, and both of today's ways leave a
person's command out:

- A message of another is heard as its text and attachments alone, without its tool calls and
  results, which were the sender's (`Message::as_heard`, `caelumora-domain/src/message.rs:317`).
- A command carried out is recorded (`HistoryEntry::CommandExecuted`, issue 417), but never
  reaches an agent's context: COMMANDS.md says "A command is not a message: it never reaches an
  agent's context as something said." This issue asks for exactly the opposite for one command,
  and so reopens that rule for it; whatever is chosen, the rule is to be restated with its
  exception.
- What an agent takes note of without hearing it as words is today a join or a leave
  (`AgentChannelState::note_announced`, `caelumora-agents/src/actor/intake.rs:260`, issue 408):
  recorded into the transcript where it happened, beginning no turn.

The options:

- **A. A message of the person carrying the command and its output as text**, as Claude Code
  puts them into a user message. The agents hear it by the existing rule. But it is a message:
  it asks the moderator who is to respond, it can be modified and taken back, its text would be
  composed by the frontend - in each frontend alike -, and the marker `@alice:` would say that
  the person said what the command printed.
- **B. The record of the command, taken note of by the agents** as a join is: the run is
  recorded channel-wide - command, output, exit code, whether output was dropped -, and every
  agent of the channel writes it into its transcript where it happened, worded for the model in
  one place, as the notes of joins are. Which entry carries it is to be decided:
  `CommandExecuted`, whose `output` is a text and which agents today do not take in - then with
  a rule which of its commands they do -, or an entry of its own carrying the command's
  `CommandState`, as the shell tool's result does.

Recommended: B. Its structured form lets both frontends show a person's command as they show an
agent's, by the `shell` presentation (issue
[586](586-the-terminal-shows-a-tool-call-by-its-raw-arguments.md) for the terminal); an entry of
its own beside `CommandExecuted` is to be weighed against a second way of recording a command.

To be decided as well: whether a run asks anyone to respond. Claude Code's does not - the model
sees it with the next message -, and recommended is the same: it wakes nobody and asks nobody,
the agents take it in as they take in a join.

### Running it

- The terminal follows the command until it ends, showing what it prints as it comes; a key -
  as the one that cancels a turn - ends it (`Shell::kill`). Whether the input line stays free
  meanwhile is to be decided.
- What is recorded is bounded as a tool's result is, and says when output was dropped; the
  terminal may show more than it records.
- A command gets no terminal and no input: one that waits for input or draws a screen (`vim`,
  `less`) does not work, as in Claude Code. Whether such a command is refused or left to
  `max_run_secs` is to be decided.
- A file the command changes is changed for the agents too: an agent that knows the file
  otherwise is told to read it again before writing it (`AgentChannelState::last_known`), as for
  a file a person changed in any other way.

### Who may

A person acts in their own name and asks no approval: approvals are for agents' calls
([APPROVALS.md](../docs/APPROVALS.md)). So whoever may change a workspace's files in a channel
may run any command there. With a `local` storage, whose shell runs on Caelumora's host as its
user ([WORKSPACE.md](../docs/WORKSPACE.md#storages)), that is a shell on the host for every such
person - which an agent has today too, but only through approved calls. The operator's means
against it is the component's place in `AppConfig::commands`; whether more is needed - a
storage parameter, a permission of its own beside `WriteWorkspace` - is to be decided against
[SECURITY.md](../docs/SECURITY.md) and [PERMISSION-CHECKS.md](../docs/PERMISSION-CHECKS.md).

## Done when

- In `caelumora-tui`, a line beginning with `!` runs the rest of it in the session's workspace,
  through the shell of the channel talked in, in the user's name, and shows its output as it
  comes and how it ended; it can be ended by a key.
- It is refused, saying why, where the channel uses no workspace, its storage has no shell, or
  the user may not run it there.
- The run is recorded in the channel's history channel-wide; every agent of the channel takes
  the command, its output and how it ended into its transcript where it happened, and is not
  asked to respond by it.
- The web UI shows the recorded run - and, if so decided, accepts `!` too.
- [COMMANDS.md](../docs/COMMANDS.md), [CHANNELS.md](../docs/CHANNELS.md),
  [AGENT-TURN.md](../docs/AGENT-TURN.md), [WORKSPACE.md](../docs/WORKSPACE.md) and
  [WEB-UI.md](../docs/WEB-UI.md) say what a person's command is, what is recorded of it, and what
  an agent takes in.

## History

2026-11-16: Opened at the maintainer's request: in the terminal, a line beginning with `!` should
run a shell command, as in Claude Code, and the command with its result should enter the history,
so that the agents see it.
