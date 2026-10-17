# Tools

Agents act through tools. A tool is contributed by a component and run by the runtime's
`ToolExecutor`, which receives the ready providers and checks permissions. This document describes
what a tool is, how its calls are run and what they answer, and the tools that come with Caelumora.
Which calls need a person's approval, and how a tool asks a person, is in
[APPROVALS.md](APPROVALS.md); how components are built in [COMPONENTS.md](COMPONENTS.md); how far a
tool call may travel in [CONFIDENTIALITY.md](CONFIDENTIALITY.md).

A component offers tools by implementing `ToolProvider`; one component may expose several `Tool`s.
Besides running a call, a provider says what its tools are for, which of them it offers in a turn
and why it offers none (`offered`, in general terms, never with the error), what approvers are shown
for a call (`describe`), which calls are like a given one (`matcher`, for a rule derived from it),
and what a call asks a person before it runs (`questions`). A tool whose argument is a language says
better than the default what makes two calls alike.

A `Tool` declares its name, description and parameters, whether it requires approval, its
visibility, optionally the classification of its results, and its presentation: how frontends show
its calls, so that they need not recognise a tool by its name. The presentation is display only -
what a call does, and whether it waits for anyone, does not depend on it. Approval and
classification are decided per tool by its provider, not by the personality.

## Tool sets

The tools of one provider are a *tool set* (`ToolSet`), named after its component, with the
provider's description and, if it is unavailable, why. A model's backend is given the tools
natively, without the sets; the sets are kept in the record of what the model was offered and shown
in the web UI. Only a generic provider such as `mcp_tool` takes its description from its
configuration.

**What a turn is offered** depends on where it works (issue
[290](../issues/290-tools-offered-by-the-turn.md)). A provider has its tools (`tools`), by which a
call finds it and whose names no other provider of the agent may have, and offers of them those
that may be called in the turn's work (`offered`): by default all, a provider whose tools depend
on the place fewer, or none, saying why. It is asked once the turn is begun and before its context
is built, given the turn's work (`AgentWork`) - the place and whose authority it acts with -, as a
prompt part is; what it offers is fixed in the turn (see
[AGENT-TURN.md](AGENT-TURN.md#tools-offered)). Offering is a convenience for the model, never the
guard: a call of a tool not offered is answered as unknown, and a call of one offered keeps its own
checks, since what may be called can change while the turn runs. What the web UI shows of an
agent's tools is what a turn would be offered now, in the agent's own name.

The model needs no prompt to learn of its tools, but a source that is unavailable leaves no trace in
what it is offered. The optional component `tool_sets_prompt_provider` names the unavailable sets of
the turn, with why in general terms, and tells the model to say they are unavailable rather than
that it cannot do what they do. A personality without it tells its model nothing of missing tools.

A set may say where the arguments of its calls go (`ToolSet::destination`): where the channel may
not flow there, the set is offered as unavailable, and a call of a tool the turn was not offered is
answered as unknown (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md#what-leaves-through-tools)).

## Tool visibility

Every tool declares a `ToolVisibility`: whether the call is published to the channel, and whether
its arguments and result are, or only the agent's owner sees them.

*   `SHOWN`, the default: others see the call in full.
*   `WITHOUT_RESULT`: others see that the tool was called and with what, but not what it returned.
*   `CALL_ONLY`: others see that the tool was called, but not with what.
*   `HIDDEN`: the whole call stays with the agent.

How published messages are redacted, and what the model is told about results it must not quote, is
described in [CONFIDENTIALITY.md](CONFIDENTIALITY.md).

## Classified results

Every result is classified: as the tool classified it, else as the tool is configured, else as the
channel. A result that may not flow into the channel (`may_publish`, in the agent's name) never
reaches the model: it is replaced by the status `withheld` and a message that names the
classification and asks the model not to obtain it another way. The value is kept nowhere. Results
that did not come from the tool - refused, denied, timed out - are the runtime's fixed texts,
classified as the turn's label; a failed one, whose error may come from the tool, is the channel's.
Built-in tools whose answer is derived from the arguments alone classify it as the turn's label
too. See [CONFIDENTIALITY.md](CONFIDENTIALITY.md#tool-results).

## Running tool calls

An agent's tool calls are run by the runtime's `ToolExecutor`, built with the `ToolProvider`s of its
components. The executor is no component: how a call runs is its tool's business, but whether it
runs and what the model is told is never the choice of whoever may write a personality. Before it
hands a call to its tool, it checks that the caller is an agent - only agents call tools - that the
call was offered to it, and that the place's rules still let it run. Whether the policy permits the
call (`CallTool`) is asked once, as the call is admitted (see [Checked before anyone is
asked](APPROVALS.md#checked-before-anyone-is-asked)), not again as it runs. Afterwards the executor
times the call, turns an error into a status and a next step, classifies the result, withholds what
may not flow into the channel and marks what only the owner sees.

**Two tools of the same name** are a configuration error that keeps the agent from starting, naming
both components: resolved by order, one of them would be silently unreachable. User commands follow
the same rule.

**A call is not waited for longer than its turn lasts.** A cancel or a stop drops the tool's future
wherever it stands; the call is answered as `interrupted`, and the agent starts no further call of
the step. A tool that is done already keeps its result. Dropping a tool does not undo what it did
([20](../issues/20-transactional-effects.md)): the call counts as having run - a turn given up keeps
it with its result - and the model is told to check its effect before calling it again.

A user may also interrupt one call alone and let the turn go on (see
[CHANNELS.md](CHANNELS.md#cancelling)). A call interrupted while it runs is dropped as above and
answered as `interrupted`, saying who interrupted it and that the turn goes on; one interrupted
before it runs is answered as `not_executed`. The other calls of the step run. A shell command
handed back as running is no call any more; a user ends it as a command (see [The
workspace](#the-workspace)).

Dropping a future is all the stopping a tool gets: none of its code runs after the `await` it stood
at, and what the future holds is released. A tool therefore:

*   expects to be dropped at any `await`, and never leaves the turn half changed across one;
*   binds whatever it starts to its own future: a task it spawns is aborted on drop (a plain
    `JoinHandle` only detaches it), a child process is killed on drop, and work on a thread of its
    own cannot be interrupted at all - the one exception is `terminal_tool`, whose commands live
    with the storage;
*   does not block: code that never reaches an `await` cannot be interrupted, and holds up the
    agent's whole task, which even the abort after the stop's grace does not end.

What happened outside the process - a request the remote side already received - is not undone,
and whether the remote side stops depends on it.

### Results

Every call ends with a result the model can act on.

*   A successful call's result is the tool's output.
*   Every other outcome is `{"status": ..., "message": ...}`, whose message says what happened and
    what to do next: `denied`, `forbidden` (without the reason, which stays in the log),
    `timed_out`, `declined`, `invalid_arguments`, `failed`, `not_executed` (the tool never ran),
    `interrupted` (it ran, but was broken off) and `unknown_tool` (with the tools that do exist).
    The status is stored with the result, so that refusals and failures can be told from results.
    The wording lives in `crate::tool`.
*   No call is left without a result. A turn that ends while calls of its step have none answers
    them as `not_executed`, saying why the turn ended.

A call that ran records when it started and ended, around the tool's own run, so that a client can
show the duration.

A tool may give the agent something to read besides its result (`Turn::give`) - a skill it loaded,
say, with the knowledge it is. It is written after the results of the step, since nothing may stand
between the calls and their results (see
[AGENT-TURN.md](AGENT-TURN.md#beginning-and-ending-a-turn)).

## MCP servers

An `mcp_tool` component exposes the tools of a remote MCP server, reached over the Streamable HTTP
transport:

```json
{ "id": "github-mcp", "kind": "mcp_tool", "params": { "endpoint": { "url": "https://mcp.example.com/mcp", "auth": { "type": "bearer", "secret": "GITHUB_MCP_TOKEN" } } } }
```

The kind lives in the plugin `caelumora-mcp` (issue 325), with `rmcp`, so that the runtime does not
depend on it; `caelumora-app` and `caelumora-tui` list the plugin, and a personality names the kind
as before. What such a component kind may use of the runtime is said in
[PLUGINS.md](PLUGINS.md#what-a-component-kind-outside-the-core-may-use).

*   **Several servers.** Each server is an instance of its own; its tools are named
    `{component_id}__{remote_tool_name}`, so that servers cannot collide.
*   **Discovery.** The tools are discovered once, when the component is built. A server that is
    unreachable then does not fail the agent: its tool set is unavailable, with why in general
    terms, never the error, which may name hosts; a warning is logged.
*   **Description.** The parameter `description`, else what the server says of itself, else a
    generic text.
*   **Credentials.** `auth.secret` names a secret, resolved through the secret chain (see
    [SECURITY.md](SECURITY.md)); it is never a literal credential.
*   **Parameters.** `endpoint` (required: `url`, `auth` - optional, `bearer` -, `timeout_secs` -
    default 10; see [COMPONENTS.md](COMPONENTS.md#remote-endpoints)), `requires_approval` (`true`
    by default, `false` or `"unless_read_only"`; see
    [What needs approval](APPROVALS.md#what-needs-approval)), `classification`, `description` and
    `destination`.
*   **Calls.** Each call opens a connection, and so an MCP session, of its own. An interrupted call
    drops it, which ends the session; whether the remote tool stops is up to the server.

## Built-in tools

The tools that come with Caelumora are listed with the components that offer them, their parameters
and their approval defaults in [COMPONENTS.md](COMPONENTS.md#the-components); what the model is told
of each - its description and arguments - is the tool's own, in the code. How frontends show the
calls of a tool is said by its `ToolPresentation`: a name, defined as a constant next to the tools
shown so (`SHELL_PRESENTATION`, `SKILL_PRESENTATION`, …) with what it shows of their arguments and
results; a frontend shows those it knows, and every other as `default` (issue 323). Notes, private
notes and agent memory are described in [MEMORY.md](MEMORY.md), skills in [SKILLS.md](SKILLS.md).

`stop`, `bye` and `silence` end the turn with the current tool step: after `stop` and `bye` the
agent sleeps, after `silence` it stays idle without an answer. `create_plan` submits a multi-step
plan; a plan in progress is extended, not replaced. `sleep` waits at most 60 seconds, for testing a
cancel or a stop while a tool runs.

### The workspace

`terminal_tool` gives an agent its session's workspace, where its channel may use it
([CHANNELS.md](CHANNELS.md#its-workspace)): its files, and a shell where its storage gives one. What
its access list grants decides which of its tools may be called: the tools that only read files need
reading (`GetWorkspace`), the others and every shell tool changing (`WriteWorkspace`) - a command
may change anything its place reaches. A turn is offered those it may call: none where there is no
workspace it may use in its channel - the set is unavailable, saying so -, none that change where
the list grants reading alone, and no shell tool where its storage gives none; the prompt part says
the same ([AGENT-TURN.md](AGENT-TURN.md#its-workspace)). The workspace is reached through the turn's
views on every call all the same, before anything is sent: a call where there is none, or one the
list does not grant, fails and says so. Which calls need approval is configured as for `mcp_tool`:
all - the default -, none, or all but those that only read files.

**Files.** The file tools work over the workspace's `Terminal`, whatever its storage, without a
shell. A path is relative to the workspace's top; one that leaves the workspace is the model's to
correct, as is a file that is not there or not text. Reading is bounded, and says where to read on.
A replacement must find its text exactly once, unless every occurrence is to be replaced; it reads,
replaces and writes, so what changed the file in between is overwritten. Nothing is moved over,
which would be a deletion, and the workspace's top is neither moved nor moved over. What was read
is recorded as the turn's knowledge, a file's content by its digest (see
[MEMORY.md](MEMORY.md#recording-what-was-read)). The approver reads a change whole, and a rule
derived from an approved change covers the file, whatever is written into it - for a move, both
places.

`publish_file` offers a file for download at the message that calls it: a link to the file as it is
now, with its digest, downloaded through the channel's workspace by whoever may use it there. It
changes nothing and hands nothing to anyone who could not download the file already, so it reads,
and never needs approval, whatever is configured. What is to be kept as it was offered is published
with `publish_artifact`.

**Commands.** A command runs in the workspace's directory, every one anew - a `cd` does not carry
over - and is seen only from the channel it was started in. A call follows its command for
`wait_secs`; one still running then is handed back with its id, and the model follows it with
`command_output` or ends it with `kill_command`. The result is classified as the workspace. The
approver reads the command whole; following and ending a command need no approval, since only a
command that was approved is followed or ended.

A call dropped while it waits for its command - the turn cancelled or stopped - kills the command,
in a task of its own, bounded and logged, that the drop does not await. It is the deliberate
exception to "a tool binds whatever it starts to its own future": a command lives with the storage,
so that one handed back as running outlives the call, and one left running by a dropped call would
go on writing into the workspace after the turn that started it.

A command handed back as running is ended by the model, or by a user who may change the workspace's
files in the channel; the agent sees `killed` the next time it follows the command. How long a
command lives and when the runtime ends it is in [WORKSPACE.md](WORKSPACE.md#commands).

### Artifacts

`publish_artifact` (component `artifact_tool`, of the plugin `caelumora-artifacts`, issue 328)
publishes a file of the session's workspace as a version of an artifact: a fixed copy, kept in an
artifact storage beyond the workspace, downloaded at the message that published it. `publish_file`
offers a link to a file as it is now; a model is told the difference in their descriptions - a link
for a look now, an artifact for a result to be kept.

It is offered where a file can be published: where the turn may use a workspace in its channel and
an artifact storage is configured. The file is read from the workspace as the file tools read it and
handed to the `ArtifactManager`. It publishes in the name of whoever the agent acts for: its owner,
who makes the artifact (`CreateArtifact`) or adds a version where the artifact's list lets them
(`WriteArtifact`), asked before anything is kept. The members of the channel it is published in read
the version, and the artifact, which reading a version needs as well (issue 316). An artifact is
published into the session's scope under its name, made with its first version and given one more by
every later publication under that name. It is classified as the workspace, fixed with its first
version; a later version from a workspace classified otherwise is refused. A storage takes nothing
above its `destination`. Publishing changes what is kept, so it is approved where changes are; the
approver reads which file is published as which artifact.

The members of the channel download the version published there, wherever they act; whom the owner
shares the artifact with read the versions there are as they share it; its owner reads every version,
shares it and keeps it beyond the session. One that is not kept goes with the session, its content too (see
[STORAGE.md](STORAGE.md) and [SECURITY.md](SECURITY.md#ownership)).
