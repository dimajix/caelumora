# 587 — A person cannot run a command in the workspace

**Status:** done
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

### The terminal alone offers it

The web UI does not get it (decided by the maintainer): a shortcut to the shell belongs to a
program a person runs on their own machine, in the project they work on, not to a page served by
a server. So nothing a frontend sends as a command of the channel may run it either -
`/workspace run` offered by a component would be listed by the web UI's `/help` and run from
there.

Still, what the agents are to see has to enter the channel's history: through the user's actor,
admitted by the channel, recorded, and perceived by the agents (see
[CHANNELS.md](../docs/CHANNELS.md#acting-in-a-channel)). The terminal cannot do that itself, so
the kernel needs a kind of its own for a person's command and its record, which the terminal
alone calls.

### Where the command runs

A bang is commonly a shortcut to the system - the shell of the machine the program runs on. In
`caelumora-tui` the session's workspace is exactly that: its storage `directory` runs commands
on the same machine, as the same user, in the directory the program was started in, with the
program's environment (`shell: true`, `environment: inherit` in the defaults of
`caelumora-tui/src/home.rs`), under the policy `allow_all`. So the options differ not in where
a command runs, but in who runs and bounds it:

- **A. In the session's workspace, through the channel's shell**, as an agent's command runs:
  `WorkspaceController::shell_in` asks what it asks of an agent - reading the session and the
  channel, the workspace usable in the channel, `WriteWorkspace` - in the person's name. The
  command is then the channel's ([WORKSPACE.md](../docs/WORKSPACE.md#commands)): bounded by
  the storage's `max_run_secs`, ended by the `CommandReaper` when nobody may run it any longer,
  ended when the session stops or the program shuts down in order (issue
  [297](297-orderly-shutdown.md)). Its output is classified as the workspace, which flows into
  every channel that uses it. The kernel ran it, so what is recorded is what came out. And it
  runs where the agents' commands run, so an agent running it again finds the same.
- **B. By the terminal itself, as a process of its own.** Passes by the bounds of the commands
  and by the classification, and what is recorded is the terminal's word for what came out, not
  the kernel's. It would work where the workspace's shell is switched off (`shell: false`), or
  where no workspace is used - but there the agents could not run the command either, and what
  they are shown of it could not be checked against anything. Rejected.

Recommended: A. Where the channel uses no workspace, or its storage has no shell, the command is
refused, saying why. A terminal configured with another storage - a container of
`open_terminal_docker` - runs it there, where its agents run theirs.

### How it is issued

The terminal recognises a line beginning with `!` before it classifies it by
`caelumora_domain::command::parse_input`, which stays as it is: it is the rule the web UI
mirrors, and the web UI does not get `!`. The rest of the line is handed to the user's actor -
which runs the command through the channel's shell and has its record sent, as it carries out
and announces a command of the channel (`UserActor::execute_command`) -, so that running a
person's command and recording it are owned by one place in the kernel, not by the terminal.
Whether this is a method of `UserActor` beside `execute_command`, and how it is named, is to be
decided with the interface.

To be decided as well:

- how a message beginning with a literal `!` is written - `!!`, as `//` writes a literal `/`,
  or no escape at all;
- how a person learns of `!` - a line of `/help` in the terminal, as it lists the terminal's own
  commands.

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

Recommended: B. Its structured form lets the terminal show a person's command as it shows an
agent's, by the `shell` presentation (issue
[586](586-the-terminal-shows-a-tool-call-by-its-raw-arguments.md)); an entry of its own beside
`CommandExecuted` is to be weighed against a second way of recording a command. The web UI, which
does not issue the command, has to read such an entry in a history all the same, and show it or
pass it by - to be decided with the entry.

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
may run any command there - in the terminal program the one person who runs it, on their own
machine, which is what a bang is for. No server offers it: the REST server maps no route to it,
so a `local` storage, whose shell runs on Caelumora's host
([WORKSPACE.md](../docs/WORKSPACE.md#storages)), gives nobody a shell there through it. The
kernel's operation asks what an agent's command asks (`shell_in`); no permission of its own is
planned - to be checked against [SECURITY.md](../docs/SECURITY.md) and
[PERMISSION-CHECKS.md](../docs/PERMISSION-CHECKS.md) with the interface.

## Done when

- In `caelumora-tui`, a line beginning with `!` runs the rest of it in the session's workspace,
  through the shell of the channel talked in, in the user's name, and shows its output as it
  comes and how it ended; it can be ended by a key.
- It is refused, saying why, where the channel uses no workspace, its storage has no shell, or
  the user may not run it there.
- The run is recorded in the channel's history channel-wide; every agent of the channel takes
  the command, its output and how it ended into its transcript where it happened, and is not
  asked to respond by it.
- The web UI does not offer it, and neither does the REST server; `parse_input` is unchanged.
- [COMMANDS.md](../docs/COMMANDS.md), [CHANNELS.md](../docs/CHANNELS.md),
  [AGENT-TURN.md](../docs/AGENT-TURN.md) and [WORKSPACE.md](../docs/WORKSPACE.md) say what a
  person's command is, what is recorded of it, and what an agent takes in.

## History

2026-11-16: Opened at the maintainer's request: in the terminal, a line beginning with `!` should
run a shell command, as in Claude Code, and the command with its result should enter the history,
so that the agents see it.

2026-11-16: The maintainer decided that the web UI does not get `!`. So it is no command of the
channel and no rule of `parse_input`; the terminal recognises it and hands it to the user's
actor. Where it runs was questioned - a bang being a shortcut to the system -, and kept in the
workspace's shell: in the terminal program that is the system, the same machine, user,
directory and environment, with the bounds of the commands besides.

2026-11-16: The plan decided with the maintainer departs from two recommendations above. What
enters the history is the person's message carrying a call of `run_command` with its result, the
same shape as an agent's call (the maintainer's proposal), not a record of its own: the history,
its readers, the REST API and the web UI's `shell` presentation take it as it is, and a tool's
result is checked as it enters. It costs two rules: a person's calls and results are heard by the
others, and given to a model in words; a message without words or attachments asks nobody to
respond. And the command is run by a command of the channel, `/workspace run`, of a component
`workspace` that no deployment has by default and the terminal's configuration adds - so the web
UI does not get it by configuration, not by a rule of the terminal; `!` is its short form in the
terminal, `!!` writes a literal `!`. Its output is shown once it ended; Ctrl-C ends a command under
way, by dropping it, and a command so ended is not recorded - accepted for now. Step 1 moves what
the workspace's shell tool and the command share into `caelumora_api::terminal`.

2026-11-16: A step 1b added at the maintainer's request, from a finding of the review of step 1:
the guard that kills a dropped call's command and the one that ends a dropped call's prompt in
ComfyUI were one mechanism written twice; both are `caelumora_api::guard::EndOnDrop` now.

2026-11-16: Step 2 - the kernel's part. `/workspace run` is a command of the channel, of a
component of the kind `shell`: `workspace`, the name of its group, is the kind of the agents' tools
on the workspace already. It sends the run as the issuer's call (`UserActor::send_call`); a
person's calls are heard with their results (`Message::as_heard`) and given to a model in words
(`kernel.calls_made`); a message that says nothing in words or attachments asks nobody to respond
(`Message::says_something`).

2026-11-16: A step 2b added at the maintainer's request, from a finding of the review of step 2:
`read_history`, which reads another channel, gave only what was said in words, so an agent reading
a channel from outside never saw a person's command that the agents in it heard. It reads a
person's calls now in the words the resolver gives them (`calls_in_words`), classified with what
they answered (`Message::classification_with_results`, which `Work::see_message` uses too).

2026-11-16: The maintainer decided that `read_history`, which searches the words of a message,
is to search a person's calls by their description once issue
[586](586-the-terminal-shows-a-tool-call-by-its-raw-arguments.md) is done and merged: a call has
no description yet, only its arguments. The finding that a person's call is released without its
result became issue [588](588-a-person-s-call-is-released-without-its-result.md).

2026-11-16: Step 3 - the terminal's part. A line beginning with `!` is `/workspace run` with the
rest of it, `!!` a message beginning with `!` (`read_input` in `caelumora-tui/src/typeable.rs`);
the terminal's defaults add the component `shell`. The run is a task the talk follows beside the
channels, one at a time; Ctrl-C aborts it, which kills the command, and a switch to another
session does the same, while in another channel of the session how it ended is told as of the
channel it ran in. A call the user made is printed as the channel tells it, wherever they talk,
though what is sent in their name is not printed otherwise. Where the channel offers no
`/workspace run`, `!` says so. Until issue 586 the terminal shows the result of the call by its
first line only.

2026-11-16: Steps 1 to 3 merged into `main`; in progress until `read_history` searches a person's
calls by their description, after issue 586.

2026-11-16: Step 4 - before the search, decided with the maintainer: a person's call had no
description, which only the agent's `ToolExecutor::admit` stamps, from its tool. `/workspace run`
describes its call as the workspace's tool describes one, by one function of
`caelumora_api::terminal`, `run_description`; the terminal tells a person's call by it, as an
agent's.

2026-11-16: Step 5 - `read_history` searches a person's calls by their description: a query finds
the messages whose words contain it, or that carry a call of a person whose description does
(`Filter::with_a_call_containing`, over `HistoryField::CallDescription` within `ToolCalls`). A
call stored before step 4 has no description, and no query finds it. What the model reads of a
call found stays its name and arguments (`kernel.calls_made`), so that a hit on the fixed words of
the description - "workspace" - shows none of them; left so, as decided with the maintainer.

2026-11-16: Done.
