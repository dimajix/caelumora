# Commands

People address the system through user commands, as agents act through tools: a command is
contributed by a component and run by an executor that receives the ready providers and checks
permissions. This document describes user commands, the commands an agent carries out itself, and
the commands that come with Caelumora. How components are built is in
[COMPONENTS.md](COMPONENTS.md).

## User commands

A line that starts with `/` addresses the system rather than the agents: `/help`, `/agent wakeup
[agent]`, `/participant nudge <participant>`, `/participant add <name>`. A command is not a message:
it never reaches an agent's context as something said. Of the commands of the channel, the history
keeps a channel-wide answer, and what one sends in the issuer's name as theirs - the call of
`/workspace run` ([below](#a-command-run-in-the-workspace)); what an agent carried out on someone's
word it keeps too (see [Commands of an agent](#commands-of-an-agent)).

Commands are for people; agents act through tools. A command is always issued on behalf of a
particular person, by that person's actor (`UserActor::execute_command`), as messages and approvals
are; only `UserActor` issues commands - but for those the terminal carries out itself, which act on
the terminal alone or through the same actor ([below](#commands-of-the-terminal)). What a command
does to the channel goes through the channel's
own, permission-checked operations; what it sends in the issuer's name, their actor sends, since an
actor sends only in its own name; a command of an agent is sent to the agent, which carries it out
itself.

### Recognised by the frontend

The client decides whether a line is a command and sends it as one rather than as a message; the
message API stays plain text. The rule is `caelumora_domain::command::parse_input`, mirrored in the
web UI: a leading `/` starts a command, and the rest of the line is sent, trimmed; `//` sends a
literal `/`. Only this lexical rule is repeated per frontend, in the web UI how a command's
parameters are written (`usageOf`, see below), and in the terminal what completes a command (issue
579): from the domain's rules - a name begun by what is typed (`CommandName::is_begun_by`), an
agent's command naming its agent by its first word (`command::addressed`, as the agents' actors
read it) -, its first argument by its kind, until the keyword of a later parameter is typed. Which
command of the channel a line is, what it means, who may run it and what it answers is decided in
one place.

### Commands of the terminal

The terminal (`caelumora-tui`) carries out a few commands itself, of what the channel's commands do
not offer - end the program, switch the channel or the session it talks in, follow a thread of the
channel, list, start, resume or delete the sessions of its directory, answer an approval or
questions - the one asked without an
argument, else the one whose id begins so (`beginning_with`, issue
[571](../issues/571-approvals-and-questions-in-the-terminal.md)) -, ask an agent to answer again
(issue [570](../issues/570-the-terminal-s-commands-and-its-sessions.md)). Each is an object of its
own, a `TerminalCommand` ([below](#commands-are-components)), found in the terminal's `CommandSet`,
which no component contributes to (`CommandSet::of`). They are named by the rules below as the
channel's are, and a line is tried against them before it goes to the channel: so no name or short
form of them may begin, or be the beginning of, one of the channel's - which a test holds for the
commands a session has by default, not for those a configuration, a template or an agent adds. One
that takes no arguments refuses them, by the rule of every command
([below](#commands-are-components)). `/help` lists them after the channel's, and tells of one of
them, or of a group of them, where the channel knows nothing of what is asked or no channel is
talked in. `/exit` (`/quit`) belongs to no group, as `/help` does: ending the program acts on no
entity of a channel.

In a channel, the terminal follows one thread at a time (issue 595): its line, or the one
`/thread follow` chose - by its number in the list `/thread list` showed last, or the beginning of
its id. It prints what the view of that thread shows (`Envelope::is_shown_in_line`,
`Envelope::is_shown_in_thread`), and, wherever it follows, the approvals and questions waiting for
the user, which they answer from any thread, and what addresses them; what is typed goes to the
thread followed. A thread followed that closes - or found closed as the channel is caught up with -
leaves the user in the line again, told so, also from a channel not talked in.

A line beginning with `!` is `/workspace run` with the rest of it, in the terminal alone (issue
[587](../issues/587-a-person-cannot-run-a-command-in-the-workspace.md), `read_input`): the rule
`parse_input` - which the web UI mirrors - stays as it is, and `!!` writes a literal `!`, as `//`
writes a `/`. Nothing after `!` is completed. The terminal runs the command beside the channels it
follows, so that the user talks on until it ends - in another channel too, where how it ended and
the call are told as of the channel it ran in; one at a time. How it ended may be told before the
call. Ctrl-C ends the command under way rather than the program, and nothing is sent of it; a
switch to another session ends it too. `/help` tells of `!` after the terminal's commands, also
where the channel does not offer `/workspace run`, which `!` then says.

### Names

A command's name is a `CommandName`: its words, from its group to itself (`channel list`), of lower
case letters, digits and `-`, separated by single spaces (issue
[560](../issues/560-commands-are-not-grouped-by-what-they-act-on.md)). A command may have other
names, its short forms (`CommandDescription::aliases`). A line begins with a name if the name's
words begin it, whatever whitespace runs between them there; what follows is the command's
arguments, handed to it unparsed.

No name or short form of a set of commands is the beginning of another - `channel` beside
`channel list`, two equal names, a short form equal to another's name -, so that a line begins at
most one command of the set and no rule of the longest is needed. A set that breaks this refuses to
be built (`CommandSet::new`, `CommandSet::of`), as does a name the code gave unchecked
(`CommandName::fixed`). What is recorded and announced of a command is its name, whichever it was
issued by.

The commands of the channel are grouped by the kind of entity they act on, as command line tools
group theirs - `participant`, `agent`, `channel` -: the group is a command's first word, nothing of
its own, so that any component may contribute to any group. `/help <group>` lists the commands of a
group. What is done often keeps a short form of one word - `/add`, `/nudge`, `/wakeup` -: a second
way of issuing the same command, kept deliberately for a line typed while talking. `/help` belongs
to no group. The commands of an agent are not grouped.

### Commands are components

A command is an object of its own (issue 583): it holds its `CommandDescription` - its names, what
it does, what it takes, whether it is for an agent's owners alone - and gives it as a `Command`, the
trait every command implements; how it is carried out is the trait of its kind, derived from
`Command`: `ChannelCommand` for one run on the channel, given the issuer's actor and the channel,
`AgentCommand` for one an agent carries out in its `Turn`, `TerminalCommand` for one the terminal
carries out itself in the user's `Talk`. A component contributes commands of a kind through the one
facet `CommandProvider<C>`, as a list of them - mostly made as it is built, `Vec<Arc<C>>` being
one. No provider dispatches by a command's name: whoever runs a command finds the command itself.
The channel's commands come from two places:

*   `AppConfig::commands`, the global set - by default `/help` and the groups `participant`,
    `agent` and `channel`, a component each; an empty list switches them off (see
    [CONFIGURATION.md](CONFIGURATION.md)). The group `workspace`, of the component `shell` -
    `workspace` is the kind of the agents' tools on it -, is in no default: a command run with it
    acts wherever the workspace's storage reaches, so the terminal program, one person's on their
    own machine, adds it to the defaults of its configuration, and a server only where its operator
    does;
*   `SessionTemplate::components`, the components of a session, for commands that belong to what a
    session is for, copied into the session, so a running session keeps its set - also through a
    bundle (`ref`, see [COMPONENTS.md](COMPONENTS.md#sharing-components-between-personalities)):
    the seed's `session-basics` brings `/channel notes` of the notes' plugin to every session of
    its templates, and what is added to it reaches them when they start next.

Both are built once per session, in the name of its owner
([COMPONENTS.md](COMPONENTS.md#what-a-component-may-reach)), into a `CommandExecutor` shared by the
session's channels - the counterpart of an agent's `ToolExecutor`, which likewise receives ready
providers and is no component. Two commands a line could not tell apart keep the session from
starting. What the executor finds its commands in is a `CommandSet`, generic over the kind of
command and holding the commands themselves: finding the command a line begins and refusing two that
collide are the same whoever runs the commands, and an agent's actor keeps its commands in one too.
What a command is given to run with - the session, or an agent's turn - and who asks whether it may
be seen and run are its kind's: the channel's commands the executor itself (`ShowCommand`,
`ExecuteCommand`), an agent's the channel's door as it admits the request (`ShowAgentCommand`,
`ExecuteAgentCommand`, issue 516). What a command takes is described by its parameters (issue 583,
`CommandParameter`): each with its name, whether it is required, the word that introduces it, if one
does - `as` of `/agent create <personality> [as <name>]` -, the last taking the rest of the line,
and what it takes (`ParameterKind`, issue 579) - text, a participant or an agent by its handle, an
agent of the session, a channel, a channel the user takes part in, a personality, a command, a
session or a template -, which a frontend may complete it from. `/help` and the web UI write them
from that (`CommandDescription::usage`). A command that names no parameters is not run with
arguments, and one with a required parameter not without any: whoever runs it - the executor once it
may be run, an agent's actor before it begins, the terminal - refuses it so (`check_arguments`), so
that no command checks it itself. What the arguments name - whom, which channel - the command reads
itself. How a command runs is the command's business, and there is nothing about running commands to
try out.

### Seeing and running

Seeing and running a command are separate permissions: `ShowCommand` decides whether it appears in
`/help` and in completion, `ExecuteCommand` whether it runs. Both carry the command and the
channel's attributes. The ownership policy grants them whoever reads the channel, and a command for
the owners alone the channel's owner.

A command the user may not see is answered exactly like an unknown one, so hidden commands cannot be
found by trying names, by their names and their short forms alike, nor listed in a group. Whatever
the command does is checked on its own as well, in the issuer's context - the one their views of
the session read in, too (`UserActor::security_context`): `/agent wakeup` asks for `WakeChannel`,
`/agent cancel` for `CancelChannel`, as cancelling over REST does.
`/participant nudge` asks nothing more: it asks someone to respond, as an `@`-mention does, which
asks nothing of the policy either. `/participant add` asks what taking someone in asks
(`procedure::participant::TakeIn::added`: `UpdateSession` unless they are in the session already,
`UpdateChannel`), `/participant remove` what reading the channel and taking someone out of it ask
(`LetGo::removed` with the channel: `UpdateChannel`), `/agent create` what making an agent asks
(`AgentManager::create`), and `/agent delete` what reading the session's agents and deleting one ask
(`AgentManager::delete`: `DeleteAgent`), so whoever reads the channel sees them, and those without
the right are refused on running them. `/participant list` asks what reading the channel asks, as
its view does, and `/agent list` what reading the session's agents asks (`SessionView::agents`): an
agent the issuer may not read is neither listed nor deleted by name. `/agent usage` asks what
reading the channel asks, as `/participant list` does - nothing of the agents themselves
(`ParticipantView::usage`). `/channel list`, `/channel describe` and `/channel notes` ask what
reading the session's channels asks - the notes nothing of the agents that wrote them
(`AgentNotesBoard`, of the notes' plugin) -, and `/channel rename`, `summary` and `briefing` what
changing the channel they are issued in asks (`UpdateChannel`).
`/channel create` asks what making a channel asks (`procedure::channel::Create`: `CreateChannel`,
`UpdateSession`), with the issuer alone in it, `/channel delete` what reading the channel named and
deleting it ask (`procedure::channel::Delete`: `DeleteChannel`, `UpdateSession`), and
`/channel join` what joining it asks, through the session (`TakeIn::joining`: `JoinChannel`).
`/channel leave` asks what leaving asks (`LetGo::leaving`: `LeaveChannel`), and with an agent
named what reading the channel asks besides. `/workspace run` asks what an agent's command asks in
the workspace, in the issuer's name (`WorkspaceController::shell_in`): reading the session and the
channel, the workspace usable there, `WriteWorkspace`; and its call what every message asks as the
channel admits it.

A command is issued in a thread of its channel (`CommandContext::thread`, issue 602), the one its
issuer names, the line where none is named: what it sends happens there. A `/participant nudge` asks
in it, of someone who takes part there; a `/workspace run` says its call there; a command of an
agent is asked of an agent that takes part there, and rejected at the channel's door otherwise; a
channel-wide answer is announced there. `/agent wakeup` without an agent wakes the thread it is
issued in and ends its quiet - in the line, the whole channel (issue 595), as stopping from the
line's view stops the whole channel. `/agent cancel` without an agent ends the turns of that
thread - in the line, of the whole channel - and keeps it quiet, as the stop buttons of the web UI
do.

### Private or channel-wide answers

A command answers the issuer alone (`/help`, `/participant list`, `/participant nudge`,
`/participant add`, `/participant remove`, `/agent list`, `/agent create`, `/agent delete`,
`/agent usage`, the group `channel`, `/workspace run`) or the whole channel (`/agent wakeup`,
`/agent cancel`): that `/participant add` brought someone in or `/participant remove` took someone
out, the channel tells everyone anyway. A channel-wide answer is announced by the issuer's actor as
`CommandExecuted`, forwarded to the users only, shown but never stored. The issuer gets the answer
back from `execute_command` either way, since the channel never routes an envelope back to its
sender; the REST response carries it only when it is private, since the web UI receives the
announcement on its stream. A command of an agent answers later, on the stream; an answer for the
issuer alone is routed to them alone.

### Naming an agent

Every command that names someone - taking part in the channel, or an agent of the session - names
them by one rule (`value::name::named`, `command::named_one`). The text is taken as a name first,
compared as names are; only if it is nobody's name is it taken as a handle, with or without its `@`,
normalised as a mention is. A handle is coarser than a name, so it can name several; a command then
refuses with their names rather than picking one. What it is sought among is the command's:
`/agent wakeup`, `/agent cancel` and `/participant nudge` act on an actor, and seek among those
running; `/participant remove` seeks among those `/participant list` lists, as the issuer reads the
channel, so that it also removes one that cannot run - but one that no longer exists, which has no
name to be named by; `/channel leave` among the agents of the issuer's own among them,
`/agent usage` among every agent of them; `/agent delete` seeks among the agents `/agent list`
lists, taking part or not. A channel is named by its whole name, which is the only one of its
session (`/channel describe`, `/channel notes`, `CommandContext::channel_view_or_named`); one the
issuer may not read is answered like one that does not exist. `/channel delete` deletes the channel
it is issued in as well, and `/channel leave` takes the issuer out of it: a command runs in the
issuer's call, not in the channel, so its answer still reaches them. `/channel join` alone does not
seek among the channels the issuer reads, since who may join a channel reads nothing of it before
they did ([SECURITY.md](SECURITY.md#access-lists)): it takes the running channel of that name, and
answers one the issuer may not join - by its access list, or not cleared for it -, or that does not
run, like one that does not exist.

Who is to be brought into the channel does not take part in it yet, so `/participant add` looks
further, by the whole name (`SessionView::candidates`, issue
[555](../issues/555-no-command-brings-an-agent-into-a-channel.md)): among the users and agents the
issuer may read, the nearest from the channel - the channel's scope, the session with its
participants wherever they lie, the issuer's own agents, the global scope with the users. One that
is shadowed by a nearer one of the same name is not reached by the command; the web UI's dialog
still lists it. Several equally near - a user and a global agent of one name - are refused with
their names. An agent of another user that does not take part in the session is not found, and a
name the issuer may not read is answered like one that does not exist. `/agent create` finds its
personality the same way from the channel (`Views::personality_named`), the innermost first. A
command that takes two names takes the last `as` as what separates them: `/agent create Reviewer as
Bob`.

### A command run in the workspace

`/workspace run <command>` runs a shell command in the session's workspace, through the shell of
the channel it is issued in (issue 587) - as an agent's `run_command` runs, so the command is the
channel's, bounded and ended as every command there ([WORKSPACE.md](WORKSPACE.md#commands)). It
follows the command until it ends, and sends it as a call the issuer made (`UserActor::send_call`):
their message without words, carrying the call of `run_command` - its presentation `shell`, its
description the tool's (`run_description`) - and its result, the end of the command's output as the
tool gives it, classified as the workspace. The
others hear it, the agents too, and nobody is asked to respond (see
[CHANNELS.md](CHANNELS.md#acting-in-a-channel)). It answers the issuer alone how the command ended.
Dropped before, it kills the command, and nothing is sent.

## Commands of an agent

Some commands act on an agent rather than on the channel: `/skill` puts a skill into its transcript,
`/compact` compresses it. An agent's state in a channel has one writer, its actor there, so such a
command is not run by the session but carried out by the agent itself - an agent run here and an
external agent alike.

*   **Contributed by components.** A component of the personality offers them as `AgentCommand`s
    (`CommandProvider<dyn AgentCommand>`), as a tool is offered, carried out in the agent's `Turn`.
    Which commands an agent carries out follows from its personality alone. Two that a line could
    not tell apart, or one that a line could not tell from a command of the session, keep the
    agent from starting.
*   **Always addressed.** A command of an agent names its agent - `/compact @bob`; `@all`, or
    `@channel`, as for a mention, names every agent that carries it out and takes part in the
    thread the command is issued in (issue 602), as a handle names only such an agent. Without a
    handle, or with one that names no such agent, it is refused with the handles to choose from.
*   **Seen and asked for.** `ShowAgentCommand` and `ExecuteAgentCommand` carry the command and the
    attributes of the agent and of the channel. Under the ownership policy they are granted to
    whoever may write in the channel, unless the command is for the agent's owners alone: then to
    whoever answers for the agent, as approving its calls (see
    [SECURITY.md](SECURITY.md#the-ownership-policy)). Who may not see a command finds it unknown.
*   **Sent, then carried out.** The issuer's actor asks the session's commands first, then the
    agents of the channel, each splitting the line by its own commands: an agent is asked if the
    line begins with one of its commands and the word after it names the agent or a group of them.
    The request names the command by its name. Which of an agent's commands the issuer sees, the
    channel answers from the agent's list (`ChannelLoop::commands_of`), so it answers at once while
    the agent is busy. Whether the issuer may ask for the command is decided at the channel's door,
    as it admits the request (issue 516): only from a user, to an agent taking part that carries the
    command out, and only if the issuer may see it and ask for it; what is rejected is answered to
    the issuer alone (`EventRejected`). The moderator routes an admitted request to that agent
    alone. A quiet channel delivers it as well; one read back from the history for the agent is
    admitted again, with the issuer's rights as they are then, and left out if it is not - the
    issuer is not told, having been answered when it entered.
*   **Where it came.** The agent takes in what it was told up to the command, carries it out, and
    takes in the rest, so that what was said before and after stands around it in its transcript as
    in the history. A command wakes nobody and is carried out asleep as well; the agent does not
    respond to it. One that comes during a turn waits for it.
*   **In a turn of its own** (see [AGENT-TURN.md](AGENT-TURN.md#a-command-turn)), in the request's
    contexts continued in the agent's name - its owner's rights, the issuer as initiator
    ([SECURITY.md](SECURITY.md#in-whose-name-an-agent-acts)). The agent asks nothing: what reached
    it was admitted. Carried out, the turn is taken over and stored; not carried out - the command
    failed, its arguments or what it acts on were refused, or the agent has none of that name -, it
    is dropped as if it had never begun, and the agent answers `CommandFailed` to the issuer.
*   **Begun.** The agent tells the issuer alone that it began, whatever its answer's visibility:
    for `/compact`, the issuer sees it begin and everybody sees it done.
*   **Answered.** Carried out, the agent answers `CommandExecuted` - to the issuer alone if the
    answer is private, to every user otherwise; not carried out, `CommandFailed` to the issuer
    alone, with why. Nothing withholds its answer as a tool's result is withheld: a command that
    answers what it read - `/skills`, `/private-notes`, `/memory` - reads through the readers of
    the agent's work, which give only what may be told in the channel.
*   **Recorded.** What was carried out - by an agent, with its arguments and on whose word, or by
    the session (`/agent wakeup`) - goes into the channel's history with what it answered, to whom
    it was answered: the issuer alone for a private answer, every user otherwise (issue 417). A
    command refused or failed did nothing; that it did not is recorded to the issuer alone
    (`CommandFailed`).
*   **Listed.** `/help` lists the agents' commands after the channel's, each with the handles of the
    agents that carry it out; the participant snapshots carry them for completion.

## Built-in commands

| Command | Short form | Of | Component | Arguments | Answer | What it does |
|---|---|---|---|---|---|---|
| `/help` | - | the channel | `help` | `[group or command]` | private | Lists the commands the user may use - the channel's and the agents', and in the terminal its own -, those of a group, or describes one. |
| `/participant list` | - | the channel | `participant` | - | private | Lists who takes part in the channel: their names and handles, whether users or agents, and an agent's personality. |
| `/participant add` | `/add` | the channel | `participant` | `<name>` | private | Brings the user or agent named into the channel, and into the session first if not in it yet; they see the whole history ([CHANNELS.md](CHANNELS.md#taking-someone-into-the-session)). |
| `/participant remove` | - | the channel | `participant` | `<participant>` | private | Removes the participant named from the channel; they stay in the session. |
| `/participant nudge` | `/nudge` | the channel | `participant` | `<participant>` | private | Asks the participant named to respond to the conversation as it stands, without a message ([CHANNELS.md](CHANNELS.md#nudging-someone)). |
| `/agent list` | - | the channel | `agent` | - | private | Lists the agents of the session the user may read - those taking part, and those lying in its scope -, with their personalities and whether they take part in the channel, the session or neither. |
| `/agent create` | - | the channel | `agent` | `<personality> [as <name>]` | private | Makes an agent of the personality named, in the session's scope - in the channel's for a personality of the channel's scope -, named after the personality unless named; `/participant add` brings it in. |
| `/agent delete` | - | the channel | `agent` | `<agent>` | private | Deletes the agent of the session named, among those `/agent list` lists; one taking part rests in its channels until it is restored. |
| `/agent wakeup` | `/wakeup` | the channel | `agent` | `[agent]` | channel-wide | Wakes the sleeping agents of the channel - of the thread it is issued in, if not the line -, or the one named. |
| `/agent cancel` | `/cancel` | the channel | `agent` | `[agent]` | channel-wide | Ends the turns of the agents of the channel - of the thread it is issued in, if not the line -, keeping it quiet, or the turn of the one named, and tells whose turn it ended; they stay in the channel, and an approval a turn waited for is withdrawn. |
| `/agent usage` | - | the channel | `agent` | `[agent]` | private | Tells how full the context of each agent taking part in the channel is, or of the one named - its tokens against its size, where known - and the input and output tokens of all its calls; without an agent, adds up the calls of them all. |
| `/channel list` | - | the channel | `channel` | - | private | Lists the channels of the session the user may read: whether it is this one, whether the user takes part, and its summary. |
| `/channel describe` | - | the channel | `channel` | `[channel]` | private | Describes this channel, or the one of the session named: its summary, briefing, classification and language. |
| `/channel notes` | - | the channel | `agent_notes` | `[channel]` | private | Shows what the agents noted in this channel, or in the one of the session named, by their authors - read as the channel's, as a notice board is ([MEMORY.md](MEMORY.md#notes)). Brought by the notes' plugin, in no default of the configuration: a session has it where its components name it - the seed's bundle `session-basics` - or the configuration's commands do, as the terminal's (issue 607). |
| `/channel create` | - | the channel | `channel` | `<name>` | private | Makes a channel of the session, with the user alone in it and an approver deciding by `Any`, as the web UI's dialog makes one. |
| `/channel delete` | - | the channel | `channel` | `<channel>` | private | Deletes the channel of the session named, also this one; it can be restored. |
| `/channel join` | - | the channel | `channel` | `<channel>` | private | Joins the channel of the session named, if the user may join it. |
| `/channel leave` | - | the channel | `channel` | `[agent]` | private | Leaves this channel, or takes the user's own agent named out of it; they stay in the session ([CHANNELS.md](CHANNELS.md#leaving-a-channel)). |
| `/channel rename` | - | the channel | `channel` | `<new name>` | private | Renames this channel. |
| `/channel summary` | - | the channel | `channel` | `[text]` | private | Says in a line what this channel is for, its briefing kept; without a text, removes it. |
| `/channel briefing` | - | the channel | `channel` | `[text]` | private | Says what an agent should know before it works in this channel, its summary kept; without a text, removes it. |
| `/workspace run` | `!` in the terminal | the channel | `shell` | `<command>` | private | Runs a shell command in the session's workspace until it ends, and puts it with its output into the channel as the user's call ([above](#a-command-run-in-the-workspace)). In no default but the terminal's. |
| `/skill` | - | the agent | `skills` | `@agent <name>` | private | Gives the agent a skill: its text goes into the agent's transcript in the issuer's name ([SKILLS.md](SKILLS.md)). |
| `/skills` | - | the agent | `skills` | `@agent` | private | Lists the skills the agent may load here. |
| `/private-notes` | - | the agent | `private_notes` | `@agent` | private | For the agent's owners: shows the private notes it recalls here - of this channel, and of the session's other channels as far as they may be told here ([MEMORY.md](MEMORY.md#private-notes)). |
| `/memory` | - | the agent | `agent_memory` | `@agent` | private | For the agent's owners: shows what it recalls of its memory here, as far as it may be told here ([MEMORY.md](MEMORY.md#agent-memory)). |
| `/compact` | - | the agent | `compaction` | `@agent` | channel-wide | For the agent's owners: compacts its context now rather than when it is too full, as the agent does on its own ([AGENT-TURN.md](AGENT-TURN.md#compaction)). |
| `/subagent` | - | the agent | `subagents` | `@agent <personality> <brief>` | private | For the agent's owners: has it start a sub-agent of a personality its component lists on the brief, as its tool does; it is given what it started to read, and what the sub-agent answers where the command was issued ([TOOLS.md](TOOLS.md#sub-agents)). |
| `/exit` | `/quit` | the terminal | - | - | - | Ends the program, the session shut down in order. |
| `/channel switch` | `/switch` | the terminal | - | `[channel]` | - | Talks in another channel of the session the user takes part in; without one, tells where the user could. |
| `/session list` | - | the terminal | - | - | - | Lists the sessions of the directory, the one spoken in last first, the one talked in marked. |
| `/session new` | `/new` | the terminal | - | `[template]` | - | Starts a session from the template named, or the program's, and talks in it; the one left is stopped. |
| `/session resume` | `/resume` | the terminal | - | `<id>` | - | Continues the session of the directory whose id begins so, and talks in it; the one left is stopped. |
| `/session delete` | - | the terminal | - | `<id>` | - | Deletes the session of the directory whose id begins so, not the one talked in; it can be restored. |
| `/agent retry` | `/retry` | the terminal | - | - | - | Asks for the answer under way, the turn given up, or the last answer of an agent in the thread followed again. |
| `/approval approve` | `/approve` | the terminal | - | `[id]` | - | Approves the call asked of the user, or the one waiting for approval whose id begins so ([APPROVALS.md](APPROVALS.md)). |
| `/approval deny` | `/deny` | the terminal | - | `[id]` | - | Denies the call asked of the user, or the one waiting for approval whose id begins so. |
| `/question answer` | `/answer` | the terminal | - | `[id]` | - | Asks the questions put to the user whose id begins so, or those put aside, one by one. |
| `/question decline` | `/decline` | the terminal | - | `[id]` | - | Declines the questions asked of the user, or those put to them whose id begins so. |
| `/thread list` | - | the terminal | - | - | - | Lists the threads open in the channel talked in, numbered: whose sub-agent's, who takes part, where it hangs - of one started by `/subagent`, no words, since it hangs at what its agent was given. |
| `/thread follow` | - | the terminal | - | `<thread>` | - | Follows a thread of the channel talked in, by its number in `/thread list` or the beginning of its id: what its view shows is printed, and what is typed goes there. |
| `/thread line` | - | the terminal | - | - | - | Follows the line of the channel talked in again. |
