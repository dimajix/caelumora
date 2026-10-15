# Commands

People address the system through user commands, as agents act through tools: a command is
contributed by a component and run by an executor that receives the ready providers and checks
permissions. This document describes user commands, the commands an agent carries out itself, and
the commands that come with Caelumora. How components are built is in
[COMPONENTS.md](COMPONENTS.md).

## User commands

A line that starts with `/` addresses the system rather than the agents: `/help`, `/wakeup [agent]`,
`/nudge <participant>`. A command is not a message: it never reaches an agent's context as something
said. Commands of the channel never enter the history; what an agent carried out on someone's word
does (see [Commands of an agent](#commands-of-an-agent)).

Commands are for people; agents act through tools. A command is always issued on behalf of a
particular person, by that person's actor (`UserActor::execute_command`), as messages and approvals
are; only `UserActor` issues commands. What a command does to the channel goes through the channel's
own, permission-checked operations; what it sends in the issuer's name, their actor sends, since an
actor sends only in its own name; a command of an agent is sent to the agent, which carries it out
itself.

### Recognised by the frontend

The client decides whether a line is a command and sends it as one rather than as a message; the
message API stays plain text. The rule is `caelumora_model::command::parse_input`, mirrored in the
web UI: a leading `/` starts a command, whose name runs to the first whitespace; the rest of the
line is handed to the command unparsed; `//` sends a literal `/`. Only this lexical rule is repeated
per frontend. What a command means, who may run it and what it answers is decided in one place.

### Commands are components

A `ChannelCommandProvider` contributes commands run on the channel, as a `ToolProvider` contributes
tools; the issuer is given as their actor. Commands come from two places:

*   `AppConfig::commands`, the global set - by default `/help`, `/wakeup` and `/nudge`; an empty
    list switches them off (see [CONFIGURATION.md](CONFIGURATION.md));
*   `SessionTemplate::commands`, for commands that belong to what a session is for, copied into the
    session, so a running session keeps its set.

Both are built once per session, in the name of its owner
([COMPONENTS.md](COMPONENTS.md#what-a-component-may-reach)), into a `CommandExecutor` shared by the
session's channels - the counterpart of an agent's `ToolExecutor`, which likewise receives ready
providers and is no component. Two commands of one name keep the session from starting. The
executor is generic over the facet: finding a command, refusing two of one name and asking the
policy are the same whoever runs the commands (`visible_by`, `admit`); only what a command is given
to run with - the session, or an agent's turn - and which actions are asked of it are the facet's,
said by the facet's owner: the channel's commands by the runtime (`ShowCommand`, `ExecuteCommand`),
an agent's by the agents' logic (`agent_command::AgentCommandAccess`, `ShowAgentCommand`,
`ExecuteAgentCommand`, issue 370). It checks permissions itself: how a command runs is the
command's business, and there is nothing about running commands to try out.

### Seeing and running

Seeing and running a command are separate permissions: `ShowCommand` decides whether it appears in
`/help` and in completion, `ExecuteCommand` whether it runs. Both carry the command and the
channel's attributes. The ownership policy grants them whoever reads the channel, and a command for
the owners alone the channel's owner.

A command the user may not see is answered exactly like an unknown one, so hidden commands cannot be
found by trying names. Whatever the command does is checked on its own as well: `/wakeup` asks for
`WakeChannel`. `/nudge` asks nothing more: it asks someone to respond, as an `@`-mention does, which
asks nothing of the policy either.

### Private or channel-wide answers

A command answers the issuer alone (`/help`, `/nudge`) or the whole channel (`/wakeup`). A
channel-wide answer is announced by the issuer's actor as `CommandExecuted`, forwarded to the users
only, shown but never stored. The issuer gets the answer back from `execute_command` either way,
since the channel never routes an envelope back to its sender; the REST response carries it only
when it is private, since the web UI receives the announcement on its stream. A command of an agent
answers later, on the stream; an answer for the issuer alone is routed to them alone.

### Naming an agent

Every command names agents and participants alike (`ChannelLoop::agents_named`,
`participants_named`). The text is taken as a name first, compared as names are; only if it is
nobody's name is it taken as a handle, with or without its `@`, normalised as a mention is. A handle
is coarser than a name, so it can name several; a command then refuses with their names rather than
picking one.

## Commands of an agent

Some commands act on an agent rather than on the channel: `/skill` puts a skill into its transcript,
`/compact` compresses it. An agent's state in a channel has one writer, its actor there, so such a
command is not run by the session but carried out by the agent itself - an agent run here and an
external agent alike.

*   **Contributed by components.** A component of the personality offers them through
    `AgentCommandProvider`, as a tool is offered, carried out in the agent's `Turn`. Which commands
    an agent carries out follows from its personality alone. Two of one name, or one that a command
    of the session has, keep the agent from starting.
*   **Always addressed.** A command of an agent names its agent - `/compact @bob`; `@all` names
    every agent of the channel that carries it out. Without a handle, or with one that names no such
    agent, it is refused with the handles to choose from.
*   **Seen and asked for.** `ShowAgentCommand` and `ExecuteAgentCommand` carry the command and the
    attributes of the agent and of the channel. Under the ownership policy they are granted to
    whoever may write in the channel, unless the command is for the agent's owners alone: then to
    whoever answers for the agent, as approving its calls (see
    [SECURITY.md](SECURITY.md#the-ownership-policy)). Who may not see a command finds it unknown.
*   **Sent, then carried out.** The issuer's actor asks the session's commands first, then the
    agents of the channel. Offering asks the policy through the agent's executor directly, so it
    answers at once while the agent is busy; whether the issuer may ask for the command is the
    agent's to decide as it takes the request in. The channel admits a command request only from a
    user to an agent taking part, and the moderator routes it to that agent alone. A quiet channel
    delivers it as well.
*   **Where it came.** The agent takes in what it was told up to the command, carries it out, and
    takes in the rest, so that what was said before and after stands around it in its transcript as
    in the history. A command wakes nobody and is carried out asleep as well; the agent does not
    respond to it. One that comes during a turn waits for it.
*   **In a turn of its own** (see [AGENT-TURN.md](AGENT-TURN.md#a-command-turn)), in the request's
    contexts continued in the agent's name - its owner's rights, the issuer as initiator
    ([SECURITY.md](SECURITY.md#in-whose-name-an-agent-acts)). The executor asks there, in the
    context the request came with, whether the issuer may ask for it: the one place it is asked.
    Refused, the agent answers `CommandFailed` to the issuer. Carried out, the turn is taken over
    and stored; not carried out, it is dropped as if it had never begun.
*   **Begun.** Admitted, the agent tells the issuer alone that it began, whatever its answer's
    visibility: for `/compact`, the issuer sees it begin and everybody sees it done.
*   **Answered.** Carried out, the agent answers `CommandExecuted` - to the issuer alone if the
    answer is private, to every user otherwise; not carried out, `CommandFailed` to the issuer
    alone, with why. A command answers what it did, not what it read: nothing withholds its answer
    as a tool's result is withheld.
*   **Recorded.** What an agent carried out, with its arguments and on whose word, goes into the
    channel's history and the agent's - not what it answered, which may have been the issuer's
    alone. A command refused or failed did nothing and leaves no entry.
*   **Listed.** `/help` lists the agents' commands after the channel's, each with the handles of the
    agents that carry it out; the member snapshots carry them for completion.

## Built-in commands

| Command | Of | Component | Arguments | Answer | What it does |
|---|---|---|---|---|---|
| `/help` | the channel | `help_command` | `[command]` | private | Lists the commands the user may use - the channel's and the agents' -, or describes one. |
| `/wakeup` | the channel | `wakeup_command` | `[agent]` | channel-wide | Wakes the sleeping agents of the channel, or the one named. |
| `/nudge` | the channel | `nudge_command` | `<participant>` | private | Asks the participant named to respond to the conversation as it stands, without a message ([CHANNELS.md](CHANNELS.md#nudging-someone)). |
| `/skill` | the agent | `skills` | `@agent <name>` | private | Gives the agent a skill: its text goes into the agent's transcript in the issuer's name ([SKILLS.md](SKILLS.md)). |
| `/skills` | the agent | `skills` | `@agent` | private | Lists the skills the agent may load here. |
| `/compact` | the agent | `compaction` | `@agent` | channel-wide | For the agent's owners: compacts its context as its `CompactionStrategy` does, now rather than when it is too full - as the agent does on its own ([AGENT-TURN.md](AGENT-TURN.md#compaction)). |
