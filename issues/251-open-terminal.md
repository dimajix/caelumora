# 251 — Agents have no workspace to run commands and keep files in

**Status:** open
**Area:** Integrations

## The problem

An agent can talk, take notes, remember and call the tools of MCP servers, but it has no computer of
its own: nowhere to run a command, keep a file between turns, install a package or check what a
script it wrote does. Open Terminal (`open-webui/open-terminal`, MIT, "a computer you can curl";
version 0.14.0 when this was written) offers exactly that: a container - or a bare host - with a
shell, a file system and running processes, reached over HTTP.

Caelumora can reach it today only as an MCP server, by configuration alone:

```json
{ "id": "terminal", "kind": "mcp_tool", "params": { "endpoint": { "url": "http://open-terminal:8000/mcp", "auth": { "type": "bearer", "secret": "OPEN_TERMINAL_API_KEY" } } } }
```

That works, but it lacks what a shell needs:

- **No approval by kind of tool.** Its MCP server is `FastMCP.from_fastapi(app)`: every endpoint
  becomes a tool, without `readOnlyHint`, so `"unless_read_only"` (issue
  [246](246-mcp-tool-hints.md)) asks for every tool - reading a file as much as running a command -
  or, with `false`, for none.
- **No matcher of its own.** The approver sees the arguments as JSON, and "and from now on" offers
  the exact call; a shell command is the argument issue [247](247-tool-own-matcher.md) waits for.
- **Long commands are the model's work.** `run_command` returns after its `wait` with an id; the
  model polls `get_process_status` itself, one turn step per look.
- **An interrupted call leaves its command running** (issue
  [161](161-tools-cannot-be-interrupted.md)): dropping the MCP connection ends the request, not the
  process.
- **No channel, no agent.** The proxy sends one fixed header (the API key). Open Terminal's
  `X-Session-Id` (a working directory and a process list of its own) and `X-User-Id` (a Linux user
  of its own in multi-user mode) cannot be set, so all agents and channels share one directory and
  one user.

## Why it matters

A workspace is what turns an agent from one that describes what to do into one that does it and
checks the result: plan, act, observe, repair. It is also the most dangerous tool an agent can be
given - a shell with network access acts outside in every sense issue
[150](150-impersonation-towards-the-agent.md) worries about, and a file system that outlives the
turn is a store across channels that no confidentiality rule looks at.

## What Open Terminal offers

From its sources (`open_terminal/main.py`, `mcp_server.py`, v0.14.0):

- **REST API** (FastAPI, described at `/openapi.json`), authenticated by
  `Authorization: Bearer <api key>`:
  - `POST /execute?wait=&tail=` starts a command (`command`, optional `cwd`, `env`) in the
    background and returns its id, status, exit code and the output so far - all of it if it ended
    within `wait` (at most 300 s).
  - `GET /execute/{id}/status?offset=&wait=&tail=` returns new output since `offset`
    (`next_offset`), status and exit code; `POST /execute/{id}/input` writes to its stdin;
    `DELETE /execute/{id}` kills it; `GET /execute` lists the running ones.
  - `/files/list`, `read`, `write`, `replace`, `grep`, `glob`, `search`, `upload`, `move`,
    `delete`, `mkdir`, `view` (original or rendered preview); `/ports` (a proxy to servers started
    in the workspace), `/skills`, `/health`, `/info`.
- **Headers:** `X-Session-Id` keeps a working directory per session and scopes the process list;
  `X-User-Id` (multi-user mode only) runs everything as a Linux user of its own with its own home -
  "workspace separation, not a security boundary": one container, kernel, process list and network.
- **Interactive terminals:** `POST /api/terminals` and a WebSocket `/api/terminals/{id}` (PTY),
  authenticated by a first message `{"type": "auth", "token": ...}`; not in the OpenAPI schema,
  meant for a UI.
- **MCP:** `open-terminal mcp --transport stdio|streamable-http`, every endpoint of the schema as a
  tool, the API key as bearer token.
- **Isolation per user** is a separate project ("Terminals"), one container per user, which needs an
  Open WebUI Enterprise License in production.
- The API is 0.x and moves: releases every one to two weeks, endpoints renamed before
  (`/files/search` to `/files/grep`).

## Considerations

### Decision 1: the shape of the integration

- **MCP by configuration (today).** No code; the gaps above. Good enough to try Open Terminal out,
  with `requires_approval: true` and one container per agent.
- **A. A generic `openapi_tool`.** Read `/openapi.json` and offer every operation as a tool, as Open
  WebUI does (issue [252](252-openapi-tool.md)). Reusable for any OpenAPI server, but it has the
  gaps of the MCP route for the same reason - the tools carry no meaning: no approval by kind, no
  matcher, no polling, no kill on interruption. For Open Terminal it gains one thing over MCP -
  approval by HTTP method (see *Relation to issue 252* below) - and lacks the rest.
- **B. A component `open_terminal_tool` with chosen tools (recommended).** `run_command` (command,
  optional `cwd` and input), `command_output` (for a command that outlived its call),
  `kill_command`, `read_file`, `list_files`, `grep`, `glob`, `write_file`, `replace_in_file` - with
  descriptions written for a model, each tool saying whether it only reads, `describe` showing the
  command line, `matcher` proposing what makes two commands alike (247). Left out on purpose:
  `/ports`, `/skills`, `/files/view`, the PTY. Cost: a module `tool_impl/open_terminal/` - a client
  (`client.rs`: the API's types, `reqwest`, errors through `RemoteEndpoint` and
  `describe_request_error`, issue [188](188-remote-helpers-on-llm-error.md)) and the component with
  its `ToolProvider` (`mod.rs`) - an integration test against a stand-in server, and the documents.
  The client follows a 0.x API and has to be maintained. Whether the file tools belong to it at all
  is Decision 5: with Open Terminal as the executor over a workspace of our own, only the commands
  remain.
- **C. Extend `mcp_tool` instead.** Headers with placeholders (`{channel_id}`, `{agent_id}`) and
  approval per remote tool in the configuration. All MCP servers would gain from it, but polling,
  the kill and the matcher stay missing, and whether FastMCP's proxy passes headers on to the REST
  API is not known. Worth an issue of its own; it does not solve this one.

**Alignment with `mcp_tool` (for B).** The same `endpoint` (`RemoteEndpoint`, `bearer` secret), the
same `requires_approval` (`true` by default, `false`, `"unless_read_only"`), `classification` and
`description`; an unreachable server makes the tool set unavailable (issue
[32](32-unavailable-tools-invisible-to-model.md)) and does not stop the agent. For that the
`Approval` enum moves out of `mcp_tool.rs` to a shared place - a step of its own, without behaviour
change. `true` stays the default: commands usually have network access and act outside.

### Decision 2: the workspace and confidentiality

A workspace is a store the agent writes and reads across channels, and its files carry no
classification. The `GuardedToolExecutor` checks what comes back (issue
[133](133-classification-model.md)), nothing checks what goes out (issue
[180](180-outbound-flow-unchecked.md)): an agent writes a file in a `strictly_confidential` channel
and reads it in a public one. `X-Session-Id` separates working directories, not files, and is no
boundary.

A workspace of classification W may be used in a channel of label L only if both directions flow:
results into the channel (W ≤ L) and what the agent saw there into the workspace (L ≤ W) - in
practice L = W.

- **a. Only the configured `classification` of results,** as `mcp_tool` has. No cost; leaves the
  outbound gap open, which for a store that keeps everything is worse than for an MCP call.
- **b. Issue 180 first, generically (recommended).** A tool set whose remote side is not cleared for
  the channel is not offered there - decided where the tools are assembled for a turn, not per
  call, as 180 proposes. Open Terminal would be its first user, its clearance being W in both
  directions; `mcp_tool` would gain the same. An architecture decision of its own (per agent or per
  tool set; how a clearance is read without building the component).
- **c. A check in the provider for the time being:** `execute` refuses unless `turn.label()` = W.
  Quick, but the check sits on the wrong owner and would be removed again with 180.

Until 180 is done, the provider would be configured only for channels of one classification, and
the documents would say so. In operation: one Open Terminal per classification level, or per
channel where that is too coarse.

### Decision 3: separating agents and channels

- **One workspace per component** - shared by every agent built with it.
- **`X-Session-Id` = the channel's id** - a working directory and process list per channel; a
  convenience, no boundary. Cheap enough to set always.
- **`X-User-Id` = the agent's id** - a home of its own per agent in multi-user mode; by Open
  Terminal's own word no security boundary either.
- **A container per agent** - real isolation, through "Terminals" (enterprise license) or
  provisioning of our own; out of scope here.

### Decision 4: long commands

- **i. Waiting within one call (recommended).** `run_command` waits up to a configured
  `max_wait_secs` (say 60; the API allows 300), following the output with `offset`, and returns it
  bounded (`tail`, a maximum size, `truncated`). A command still running returns its id,
  `status: running` and a pointer to `command_output`. An interrupted call kills its command: a
  drop guard sends `DELETE /execute/{id}` in a short, bounded task of its own - a deliberate
  exception to "everything is bound to the tool's own future", to be documented in
  TOOLS-AND-COMMANDS.md.
- **ii. Everything in the background,** the model polling. Costs steps and tokens, and gains only
  for very long jobs - which `command_output` under i already allows.

### Decision 5: Open Terminal as the workspace, or as the executor over ours

[PLAN.md](PLAN.md) (block B1) asks for a workspace of Caelumora's own - W1, an issue still to be
opened: a tree of files with `SecurityAttributes` and one classification, kept as a plain
directory, with tools for agents (W2), a side for the users (W3: upload, download, a panel, diffs)
and versions (W4). Open Terminal brings a workspace of its own. Two of them would be the repeated
logic AGENTS.md warns of, so the question is which one it is:

- **Open Terminal's container is the workspace** - B as described, file tools included. Fast, and
  it covers code that is to be built and run. But the files belong to nobody in Caelumora: no
  `SecurityAttributes`, no owner, no versions, no download in the web UI, one classification per
  container by configuration; and the artifacts of issue [208](208-generated-artifacts.md) would
  need a store of their own anyway.
- **Open Terminal is the executor over our workspace (recommended).** An Open Terminal container
  mounts the workspace's directory and runs commands on it. Reading and writing files is W2's
  alone, and this component offers only the commands: `run_command`, `command_output`,
  `kill_command`. Otherwise two components offer the same file tools, with two approval rules and
  two records of what was read and written. The workspace's classification is the one of Decision
  2, now a property of the workspace instead of the component's configuration. The costs:
  - What a command changes passes W2's record and versions by. W4 needs a snapshot around every
    command (git in the directory, say), not only around a write; what a command read is not
    recorded as knowledge at all.
  - A container per workspace, mounting its directory, is something to operate - and to start
    when a workspace is attached, if workspaces are created at run time.
  - Open Terminal's own file endpoints stay reachable with the API key; the component simply does
    not offer them, which is no boundary but the same trust the shell already needs.
- **First the one, then the other.** B with its file tools can come first, as the quick way to try
  agents on files, if its file tools are understood as interim and removed once W2 exists. The
  configuration of agents that used them changes then.

### Relation to issue 252

The two issues share one preparatory step and are otherwise independent.

- **Shared:** the `Approval` enum, which both need and which lives in `mcp_tool.rs` today. Moving it
  to a shared place is a restructuring without behaviour change and comes first, whichever of the
  two is done first. `RemoteEndpoint` with a bearer secret exists already. The conversion of JSON
  Schema into `ToolParameters` only 252 needs: B writes its parameters by hand.
- **B is not built on 252.** It could be - `open_terminal_tool` as a thin layer of chosen
  operations over `openapi_tool` - but B's worth is the meaning of its tools (waiting and following,
  the kill, `describe`, `matcher`), while the transport to a fixed, known API of a few endpoints is
  a small typed client. Through 252, the shape of these endpoints would be read from the document at
  run time instead of from Rust types: more work and more fragile, above all for a 0.x API. The hard
  parts of 252 - reading the document, `$ref`, 3.0 against 3.1, mapping parameters - B does not
  need.
- **252 can bridge to B.** With `openapi_tool` and `"unless_read_only"` trusting the HTTP method,
  Open Terminal almost divides itself: the `GET` endpoints read (`/files/read`, `list`, `grep`,
  `glob`, `/execute/{id}/status`) and need no approval; `POST` and `DELETE` (`/execute`,
  `/files/write`, `replace`, `move`, `delete`, the kill) do. That closes the largest gap of the MCP
  route. The proxy `/ports/...` must be filtered out: it passes a `GET` on to any service running in
  the container. Still missing would be `describe` with the command, the shell matcher, waiting with
  the kill and the headers per channel. B would replace such a configuration, not use it.

### Order, if B is chosen

1. Restructure without behaviour change: `Approval` out of `mcp_tool` to a shared place.
2. Decide issue 180 - not necessarily implement it: which channels a workspace may be offered in at
   all. The discussion can run alongside the next step.
3. `open_terminal_tool` with `describe`; the default (exact) matcher; configured only for channels
   of one classification.
4. Issue 247: a shell matcher that refuses chains and substitutions, Open Terminal its first user.
5. Issue 180 implemented: the clearance where tools are offered, Open Terminal and `mcp_tool` its
   users; from then on channels of mixed classification.
6. Later: files of the workspace as artifacts through `/files/view` (issue
   [208](208-generated-artifacts.md)); option C as an issue of its own; issue
   [252](252-openapi-tool.md) once a second, concrete OpenAPI service is to be reached.

If reaching many services quickly matters more than Open Terminal itself, 252 comes first instead,
with Open Terminal as its first test case and bridge (see above), and B follows as the refinement.
The price is that the harder and less certain work comes before the concrete need.

## Open questions

1. The shape: B, or A (252) or C?
2. Confidentiality: 180 first or alongside (b), or c for the time being?
3. Separation: `X-User-Id` = the agent's id, or one shared workspace per component for now?
4. Open Terminal as the workspace, or as the executor over ours (Decision 5)? And if the executor:
   wait for W1, or B with file tools first as an interim?

## Done when

- An agent can run commands in an Open Terminal workspace - and read and write files there, unless
  Decision 5 leaves that to a workspace of our own - with approval by kind of tool, the command
  shown to the approver, long commands followed within a call, and an interrupted call killing its
  command.
- A workspace is not offered in a channel whose classification it does not match, and the agent is
  told so.
- The integration is documented in TOOLS-AND-COMMANDS.md and COMPONENTS.md, its limits in
  CONFIDENTIALITY.md.

## History

- **2026-10-07** — Opened after researching how Open Terminal is reached (REST API with OpenAPI,
  optionally MCP through FastMCP; no protocol of its own), with the options laid out above. No
  decision yet.
- **2026-10-07** — Added how this issue relates to [252](252-openapi-tool.md): the shared step
  (`Approval`), why B is not built on 252, and 252 as a possible bridge for Open Terminal, with the
  `/ports` proxy filtered out. Option A was said to gain nothing over MCP; it gains approval by HTTP
  method, and the text says so now. The order now decides 180 before B is built.
- **2026-10-07** — Added Decision 5, from the update of PLAN.md: Open Terminal as the workspace, or
  as the executor over a workspace of Caelumora's own (W1), its tools then the commands alone. The
  open questions and *Done when* say so.
