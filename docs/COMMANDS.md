# Commands

People address the system through user commands, as agents act through tools: a command is
contributed by a component and run by an executor that receives the ready providers and checks
permissions. This document describes user commands, the commands an agent carries out itself, and
the commands that come with Caelumora. How components are built is in
[COMPONENTS.md](COMPONENTS.md).

## User commands

A line that starts with `/` addresses the system rather than the agents: `/help`, `/wakeup [agent]`,
`/nudge <participant>`, `/add <name>`. A command is not a message: it never reaches an agent's
context as something said. Commands of the channel never enter the history; what an agent carried
out on someone's word does (see [Commands of an agent](#commands-of-an-agent)).

Commands are for people; agents act through tools. A command is always issued on behalf of a
particular person, by that person's actor (`UserActor::execute_command`), as messages and approvals
are; only `UserActor` issues commands. What a command does to the channel goes through the channel's
own, permission-checked operations; what it sends in the issuer's name, their actor sends, since an
actor sends only in its own name; a command of an agent is sent to the agent, which carries it out
itself.

### Recognised by the frontend

The client decides whether a line is a command and sends it as one rather than as a message; the
message API stays plain text. The rule is `caelumora_domain::command::parse_input`, mirrored in the
web UI: a leading `/` starts a command, whose name runs to the first whitespace; the rest of the
line is handed to the command unparsed; `//` sends a literal `/`. Only this lexical rule is repeated
per frontend. What a command means, who may run it and what it answers is decided in one place.

### Commands are components

A `ChannelCommandProvider` contributes commands run on the channel, as a `ToolProvider` contributes
tools; the issuer is given as their actor. Commands come from two places:

*   `AppConfig::commands`, the global set - by default `/help`, `/wakeup`, `/nudge`, `/add` and
    `/create-agent`; an empty list switches them off (see [CONFIGURATION.md](CONFIGURATION.md));
*   `SessionTemplate::commands`, for commands that belong to what a session is for, copied into the
    session, so a running session keeps its set.

Both are built once per session, in the name of its owner
([COMPONENTS.md](COMPONENTS.md#what-a-component-may-reach)), into a `CommandExecutor` shared by the
session's channels - the counterpart of an agent's `ToolExecutor`, which likewise receives ready
providers and is no component. Two commands of one name keep the session from starting. What the
executor finds its commands in is a `CommandSet`, generic over the facet: finding a command and
refusing two of one name are the same whoever runs the commands, and an agent's actor keeps its
commands in one too. What a command is given to run with - the session, or an agent's turn - and
who asks whether it may be seen and run are the facet's: the channel's commands the executor
itself (`ShowCommand`, `ExecuteCommand`), an agent's the channel's door as it admits the request
(`ShowAgentCommand`, `ExecuteAgentCommand`, issue 516). How a command runs is the command's
business, and there is nothing about running commands to try out.

### Seeing and running

Seeing and running a command are separate permissions: `ShowCommand` decides whether it appears in
`/help` and in completion, `ExecuteCommand` whether it runs. Both carry the command and the
channel's attributes. The ownership policy grants them whoever reads the channel, and a command for
the owners alone the channel's owner.

A command the user may not see is answered exactly like an unknown one, so hidden commands cannot be
found by trying names. Whatever the command does is checked on its own as well: `/wakeup` asks for
`WakeChannel`. `/nudge` asks nothing more: it asks someone to respond, as an `@`-mention does, which
asks nothing of the policy either. `/add` asks what taking someone in asks
(`SessionLoop::add_participant`: `UpdateSession` unless they are in the session already,
`UpdateChannel`), and `/create-agent` what making an agent asks (`AgentManager::create`), so
whoever reads the channel sees both, and those without the right are refused on running them.

### Private or channel-wide answers

A command answers the issuer alone (`/help`, `/nudge`, `/add`, `/create-agent`) or the whole
channel (`/wakeup`): that `/add` brought someone in, the channel tells everyone anyway. A
channel-wide answer is announced by the issuer's actor as `CommandExecuted`, forwarded to the users
only, shown but never stored. The issuer gets the answer back from `execute_command` either way,
since the channel never routes an envelope back to its sender; the REST response carries it only
when it is private, since the web UI receives the announcement on its stream. A command of an agent
answers later, on the stream; an answer for the issuer alone is routed to them alone.

### Naming an agent

Every command that names someone taking part in the channel names agents and participants alike
(`ChannelLoop::agents_named`, `participants_named`). The text is taken as a name first, compared as
names are; only if it is nobody's name is it taken as a handle, with or without its `@`, normalised
as a mention is. A handle is coarser than a name, so it can name several; a command then refuses
with their names rather than picking one.

Who is to be brought into the channel does not take part in it yet, so `/add` looks further, by the
whole name (`SessionView::candidates`, issue
[555](../issues/555-no-command-brings-an-agent-into-a-channel.md)): among the users and agents the
issuer may read, the nearest from the channel - the channel's scope, the session with its
participants wherever they lie, the issuer's own agents, the global scope with the users. One that
is shadowed by a nearer one of the same name is not reached by the command; the web UI's dialog
still lists it. Several equally near - a user and a global agent of one name - are refused with
their names. An agent of another user that does not take part in the session is not found, and a
name the issuer may not read is answered like one that does not exist. `/create-agent` finds its
personality the same way from the channel (`Views::personality_named`), the innermost first. A
command that takes two names takes the last `as` as what separates them: `/create-agent Reviewer as
Bob`.

## Commands of an agent

Some commands act on an agent rather than on the channel: `/skill` puts a skill into its transcript,
`/compact` compresses it. An agent's state in a channel has one writer, its actor there, so such a
command is not run by the session but carried out by the agent itself - an agent run here and an
external agent alike.

*   **Contributed by components.** A component of the personality offers them through
    `AgentCommandProvider`, as a tool is offered, carried out in the agent's `Turn`. Which commands
    an agent carries out follows from its personality alone. Two of one name, or one that a command
    of the session has, keep the agent from starting.
*   **Always addressed.** A command of an agent names its agent - `/compact @bob`; `@all`, or
    `@channel`, as for a mention, names every agent of the channel that carries it out. Without a
    handle, or with one that names no such agent, it is refused with the handles to choose from.
*   **Seen and asked for.** `ShowAgentCommand` and `ExecuteAgentCommand` carry the command and the
    attributes of the agent and of the channel. Under the ownership policy they are granted to
    whoever may write in the channel, unless the command is for the agent's owners alone: then to
    whoever answers for the agent, as approving its calls (see
    [SECURITY.md](SECURITY.md#the-ownership-policy)). Who may not see a command finds it unknown.
*   **Sent, then carried out.** The issuer's actor asks the session's commands first, then the
    agents of the channel. Which of an agent's commands the issuer sees, the channel answers from
    the agent's list (`ChannelLoop::commands_of`), so it answers at once while the agent is busy.
    Whether the issuer may ask for the command is decided at the channel's door, as it admits the
    request (issue 516): only from a user, to an agent taking part that carries the command out,
    and only if the issuer may see it and ask for it; what is refused is answered to the issuer
    alone (`MessageRejected`). The moderator routes an admitted request to that agent alone. A
    quiet channel delivers it as well; one read back from the history for the agent is admitted
    again, with the issuer's rights as they are then, and left out if it is not - the issuer is
    not told, having been answered when it entered.
*   **Where it came.** The agent takes in what it was told up to the command, carries it out, and
    takes in the rest, so that what was said before and after stands around it in its transcript as
    in the history. A command wakes nobody and is carried out asleep as well; the agent does not
    respond to it. One that comes during a turn waits for it.
*   **In a turn of its own** (see [AGENT-TURN.md](AGENT-TURN.md#a-command-turn)), in the request's
    contexts continued in the agent's name - its owner's rights, the issuer as initiator
    ([SECURITY.md](SECURITY.md#in-whose-name-an-agent-acts)). The agent asks nothing: what reached
    it was admitted. Carried out, the turn is taken over and stored; not carried out - the command
    failed, or the agent has none of that name -, it is dropped as if it had never begun, and the
    agent answers `CommandFailed` to the issuer.
*   **Begun.** The agent tells the issuer alone that it began, whatever its answer's visibility:
    for `/compact`, the issuer sees it begin and everybody sees it done.
*   **Answered.** Carried out, the agent answers `CommandExecuted` - to the issuer alone if the
    answer is private, to every user otherwise; not carried out, `CommandFailed` to the issuer
    alone, with why. A command answers what it did, not what it read: nothing withholds its answer
    as a tool's result is withheld.
*   **Recorded.** What was carried out - by an agent, with its arguments and on whose word, or by
    the session (`/wakeup`) - goes into the channel's history with what it answered, to whom it was
    answered: the issuer alone for a private answer, every user otherwise (issue 417). A command
    refused or failed did nothing; that it did not is recorded to the issuer alone
    (`CommandFailed`).
*   **Listed.** `/help` lists the agents' commands after the channel's, each with the handles of the
    agents that carry it out; the participant snapshots carry them for completion.

## Built-in commands

| Command | Of | Component | Arguments | Answer | What it does |
|---|---|---|---|---|---|
| `/help` | the channel | `help` | `[command]` | private | Lists the commands the user may use - the channel's and the agents' -, or describes one. |
| `/wakeup` | the channel | `wakeup` | `[agent]` | channel-wide | Wakes the sleeping agents of the channel, or the one named. |
| `/nudge` | the channel | `nudge` | `<participant>` | private | Asks the participant named to respond to the conversation as it stands, without a message ([CHANNELS.md](CHANNELS.md#nudging-someone)). |
| `/add` | the channel | `add` | `<name>` | private | Brings the user or agent named into the channel, and into the session first if not in it yet; they see the whole history ([CHANNELS.md](CHANNELS.md#taking-someone-into-the-session)). |
| `/create-agent` | the channel | `create-agent` | `<personality> [as <name>]` | private | Makes an agent of the personality named, in the session's scope - in the channel's for a personality of the channel's scope -, named after the personality unless named; `/add` brings it in. |
| `/skill` | the agent | `skills` | `@agent <name>` | private | Gives the agent a skill: its text goes into the agent's transcript in the issuer's name ([SKILLS.md](SKILLS.md)). |
| `/skills` | the agent | `skills` | `@agent` | private | Lists the skills the agent may load here. |
| `/compact` | the agent | `compaction` | `@agent` | channel-wide | For the agent's owners: compacts its context now rather than when it is too full, as the agent does on its own ([AGENT-TURN.md](AGENT-TURN.md#compaction)). |
