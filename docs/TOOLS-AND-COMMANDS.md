# Tools and Commands

Agents act through tools; people address the system through user commands. Both are contributed by
components and built the same way: a `…Provider` facet, and something that receives the ready
providers and checks permissions. This document describes tools, how their calls are run and
approved, user commands, and lists the built-in ones. How components are built is in
[COMPONENTS.md](COMPONENTS.md); how far a tool call may travel is in
[CONFIDENTIALITY.md](CONFIDENTIALITY.md).

## Tools

A component offers tools by implementing `ToolProvider`; one component may expose several `Tool`s.
The trait has these methods:

*   `description()` - what its tools are for, written for a model: the description of its tool set.
*   `tools()` - the tools the component offers.
*   `unavailable()` - why it gives no tools now, in general terms and never with the error; by
    default `None`, available.
*   `execute(name, params, turn)` - runs one of them.
*   `describe(name, params)` - the text approvers are shown for a call; by default the tool's
    description.

A `Tool` declares its `name`, `description` and `parameters`, whether it `requires_approval`, its
`visibility`, and optionally the `classification` of its results. Approval and classification are
decided per tool by its provider, not by the personality; `mcp_tool` takes them from its parameters
`requires_approval` and `classification` for every tool of its server.

`execute` answers with a `ToolOutput`: the tool's value and, if the tool classifies it - by its
input, by what it read, by the source it drew on - its classification. A tool that does not classify
returns a plain value (`Ok(value.into())`).

### Tool sets

The tools of one provider are a *tool set* (`ToolSet`, issue 194), named after its component -
unique within the agent - with the provider's description and, if it is unavailable, why. A model's
backend is given the tools natively, without the sets; the sets are kept in the record of what the
model was offered and shown in the web UI. The description is the provider's own: built-in
components word it for their tools, and only a generic provider such as `mcp_tool` takes one from
its configuration.

The model needs no prompt to learn of its tools, but a source that is unavailable leaves no trace in
what it is offered (issue 32). The optional component `tool_sets_prompt_provider` names the
unavailable sets of the tool sets the turn began with (`AgentWork::tool_sets`) - each with its
description and why, in general terms - and tells the model to say they are unavailable rather than
that it cannot do what they do. While every set is available it contributes nothing. A personality
without it tells its model nothing of missing tools.

### Tool visibility

Every tool declares a `ToolVisibility` (issues 54, 133): whether the call itself is published to the
channel, and whether its content - arguments and result - is, or only the agent's owner sees it.

*   `SHOWN`, the default: others see the call in full.
*   `CALL_ONLY`: others see that the tool was called, but not with what.
*   `HIDDEN`: the whole call stays with the agent.

How these flags are stamped on calls, how published messages are redacted, and what the model is
told about results it must not quote is described in [CONFIDENTIALITY.md](CONFIDENTIALITY.md).

### Classified results

How sensitive a result is, is its classification (issue 133). The `GuardedToolExecutor` gives every
result one: as the tool classified it, else as the tool is configured (`Tool::classification`, as
the turn began with its tools), else as the channel. A result that may not flow into the channel
(`ActionType::Publish`, in the agent's name) never reaches the model: it is replaced by the status
`withheld` and a message that names the classification and asks the model not to obtain it another
way. The value is not kept anywhere. Results that did not come from the tool - refused, denied,
timed out - are the runtime's fixed texts and classified as the turn's label; a failed one, whose
error may come from the tool, is the channel's. Built-in tools whose answer is derived from the
arguments alone classify it as the turn's label too (issue 168): the model sees every result, and
what it has seen classifies what the turn writes (see
[CONFIDENTIALITY.md](CONFIDENTIALITY.md#what-a-turn-writes-as)). See
[CONFIDENTIALITY.md](CONFIDENTIALITY.md#tool-results).

### Running tool calls

How an agent runs its tool calls is a component too (issue 127): the `ToolExecutor`. An agent has
exactly one - its personality's, the one `AppConfig::component_defaults` names, or else the default
`default_tool_executor` (see [COMPONENTS.md](COMPONENTS.md#facets-of-which-there-is-exactly-one)).
So another way of running tools - in parallel, with retries, cached - can be tried by configuration.

The executor decides *how* a call runs, never *whether*, and not what the model is told. It answers
with what the tool gave (`ToolOutput`) or a `ToolError` - `UnknownTool` if no tool has the name. The
agent holds it behind the runtime's `GuardedToolExecutor`, which checks before it delegates:

*   that the caller is an agent - only agents call tools;
*   that the policy permits the call (`ActionType::CallTool`);

and makes the result for the model afterwards: it times the call, turns an error into a status and a
next step (issue 15), classifies the result and withholds what may not flow into the channel (issue
133), and marks what only the owner sees (`successful_result`). The code that decides about
permissions and about what the model sees is thus never the choice of whoever may write a
personality.

A call is not waited for longer than its turn lasts (issue 161). The guard runs it next to the
turn's `CancellationToken`, which a cancel and a stop cancel; then the executor's future - and with
it the tool's - is dropped wherever it stands, the call is answered as `interrupted`, and the agent
starts no further call of the step. A tool that is done when the token is cancelled keeps its
result. Dropping a tool does not undo what it did ([20](../issues/20-transactional-effects.md)):
the call keeps when it started, so that it counts as having run - a turn given up tells the agent
of it - and the model is told to check its effect before calling it again.

Dropping a future ends it: none of the tool's code runs after the `await` it stood at, and what the
future holds - timers, sockets, open requests - is released. That is all the stopping a tool gets;
it does not see the token, and there is no way yet for one to clean up otherwise. A tool therefore:

*   expects to be dropped at any `await`, and never leaves the turn half changed across one;
*   binds whatever it starts to its own future, since that is all that is dropped: a task it spawns
    is aborted on drop (e.g. through `tokio_util::task::AbortOnDropHandle`; a plain `JoinHandle`
    only detaches it), a child process is started with `kill_on_drop(true)`, and work on a thread
    of its own (`spawn_blocking`, `std::thread`) cannot be interrupted at all;
*   does not block: code that never reaches an `await` - a long computation, `std::thread::sleep` -
    cannot be interrupted, and holds up the agent's whole task, which even the abort after
    `STOP_GRACE` does not end.

What happened outside the process - a request the remote side already received - is not undone,
and whether the remote side stops depends on it (for MCP servers see below).

The default executor has the `ToolProvider`s of the agent's components injected, with their
component ids. It runs a call on the provider that has the tool - no more - and gives the agent's
tools as one tool set per provider (`ToolExecutor::tool_sets`), asked of the providers each time.

### Two tools of the same name

Two components providing a tool of the same name are a configuration error that keeps the agent from
starting, naming both components. Resolved by order, one of them would be silently unreachable
(issue 37). User commands follow the same rule.

### MCP servers

An `mcp_tool` component exposes the tools of a remote MCP (Model Context Protocol) server, reached
over the Streamable HTTP transport:

```json
{ "id": "github-mcp", "kind": "mcp_tool", "params": { "url": "https://mcp.example.com/mcp", "auth": { "type": "bearer", "secret": "GITHUB_MCP_TOKEN" } } }
```

*   **Several servers.** Each server is an `mcp_tool` instance of its own. Its tools are named
    `"{component_id}__{remote_tool_name}"`, so that servers cannot collide.
*   **Discovery.** The tool list is discovered once, when the component is built, and cached. A
    server that is unreachable then does not fail the agent; it starts with no tools from that
    server, and a warning is logged with the error. Its tool set is unavailable, with why in
    general terms - "its server cannot be reached", never the error, which may name hosts - so
    that `tool_sets_prompt_provider` and the web UI can tell (issue 32). Discovering again is
    issue 126.
*   **Description.** The tool set is described by the parameter `description`, else by what the
    server says of itself in the `instructions` of its `initialize` result, else as "The tools of
    the MCP server '<component id>'" (issue 194).
*   **Credentials.** `auth.secret` names a secret, resolved through the secret chain (see
    [SECURITY.md](SECURITY.md)); it is never a literal credential.
*   **Parameters.** `url` (required), `auth` (optional; only `bearer` so far), `timeout_secs`
    (default 10, for discovery and for every call), `requires_approval` (default `false`, for
    every tool of the server), `classification` and `description`.
*   **Calls.** Each call opens a connection of its own. The remote structured content is the result
    if there is one, else the joined text; a remote error is a failed call.
*   **Interrupted calls.** An interrupted call (issue 161) drops its connection. rmcp then closes
    it in the background: it aborts the HTTP request under way and ends the MCP session
    (`DELETE`, if the server keeps sessions). No `notifications/cancelled` is sent - rmcp does so
    only for its own timeout - but as each call has its session of its own, the session ending says
    the same. Whether the remote tool stops is up to the server.

## Approvals

Some tool calls need a person's approval before they run. The process is asynchronous: the agent
suspends its turn while it waits, and nothing blocks.

1.  **Interruption.** When the model's answer contains tool calls that require approval, the
    `AgentActor` registers them as pending, runs whatever needs no approval, and moves to
    `WaitingForApproval`. Its state, the pending calls included, is stored after every step.
2.  **Request.** The agent emits `ApprovalRequested`, carrying the call's `ApprovalId`. The
    moderator routes it to the channel's `ApproverActor` only. The approver passes it on unchanged
    to the users (`UserActor`s), whose frontends show it.
3.  **Answers.** A user approves or denies over REST (`POST …/approvals`) or in the TUI.
    `UserActor::send_approval` sends `ApprovalGranted` or `ApprovalDenied` with the same id, which
    the moderator again routes to the approver only.
4.  **Decision.** The approver collects the answers and decides by its `ApprovalStrategy` (`Any`,
    `Majority`, `All`, `DenyOnFirst`). It decides again when users join or leave. A request nobody
    decides times out after ten minutes. The approver sends `ApprovalGranted`, `ApprovalDenied` or
    `ApprovalTimedOut` to the agent - the only way a decision reaches an agent (issue 81) - and to
    the users: an approval is over only once the approver says so, not when one user answered.
5.  **Resumption.** The agent runs the granted calls and continues its turn.

A turn that ends while it waits - cancelled, given up - withdraws its requests (issue 148): the
agent sends `ApprovalRequestWithdrawn` with the id, which the moderator routes to the approver only.
The approver takes it only from the agent that asked, forgets the request without deciding it, and
passes the withdrawal on to the users, so that nobody is asked any longer. A stopped
agent withdraws nothing - it has left - and the approver answers its pending requests as it leaves
(see [CHANNELS.md](CHANNELS.md)).

The agent's turn is the record of what it waits for (issue 172): the approver holds its requests
only in memory and starts empty after a restart. An agent that resumes a turn with calls still
pending - in `WaitingForApproval`, or stored right after asking - asks again, with the same ids
(`TurnState::pending_approvals`). The approver takes a request it does not know up as new, with a
new timeout; one it still holds it only announces again, keeping the answers given so far. Answers
given before a restart are lost with the approver's memory: the users are asked again.

Messages that arrive while the agent waits for approval are queued and processed in order once it
resumes, so its context stays consistent.

Approval requests are not redacted by confidentiality: the approvers see the arguments of a call
they are asked to decide.

### Checked before anyone is asked

Before a call is registered, the runtime's guard decides how it is admitted
(`GuardedToolExecutor::admit`, issue 155): answered at once, put to a user if its tool requires
approval, or run as it is. The actor only records what the guard decided.

*   **Policy.** A call the `SecurityManager` does not permit (`ActionType::CallTool`) is refused
    before any approval is requested, so users are not asked about calls that would be refused
    anyway. `execute` checks again.
*   **Validity.** A call that cannot run - arguments its tool's `describe` rejects, a tool that does
    not exist - is answered at once instead of being put up for approval (issue 118).

### Results

Every call ends with a result the model can act on (issue 15).

*   A successful call's result is the tool's output.
*   Every other outcome reaches the model as a JSON object `{"status": ..., "message": ...}`, whose
    message says what happened and what to do next. The statuses are `denied` (by a user),
    `forbidden` (by policy - without the reason, which stays in the log), `timed_out` (no approval
    in time), `invalid_arguments`, `failed` (with the tool's error), `not_executed` (the tool never
    ran: its turn ended first, or the server restarted before it ran), `interrupted` (the tool ran,
    but its turn was cancelled or stopped before it ended) and `unknown_tool` (with the tools that
    do exist).
*   The same status is stored as `ToolCallResult::status`, so the UI and an audit trail can tell
    refusals and failures from results.
*   The wording lives in `crate::tool` (`ToolError::to_result`, `denied_result`, `timed_out_result`,
    `forbidden_result`, `abandoned_result`, `not_executed_result`, `interrupted_result`).
*   No call is left without a result. A turn that ends while calls of its step have none - cancelled
    while waiting for an approval, given up - answers them as `not_executed`, saying why the turn
    ended, and keeps the tool message with the results of those that did run (`ToolCalls::abandon`,
    issue 148).

A call that ran also records when: `ToolCallResult::started_at` and `finished_at`, set by the
executor around the tool's own run (not the checks before it) - `finished_at` is when an
interrupted call was interrupted. They are stored with the history and
published with the call, so that a client can show the duration (issue 100).

## User commands

A line that starts with `/` addresses the system rather than the agents: `/help`, `/wakeup [agent]`
([28](../issues/28-user-commands.md)). A command is not a message: it never enters the history and
never reaches an agent's context.

Commands are for people; agents act through tools. A command is always run on behalf of a particular
person, so it is run by that person's actor, `UserActor::execute_command(name, args)`, as messages
and approvals are. REST reaches it the same way (`user_actor_of`, `403` for non-participants). Only
`UserActor` runs commands. What a command does to the channel still goes through the channel's own,
permission-checked operations (`ChannelLoop::wakeup`, `wakeup_actor`).

### Recognised by the frontend

The client decides whether a line is a command and sends it to `POST .../commands` (`{name, args}`)
instead of `POST .../messages`; the message API stays plain text. The rule is
`caelumora_model::command::parse_input`, mirrored in the web UI's `command.ts`:

*   a leading `/` starts a command, whose name runs to the first whitespace;
*   the rest of the line is handed to the command unparsed;
*   `//` sends a literal `/`.

Only this lexical rule is repeated per frontend. What a command means, who may run it and what it
answers is decided in one place.

### Commands are components

A `CommandProvider` (`caelumora_runtime::command`) contributes commands, as a `ToolProvider`
contributes tools, registered in the `ComponentRegistry`. They come from two places:

*   `AppConfig::commands`, the global set - by default `/help` and `/wakeup`; an empty list switches
    them off (see [CONFIGURATION.md](CONFIGURATION.md));
*   `SessionTemplate::commands`, for commands that belong to what a session is for. They are copied
    into the `Session`, so a running session keeps its set.

`RuntimeManager::new_command_executor` builds both once per session and hands the providers to a
`CommandExecutor` shared by the session's channels. It is the counterpart of an agent's
`ToolExecutor`, which likewise receives ready providers rather than a configuration. Two components
providing the same name are a configuration error that keeps the session from starting.

The two differ in one respect, deliberately (issue 127): the `CommandExecutor` is a runtime type,
not a component, and checks permissions itself. How a command runs is the command's own business,
and there is nothing about running commands to try out.

### Seeing and running

Seeing and running a command are separate permissions:

*   `ActionType::ShowCommand(name)` decides whether a command appears in `/help` and in completion
    (`GET .../commands`).
*   `ActionType::ExecuteCommand(name)` decides whether it runs.

A command the user may not see is answered exactly like an unknown one, so hidden commands cannot be
found by trying names. Whatever the command does is checked on its own as well: `/wakeup` asks for
`WakeChannel`.

### Private or channel-wide answers

A command answers the issuer alone (`/help`) or the whole channel (`/wakeup`).

*   A channel-wide answer is announced by the issuer's `UserActor` as `ActorEvent::CommandExecuted`.
    Every moderator forwards it to the users only; like `TurnFailed`, it is shown but never stored.
*   `UserActor::execute_command` returns the answer in both cases, because the channel never routes
    an envelope back to its sender.
*   The REST response carries the answer only when it is private, since the web UI receives the
    announcement on its stream.

## Built-in tools and commands

### Tools

| Tool | Component | Parameters | Approval | Visibility | What it does |
|---|---|---|---|---|---|
| `echo` | `echo_tool` | `message` | no | `SHOWN` | Returns its arguments unchanged. |
| `sleep` | `sleep_tool` | `seconds` (optional, at most 60) | as configured, no by default | `SHOWN` | Waits, then says so; for testing a cancel or a stop while a tool runs. |
| `calculate` | `calculator_tool` | `expression` | no | `SHOWN` | Evaluates a mathematical expression. |
| `stop` | `stop_tool` | - | yes | `SHOWN` | Ends the turn: the agent has finished its task and goes to sleep. |
| `bye` | `stop_tool` | - | yes | `SHOWN` | Ends the turn: the agent says good-bye and goes to sleep. |
| `silence` | `stop_tool` | - | yes | `SHOWN` | Ends the turn without an answer; the agent stays idle. |
| `create_plan` | `create_plan_tool` | `summary`, `steps` (each with `description`) | yes | `SHOWN` | Submits a multi-step plan; a plan in progress is extended, not replaced. |
| `{id}__{name}` | `mcp_tool` | as the server declares | per `requires_approval` | `SHOWN` | A tool of a remote MCP server. |
| `note` | `notes` | `text` | no | `SHOWN` | Notes something in the channel, visible to every participant. |
| `revoke_note` | `notes` | `id` | no | `SHOWN` | Revokes one of the agent's own notes. |
| `read_notes` | `notes` | `channel` (optional) | no | `SHOWN` | Reads the notes of another channel, or of every readable channel. |
| `reclassify_note` | `notes` | `id`, `level`, `compartments` (optional) | yes | `SHOWN` | Reclassifies one of the agent's notes, never above the channel (issue 133). |
| `note_private` | `private_notes` | `text` | no | `HIDDEN` | Notes something only the agent (and its owner) sees. |
| `revoke_private_note` | `private_notes` | `id` | no | `HIDDEN` | Revokes one of the agent's private notes. |
| `reclassify_private_note` | `private_notes` | `id`, `level`, `compartments` (optional) | yes | `HIDDEN` | Reclassifies one of the agent's private notes, never above the channel (issue 133). |
| `remember` | `agent_memory` | `text` | no | `HIDDEN` | Remembers something across channels and sessions. |
| `forget` | `agent_memory` | `id` | no | `HIDDEN` | Forgets something remembered. |
| `reclassify_memory` | `agent_memory` | `id`, `level`, `compartments` (optional) | yes | `HIDDEN` | Reclassifies something remembered that may be recalled here, never above the channel (issue 133). |
| `read_history` | `channel_history` | `channel`, `last` (optional, default 20, at most 100), `query` (optional) | no | `SHOWN` | Reads the latest messages of another channel, or those containing a text. |

`stop`, `bye` and `silence` end the turn with the current tool step (`Turn::request_state`). Notes,
private notes and agent memory are described in [MEMORY.md](MEMORY.md).

### Commands

| Command | Component | Arguments | Answer | What it does |
|---|---|---|---|---|
| `/help` | `help_command` | `[command]` | private | Lists the commands the user may use, or describes one. |
| `/wakeup` | `wakeup_command` | `[agent]` | channel-wide | Wakes the sleeping agents of the channel, or the one named (`@handle` or `handle`). |
