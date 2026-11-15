# 583 — A command has no object of its own, and its parameters are a text

**Status:** done
**Type:** refactoring
**Priority:** medium
**Area:** Code structure
**Summary:** A command is a record (`Command`) beside a provider that carries out all of its commands by their names, so every group of commands matches the name a second time, keeps a constant to tie the two together and answers a name it does not know though nobody can give it one; and what a command takes is a text shown to people (`Command::usage`), from which nothing but whether it takes anything can be read.

## The problem

**What carries a command out.** A component contributes commands through a facet -
`ChannelCommandProvider` for the channel's, `AgentCommandProvider` for an agent's
([COMMANDS.md](../docs/COMMANDS.md#commands-are-components)) - which says what it provides as a
list of `Command` records (`ProvidesCommands::commands`) and carries one out by its name
(`execute(name, args, …)`). A provider of several commands therefore dispatches by name:
`ChannelGroupComponent` matches ten names in `execute`, beside the list of the same ten in
`commands`; `AgentGroupComponent`, `ParticipantGroupComponent` and `skills` do the same. In the
kernel each command already has a file of its own with its `command()` and its `execute()`
(`command/channel/notes.rs`); what binds them is the `match` of their group and a constant of the
name both use. Every provider answers a name it does not have with `CommandError::Unknown`, which
no test can reach: whoever runs a command finds its provider by the name the provider declared
(`CommandSet`).

**What a command takes.** `Command::usage` is a text - `[agent]`, `<personality> [as <name>]` -,
shown by `/help` and by the web UI's completion (`ChatView`), and since issue 574 the one thing
read from it is whether it is there at all: a command without one takes no arguments, which
whoever runs it refuses (`caelumora_api::command::check_arguments`). What a command does take it
parses in `execute` - a name sought with `named_one`, a channel with `channel_named`, a text as
a `Name` -, each in its own way, and nothing beside the command knows its parameters: the
terminal cannot complete them
([579](579-commands-and-mentions-are-not-completed-in-the-terminal.md)), and `/help` can only
repeat the text.

Tools have the same two halves in another state: a `ToolProvider` carries out its tools by name
(`execute(name, params, …)`) with a `match` of its own, but a tool's parameters are typed
(`ToolParameters`), checked and described from that.

## Considerations

- **A command as an object.** A trait per kind of command - `ChannelCommand` with `command()` and
  `execute(args, context)`, an agent's with its `Turn` -, a provider giving a list of them, and
  `CommandSet` holding the command itself in each entry. The matches, the constants of the names
  and the `Unknown` of every provider go; each command's file becomes its type. It changes the
  facets of every provider: the kernel's groups and `help`, `skills`, `compaction`,
  `agent_memory`, `private_notes`, and the fakes of the tests.
- **Typed parameters.** `usage` becomes a description of the parameters - each with its name,
  whether it is required, what it is (a participant, an agent of the session, a channel, a
  personality, a free text) -, as `ToolParameters` describes a tool's. Whoever runs a command
  checks it against them as `check_arguments` checks their absence now; `/help` writes the usage
  from them; the terminal (579) and the web UI complete by them. Where a command parses its
  arguments - finding whom they name - stays the command's: a check before it would seek the same
  twice. The REST API gives the commands to the web UI with their usage, so the web UI changes
  with it.
- **Tools.** Either they follow - a tool as an object of its own beside its provider -, or
  commands and tools stay different, with the reason named (AGENTS.md, "Follow the pattern, or
  change it"). A tool's parameters are typed already, and the cost of the first half is the same.
- In which order: the object first, since the parameters belong to it; or both at once, since
  every provider is touched by either.
- **A rule for CODING-GUIDELINES.md**, at the maintainer's word: a `match` that serves as a
  multiplexer - dispatching by a name or a kind to functions that are different in themselves,
  each case calling one of them - is a smell; whoever writes or meets one weighs giving each case
  a type of its own, or a trait the cases implement, so that what belongs to one case lies
  together and the cases are found without matching again. A `match` over the variants of a state
  that every case handles in the same respect ("States are types") is no such multiplexer. The
  `execute` of the providers of commands and of tools are the examples the rule is written
  from.

Decided (2026-11-15), with the maintainer, from what a command is rather than from the code as it
stands:

| Concept | What it is | Where |
|---|---|---|
| `CommandDescription` | The record of one command: its name and short forms, what it does, its parameters, whether it is for an agent's owners alone; what the policy, the snapshots and the REST API carry. The record called `Command` until now, renamed everywhere. | `caelumora-domain` |
| `Command` | The trait every executable command implements: it gives its description (`description() -> &CommandDescription`), which it holds. No `execute`: what a command runs with differs by its kind. | `caelumora-api` |
| `ChannelCommand`, `AgentCommand`, `TerminalCommand` | The trait of each kind, derived from `Command`, with its `execute(args, …)` - given the channel's `CommandContext`, the agent's `Turn`, the terminal's own context. | the kernel, `caelumora-agents`, `caelumora-tui` |
| `CommandProvider<C>` | The facet of a component: the commands of the kind `C` it contributes, as objects. One generic facet for every kind. | `caelumora-api` |
| `CommandSet<C>` | The commands of whoever runs them - a session's, an agent's, the terminal's -, holding the commands themselves: finds the one a line begins, refuses two a line cannot tell apart. | `caelumora-api` |

In five steps:

1.  **The rule, and commands as objects.** The rule on a `match` as a multiplexer goes into
    CODING-GUIDELINES.md; `Command` is renamed `CommandDescription`; the traits above replace
    `ProvidesCommands`, `ChannelCommandProvider` and `AgentCommandProvider`, and every command of
    the channel and of an agent becomes a type of its own. No behaviour changes.
2.  **The command found is the one run.** `CommandExecutor::find` gives the command it found,
    not its name, and `execute` runs that command, instead of looking it up by its name again.
3.  **The terminal's commands follow.** `TerminalCommand`, an enum dispatched by a `match` in
    `talk.rs` today, becomes the trait of the terminal's commands, found in a `CommandSet` of
    them.
4.  **What step 3 came across.** What `/help` is asked about is decided in one place for the
    channel's `/help` and the terminal's; finding by the beginning of an id gets a module of its
    own in the terminal.
5.  **Typed parameters** in `CommandDescription`, replacing `usage`. Whether the kind of a
    parameter comes now or with 579, which is the first to need it, and how the keyword of
    `/agent create <personality> [as <name>]` is described, is decided before the step: see the
    history.

Tools follow in an issue of their own, [584](584-a-tool-has-no-object-of-its-own.md), for their
size: 25 providers in nine crates, each matching names in several methods.

## Done when

- Each command is carried out by an object of its own, found in its set without a match of its
  name, and no provider answers a name it does not have.
- A command's parameters are described with types, checked by whoever runs it, and shown and
  completed from that description.
- The terminal's commands are carried out by objects of their own too.
- CODING-GUIDELINES.md holds the rule on a `match` as a multiplexer.

## History

**2026-11-15** - Opened with the maintainer while finishing
[574](574-notes-and-memory-cannot-be-read-by-a-command.md), whose fourth step made whoever runs a
command refuse arguments to one that names no parameters: the per-command `match` and the textual
`usage` came up as what stands in the way of more than that.

**2026-11-15** - The maintainer added a rule for CODING-GUIDELINES.md to it: a `match` as a
multiplexer of functions different in themselves is a smell, to be weighed against a type or a
trait per case.

**2026-11-15** - Decided with the maintainer before the work began: the concepts in the table
above, in three steps; tools follow in [584](584-a-tool-has-no-object-of-its-own.md).

**2026-11-15** - Step 1 done: `CommandDescription` (was `Command`),
`caelumora_api::command::Command` and `CommandProvider<C>`, `ChannelCommand` in the kernel,
`AgentCommand` in `caelumora-agents`;
every command of the channel and of an agent is a type of its own, and `CommandSet<C>` holds the
commands themselves. Beside the plan: a component contributes its commands as the list it made
(`CommandProvider` for `Vec<Arc<C>>`), so the components that only held commands - `/help`'s and
those of the three groups - are gone; and a type's name in a message is written without the paths
of the types it is made of (`TypeKey::name`), so that `CommandProvider<ChannelCommand>` is named
so.

**2026-11-15** - Decided with the maintainer after step 1: `CommandProvider<C>` stays a trait,
though every provider is the list of `Vec<Arc<C>>`, as the pattern of a facet; and a step of its
own, now step 2, makes `CommandExecutor::find` give the command it found, which `execute` runs,
instead of the name it is looked up by again.

**2026-11-15** - Step 2 done: `CommandExecutor::find` gives the command it found, and `execute`
runs that command, asking `ShowCommand` and `ExecuteCommand` of it, instead of looking it up by its
name again; `CommandSet::named` is left to an agent's actor, which a request reaches by the
command's name. The executor's `find`, `execute`, `colliding` and `visible` are the kernel's alone
(`pub(crate)`), so that no other crate hands `execute` a command `find` did not give.

**2026-11-15** - Step 3 done: `TerminalCommand` is the trait of the terminal's commands, carried
out in the user's `Talk`, each a type of its own in `caelumora-tui/src/commands/`, one file per
group - `/approval approve` and `deny` one type, differing in the answer -, found in a
`CommandSet`. Since no component contributes them, the set is made by `CommandSet::of`, which
checks them as `new` checks those of components. `/help` lists the terminal's commands by name,
as it lists the channel's, no longer `/exit` first.

**2026-11-15** - Decided with the maintainer after step 3: what its review came across is step 4,
before the typed parameters, now step 5.

**2026-11-15** - Step 4 done: `help_topic` in `caelumora_domain::command` is what `/help` is asked
about - every command, or the name its words make, of one command (`CommandDescription::is_named`)
or of a group -, for the channel's `/help` and the terminal's, which repeated it; `beginning_with`
and `NotNamed` lie in a module of their own in `caelumora-tui`, `id_prefix`, no longer among the
commands, none of which uses them.

**2026-11-15** - Decided with the maintainer before step 5: a parameter is described by its name,
whether it is required, and the word that introduces it, if one does - `as` of `/agent create` is
such a keyword, an attribute of the parameter rather than a parameter of its own, since a keyword
and what it introduces are given together, which a flat list of parameters could not say without
groups; the kind of a parameter comes with
[579](579-commands-and-mentions-are-not-completed-in-the-terminal.md), the first to read it. The
web UI writes the usage from the parameters itself.

**2026-11-15** - Step 5 done: `CommandParameter` in `caelumora-domain` in place of the text
`usage`, written by `CommandDescription::usage` for `/help` and by `usageOf` in the web UI;
`check_arguments` refuses a command with a required parameter given none, so the commands that
checked that themselves no longer do, and the refusal reads alike for all of them
("it takes <channel>"). `/channel switch` takes its channel as optional (`[channel]`), since
without one it tells where the user could talk.

**2026-11-15** - Done with step 5. What "Done when" asks beyond it - the kind of a parameter, and
completing from the description - went to
[579](579-commands-and-mentions-are-not-completed-in-the-terminal.md), the first to read the kind.
