# 583 — A command has no object of its own, and its parameters are a text

**Status:** open
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

## Done when

- Each command is carried out by an object of its own, found in its set without a match of its
  name, and no provider answers a name it does not have.
- A command's parameters are described with types, checked by whoever runs it, and shown and
  completed from that description.
- Whether tools follow is decided, and done or the difference documented in TOOLS.md and
  COMMANDS.md.

## History

**2026-11-15** - Opened with the maintainer while finishing
[574](574-notes-and-memory-cannot-be-read-by-a-command.md), whose fourth step made whoever runs a
command refuse arguments to one that names no parameters: the per-command `match` and the textual
`usage` came up as what stands in the way of more than that.
