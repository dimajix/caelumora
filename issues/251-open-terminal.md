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

## Decisions

Taken on 2026-10-08 by the maintainer, answering the questions above:

1. **The shape:** B, a component of its own, `open_terminal_tool`.
2. **Confidentiality:** issue [180](180-outbound-flow-unchecked.md) is part of this work - the check
   at the door, generically, not a stand-in in the provider.
3. **Separation:** one container per component, shared by the workspaces it is given;
   `X-Session-Id` is the channel's id, so that each channel has a process list of its own; no
   `X-User-Id`.
4. **Decision 5:** Open Terminal is the executor over a workspace of Caelumora's own, which belongs
   to a user and is attached to sessions (PLAN.md's open question 3, the larger of its two
   answers). This component offers the commands alone; the workspace is issue
   [253](253-workspace.md).
5. **Not bound to Open Terminal:** a workspace is a directory Caelumora owns. Open Terminal is the
   first way into it; tools of Caelumora's own may be a second, in an issue of their own (W2). So
   the files lie where Caelumora can reach them, and a container is given them - mounted, in this
   plan; started by Caelumora for one workspace, in the step after it (plan, part A).

## Plan

The plan has two subjects with one owner each: the workspace (issue [253](253-workspace.md); parts
B, E, F and G below) and the executor over it (this issue; parts C and D). They are planned together
because the second cannot be built without the first, and implemented in the steps of part H, each
reviewable on its own. Names of types and functions are proposals for the implementation to keep
or to better; what is fixed is where a responsibility lies.

### A. Where Open Terminal is integrated

Caelumora owns the files. A workspace is a directory `<root>/<workspace id>/` under the configured
`workspaces.root`, on Caelumora's host or in a volume it has. Everything that works on a workspace
works on that directory: Caelumora itself - uploads and downloads now, tools of its own later - and
an Open Terminal container that has it mounted. Open Terminal is reached as it is today, over its
REST API, with one difference: every command is given the workspace's directory as its `cwd`.

How the container gets the directory decides how far the isolation goes:

- **A1. A shared container, started outside Caelumora** - this plan. `docker compose` starts one
  Open Terminal next to Caelumora and mounts `workspaces.root` at a path the component is told
  (`mount`); the component maps `<root>/<id>` to `<mount>/<id>`. Caelumora starts and stops
  nothing. The container sees every workspace under the root, whatever its classification, and
  nothing in it keeps a command in one workspace from reading another: it is no boundary. That
  fits the plan's target - one server, a trusted team - and only where every workspace the
  container can see may be seen by everyone who can run a command in it; in practice, one
  classification for the root a container mounts. The door (part D) keeps a container classified
  `internal` out of a `confidential` channel, but not a `confidential` workspace out of the
  `internal` container's mount. With several classifications, one container per classification is
  needed, each mounting a root of its own; `workspaces.root` is one directory in this plan, and a
  root per classification is the extension if that is wanted before A2.
- **A2. A container per workspace, started by Caelumora** - the step after, an issue of its own.
  Caelumora talks to the Docker daemon (crate `bollard`), starts an Open Terminal container for a
  workspace when it is first used, mounting that directory alone, with an API key generated for
  it, stops it when idle and removes it when the workspace is purged. Real isolation between
  workspaces, and the only way the workspaces one component serves can differ in classification.
  Its costs: access to the Docker socket is root on the host, so Caelumora's process gains that;
  the lifecycle - pull, start, health, idle stop, removal, what a restart of Caelumora does - is to
  be operated; the endpoint and the key are dynamic.

The component is cut so that A2 changes only where it finds the terminal of a workspace (C6): A1 is
a terminal configured once, A2 one found - or started - per workspace. Nothing else in the
component, nothing in the workspace and nothing in the UI knows the difference.

Weighed and not taken: Open Terminal's multi-user mode (`X-User-Id`) separates homes, not
workspaces, and is no boundary by its own word; the "Terminals" orchestrator needs an enterprise
license; a container per *agent* separates the wrong thing - a workspace is shared by the agents
and the people of a session.

### B. The workspace (issue 253)

#### B1. The model (`caelumora-model`)

`workspace.rs`, a stored kind like the others - the steps every kind takes: `define_id!`,
`Secured`, `Deletable` through `impl_deletable!`, `Scoped`, `EntityKind::Workspace`, the five
`ActionType`s, an `Actions` const in `store_impl/access.rs`, the nine methods on `EntityStore`,
both store implementations, and `lifecycle!`, `found_by_name!`, `unique_by_name!` in the store
suite:

```rust
define_id!(WorkspaceId);

pub struct Workspace {
    pub id: WorkspaceId,
    #[serde(flatten)]
    pub name: ScopedName,                  // in its owner's UserScope: unique per user
    pub description: Option<Description>,  // what it holds, as its owner describes it
    pub classification: Classification,    // one for the whole tree (issue 169)
    pub security_attributes: SecurityAttributes, // owner: the user; contributors: who may use it
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted: Option<Removal>,
}

pub enum WorkspaceUpdateEvent {
    NameChanged(String),
    DescriptionChanged(Option<String>),
    ClassificationChanged(Classification),
    ContributorsChanged(Vec<ActorId>),
}
```

What it does *not* hold: where its files are - derived, the root is configuration and the directory
is the id; no path is stored or shown, as no path goes into a message - which sessions it is
attached to, which is the session's business, and its files: the directory is the record.

`Session` gains `workspaces: Vec<WorkspaceId>` (`#[serde(default)]`, as `channels`) and
`SessionUpdateEvent::WorkspacesChanged(Vec<WorkspaceId>)`. `SessionTemplate` gains nothing: a
template is reusable, a workspace is someone's, and attaching is an act on the session.
`GlobalEvent` gains `WorkspaceCreated(WorkspaceId)`, `WorkspaceUpdated(WorkspaceId,
WorkspaceUpdateEvent)`, `WorkspaceDeleted(WorkspaceId)` and `WorkspaceRestored(WorkspaceId)`;
`EventBus::publish` routes them to `All` - a topic of their own comes when something subscribes to
one workspace - and `PublishedEvent::read_action` answers `GetWorkspace`.

The classification is a `Classification`, not a level: a workspace may have an audience.

Stored data: nothing to delete; every new field has a default.

#### B2. Permissions and flow

Two questions, kept apart as everywhere (SECURITY.md):

**Who may** - the five actions under the ownership policy, as for every kind: `CreateWorkspace`
(with oneself as owner), `GetWorkspace` (the owner and the contributors), `UpdateWorkspace` and
`DeleteWorkspace` (the owner), `ListWorkspaces` (filtered by `Get`), and the trash (`ReadDeleted`,
`Restore`, `Purge` with `EntityKind::Workspace`). *Using* a workspace - reading and writing its
files, attaching it to a session - is what contributors may, as writing into a session is: it asks
`GetWorkspace`. Changing the document - name, description, contributors, classification - is
`UpdateWorkspace`, the owner's.

**What may flow** - the workspace is one container classified W, and its files carry no label of
their own:

- *A person* reads and writes files only if cleared for W, as a member of a channel is:
  `JoinChannel { participant, roles, channel: &W }`, the clearance question with the workspace in
  the channel's place - asked when a contributor is added, when the classification is raised
  (refused above someone in it, as for a channel), and as `may_see_channel` by the panel. Uploading
  needs no further rule: what a cleared person puts in is theirs to classify as W.
- *An agent* reads and writes through a channel classified L, and both directions must flow: W into
  L, since what a command prints reaches the channel, and L into W, since what the model puts into a
  command may be anything it saw there. `W.flows_to(L) && L.flows_to(W)`, asked as the two `Publish`
  questions the policy already answers - `may_publish(ctx, agent, &W, &L)` and `(…, &L, &W)`. With
  compartments and a read relation that is not symmetric this is stricter than equality, which is
  right. It is asked per call by the operation (B3), and the result of a call is classified W
  (`ToolOutput::classified`), so that the `GuardedToolExecutor` withholds it where it does not
  flow, as for any tool.
- *Changing the classification* is the owner's act (`UpdateWorkspace`, then `Classify(new)`):
  lowering releases the whole tree, raising re-asks the clearance of every contributor. Nothing is
  fitted; there are no labels inside. Who did it is the audit log's (140).

These are existing questions with a workspace in the place of a channel. That `Publish`,
`JoinChannel` and `may_see_channel` name a channel where they mean a place is worth a rename once
180 and 169 settle what a place is; it is not done here. New `ActionType` variants: the five of the
kind, nothing else.

#### B3. The runtime: `WorkspaceManager` (`caelumora-runtime/src/workspace.rs`)

One owner of "a workspace and its directory", held by the `RuntimeManager` - made with the entity
store, the event bus, the security manager and the `workspaces` configuration - the one writer of
workspace documents (under a lock per id: store first, then announce) and the only code that
touches the directory:

- `create(ctx, name, description, classification) -> Workspace`: `Classify(new)`, the store (which
  asks `CreateWorkspace`), `mkdir`; a directory that cannot be made is a creation that fails, and
  the document is purged again.
- `rename`, `describe`, `set_classification` (`UpdateWorkspace`, `Classify`; raising re-asks
  `JoinChannel` per contributor), `set_contributors` (`UpdateWorkspace`, `JoinChannel` per
  newcomer).
- `delete` (marks; the directory stays), `restore`, `purge` (removes the directory, then the
  document; a directory that cannot be removed leaves the document, so that the next purge tries
  again), `get`, `list`, `deleted`.
- Files, each asking `GetWorkspace` and the clearance first: `list_files(ctx, id, path) ->
  Vec<FileEntry { name, is_dir, size, modified }>`, `read_file(ctx, id, path) -> (size, stream)`,
  `write_file(ctx, id, path, stream)` (bounded by `max_file_bytes`, written under a temporary name
  and renamed), `delete_file`, `mkdir`.
- `pub(crate) fn require_usable(ctx, id, place: Option<&Place>)`: `GetWorkspace`, then for a place
  the two flow questions of B2, for a person the clearance - the one check every use of a workspace
  makes, the executor's and W2's tools' alike, through `Enforcement::require`, before anything
  else.
- `pub(crate) fn directory(id) -> PathBuf` for what runs on the directory, after `require_usable`.

Paths: every path is relative to the workspace and normalised - `Path::components` with `Normal`
components only; `..`, a root or a prefix is refused - and opened with the directory as the base;
a symbolic link is followed only if its target stays inside (`canonicalize` and a prefix check). A
name is a UTF-8 `Normal` component; what a command creates under another name is listed lossily
and cannot be opened by the API. A path that does not fit is `WorkspaceError::InvalidPath`, which
names the path the user wrote, never the root.

`scoped.rs`: workspaces lie in a `UserScope`, so a user's deletion takes them along (`lying_in`
with `Workspace`); sessions and channels hold none.

Attaching is the session's: `SessionLoop::attach_workspace(ctx, id)` and `detach_workspace` ask
`UpdateSession` first, then `GetWorkspace` - the one who attaches must be able to use it - and
change the session through its controller (`SessionUpdateEvent::WorkspacesChanged`). No flow
question is asked at attaching: which channels may use it is decided per channel and turn (B2). A
workspace deleted while attached stays in the list and is left out wherever the list is read, with
a warning, as a participant whose record is gone.

#### B4. The views

Reading goes through the views, for people and agents alike (SECURITY.md, *Views*):

- `Views::workspaces() -> Vec<WorkspaceView>` (the viewer's: `ListWorkspaces` filtered by `Get`),
  `Views::workspace(id)`, `Views::deleted_workspaces()`.
- `SessionView::workspaces() -> Vec<WorkspaceView>`: the attached ones the viewer may read, absent
  ones left out.
- `WorkspaceView`: `id`, `name`, `description`, `classification`, `owner`, `may_change`
  (`may_change_workspace`), `may_use_here() -> bool` - for a viewer at a place, an agent in its
  channel, the two flow questions; for a user, the clearance - `require_usable() -> Result<(),
  WorkspaceError>` (the manager's check, as `MemberView::context()` asks `ReadAgentContext`),
  `files(path)`, `read(path)`, and `snapshot() -> WorkspaceSnapshot { id, name, description,
  classification, owner, can_change, can_use }`.
- `Views` is given the `WorkspaceManager` - it holds `entities`, `states`, `components` and
  `running` today - and `WorkspaceView` reads through it with the viewer's context; nothing of the
  directory is reachable past the view. Writes do not go through a view: a route calls the manager,
  which checks.

The `Turn` needs nothing new: a tool reaches the session's workspaces by
`turn.session()?.workspaces()`, as `channel_history` reaches the channels.

#### B5. REST (`caelumora-rest-server`)

`routes/workspaces.rs`, handlers as the others: `Result<_, ApiError>`, `Subject::Workspace`, DTOs in
`models.rs`, errors through `ApiError::from_runtime`.

| Route | Does | Asks |
|---|---|---|
| `GET /workspaces` | the user's workspaces, own and contributed, each with `can_change` and `can_use` | `ListWorkspaces`, `GetWorkspace` |
| `POST /workspaces` `{name, description?, classification?}` | creates; `classification` defaults to the channel default | `CreateWorkspace`, `Classify` |
| `GET /workspaces/{id}` | the snapshot | `GetWorkspace` |
| `PATCH /workspaces/{id}` `{name?, description?}` | renames, describes | `UpdateWorkspace` |
| `PUT /workspaces/{id}/classification` | reclassifies | `UpdateWorkspace`, `Classify`, `JoinChannel` per contributor |
| `PUT /workspaces/{id}/contributors` `[user ids]` | sets who may use it | `UpdateWorkspace`, `JoinChannel` per newcomer |
| `DELETE /workspaces/{id}`, `POST …/restore`, `POST …/purge` | the trash, as for agents | `DeleteWorkspace`, `ReadDeleted`, `Restore`, `Purge` |
| `GET /workspaces/{id}/files?path=` | lists a directory | `GetWorkspace`, the clearance |
| `GET /workspaces/{id}/files/{*path}` | downloads one file, streamed, as `Content-Disposition: attachment`, its media type guessed from its name (`mime_guess`), never inline - 208's rule: an HTML file would run in the web UI's origin | the same |
| `PUT /workspaces/{id}/files/{*path}` | uploads one file: the request body is the file, bounded by a `DefaultBodyLimit` from `max_file_bytes`; parent directories are made | the same |
| `DELETE /workspaces/{id}/files/{*path}` | removes a file or an empty directory | the same |
| `GET /sessions/{id}/workspaces` | the attached ones as the viewer sees them | `GetSession`, `GetWorkspace` each |
| `POST /sessions/{id}/workspaces` `{workspace_id}`, `DELETE …/workspaces/{wid}` | attaches, detaches | `UpdateSession`, `GetWorkspace` |

No multipart: one file per request is what the panel needs, and the body is the file. These are the
first routes that serve bytes - `Body::from_stream` for a download, the request's data stream for an
upload. Error codes: `workspace_not_found` (also for one not readable), `workspace_forbidden`,
`invalid_path` (400), `file_too_large` (413), `not_cleared` (403).

On the global stream: the four `Workspace…` events and `SessionUpdated(…, WorkspacesChanged)`.
Nothing announces a changed file - commands change files outside Caelumora's knowledge - so the
panel reloads when it is opened and after a tool call of the channel finished.

#### B6. The seed

No workspace in the seed: workspaces are someone's, and the seed is the system's. A personality in
`examples/seed.yaml` gets the component (C); a template nothing.

### C. The component `open_terminal_tool`

#### C1. Configuration

```json
{ "id": "terminal", "kind": "open_terminal_tool", "params": {
    "terminal": {
      "endpoint": { "url": "http://open-terminal:8000", "auth": { "type": "bearer", "secret": "OPEN_TERMINAL_API_KEY" }, "timeout_secs": 10 },
      "mount": "/workspaces"
    },
    "max_wait_secs": 60,
    "max_output_bytes": 65536,
    "requires_approval": true,
    "destination": { "level": "internal" },
    "description": "A shell in the session's workspace." } }
```

- `terminal`: where the container is and where it has the workspace root (A1). `endpoint` is a
  `RemoteEndpoint` as every remote component writes it; its timeout bounds every request, not a
  command's run.
- `max_wait_secs` (default 60, at most 300 - Open Terminal's own bound): how long one call follows
  a command before it hands it back as running.
- `max_output_bytes` (default 64 KiB): what of the output a result may hold; the rest is cut from
  the front, and the result says so.
- `requires_approval` (`true` by default, or `false`): for `run_command`. `command_output` and
  `kill_command` concern only a command the agent started and was approved for, and never ask; so
  there is no `"unless_read_only"` here - nothing else of this component reads - and the field is
  a `bool`, a deliberate difference to `mcp_tool`.
- `destination` (optional): the classification of the container itself, what may be sent to it,
  for the door (D). Without it, the container is cleared for public only, as an external agent that
  names no roles.
- `description`: of the tool set (issue 194); a text is built in.

Unknown fields are refused (`deny_unknown_fields`).

#### C2. The tools

| Tool | Parameters | Approval | Does |
|---|---|---|---|
| `run_command` | `command` (string); `workspace` (string, optional: its name - needed when the session has several usable here); `wait_secs` (optional, at most `max_wait_secs`) | as configured | Runs a shell command in the workspace's directory and follows it for `wait_secs`; answers with the output so far and the exit code if it ended, else with the command's id |
| `command_output` | `id`; `offset` (optional, from the last result); `wait_secs` (optional) | no | New output of a running command since `offset`, waiting up to `wait_secs` for it to end |
| `kill_command` | `id` | no | Ends a command that still runs |

The result of each is a JSON object `{ id, status: "completed" | "running" | "killed" | "failed",
exit_code?, output, next_offset, truncated, workspace }`, classified as the workspace; `output` is
the command's text, stdout and stderr in the order they came. A command still running says so and
what to call next. The tool set's description tells the model that commands run in a directory
that persists across turns and calls, that every command starts in the workspace's directory - a
`cd` does not carry over - and that long commands are followed with `command_output`.

Left out on purpose, as before: Open Terminal's file endpoints (the workspace's own tools are W2;
the model has the shell), `/ports`, `/skills`, `/files/view`, the PTY, the notebooks.

#### C3. A call

`run_command`:

1. `turn.session()`; none - a turn without a channel, in tests - is a failed call.
2. The workspace: `session.workspaces()`, of them those with `may_use_here()`; the one named, or
   the only one, or a failed call that says what there is ("the session has no workspace that may
   be used in this channel"; "name one of: docs, data").
3. `workspace.require_usable()` - `GetWorkspace` and the two flow questions, as W2's tools will
   ask them - before anything is sent.
4. `POST /execute?wait={wait}&tail=…` with `{ command, cwd: "{mount}/{id}", env: { TERM: "dumb",
   NO_COLOR: "1", CAELUMORA_WORKSPACE: "{mount}/{id}" } }`, the headers `Authorization: Bearer …`
   (the secret resolved per call, never kept) and `X-Session-Id: {channel id}`. Open Terminal runs
   the command in a PTY: `TERM=dumb` keeps colours and cursor movement out of the output, and
   carriage returns are stripped.
5. The answer's entries (`{type: stdout | stderr | output, data}`) are joined; `log_path`, an
   absolute path inside the container, is dropped; what exceeds `max_output_bytes` is cut from the
   front.
6. A command that ended within `wait` is `completed` with its exit code; one still running is
   `running` with its id and `next_offset`.

`command_output(id, offset)`: `GET /execute/{id}/status?offset={offset}&wait={wait}` with the same
headers - Open Terminal answers 404 for a process started under another `X-Session-Id`. The
provider keeps no state between calls (as `mcp_tool` keeps none): every result carries
`next_offset`, and the model hands it back. A process Open Terminal has forgotten - it drops a
process five minutes after it ended - is `failed` with "no longer known; its output was given when
it ended".

`kill_command(id)`: `DELETE /execute/{id}`, then `status?wait=2` for the exit code.

Errors are `ToolError::ExecutionFailed` with the endpoint shown without credentials
(`RemoteEndpoint::shown`, `redact_in`), a 4xx or 5xx body shortened. A container unreachable at
call time is a failed call, not an unavailable set: the set was offered, and the next call may
succeed.

#### C4. Interruption

A call dropped at an `await` (issue 161) drops its request; the command would run on. This tool
kills it: a guard holding the client, the id and the headers sends `DELETE /execute/{id}` from its
`Drop`, in a task of its own bounded by five seconds, which the drop does not await. It is the one
thing a dropped tool does after its future is gone - a deliberate exception to "everything a tool
starts is bound to its future", documented in TOOLS-AND-COMMANDS.md with why: a command left
running keeps writing into the workspace after the turn that started it is gone. A `run_command`
that handed its command back as `running` kills nothing: the model chose to let it run, and
`kill_command` ends it.

#### C5. Approval, description, matcher

- `describe(name, params)`: what the approver reads - "Run in workspace 'docs':" and the command
  verbatim on a line of its own, never shortened. `command_output` and `kill_command` are never put
  up.
- `matcher`: the default, `ToolMatcher::exact`, in this plan; the shell matcher is issue 247, with
  this tool as its first user. Until then the card proposes the exact command, and a person who
  widens the pattern by hand is on their own - TOOLS-AND-COMMANDS.md says so already.
- `questions`: none.
- `presentation`: `ToolPresentation::Shell` on `run_command` (G).

#### C6. Module layout

`caelumora-runtime/src/tool_impl/open_terminal/`, split as `a2a_agent/` is (issue 210):

- `mod.rs`: the kind (`OPEN_TERMINAL_TOOL_KIND`), the configuration, `OpenTerminalProvider`
  (`ToolProvider`), `register`, and the tests against an axum stand-in that answers `/execute` and
  `/execute/{id}/status` as Open Terminal does.
- `client.rs`: `Terminal { endpoint: RemoteEndpoint, mount: String, http: reqwest::Client,
  secret_manager }` with `run(cwd, command, wait)`, `output(id, offset, wait)` and `kill(id)`, each
  given the security context (for the secret) and the channel id (for the header); the kill guard.
  This is the seam for A2: the provider asks `terminal_for(workspace)`, which answers the one
  configured (A1) or the one found or started for the workspace (A2).
- `wire.rs`: Open Terminal's request and answer shapes - `ExecRequest`, the answers of `/execute`
  and `/status`, an output entry - `pub(super)`, with the version they were read from (0.14.0) in
  the module doc.

The errors: a `TerminalError` in `client.rs` (connect, status, timeout, unknown process), described
through `describe_request_error`, mapped to `ToolError::ExecutionFailed` in `mod.rs`.

#### C7. What the model is told

`workspace_prompt_provider` (`component_impl/`, a `PromptProvider`, built with the security manager
and context as `notes` is): a part `[Your workspaces]` listing the session's workspaces usable in
this channel, each by name, classification and description ("as its owner describes it"), or that
none is attached - so that the model does not try. It is not part of the Open Terminal component:
W2's tools need the same part, and a personality with both gets it once. It reads through the
turn's views at the start of every turn, is public (names and descriptions are, as a briefing's),
and goes to `Start` by default.

### D. The door (issue 180)

What a tool sends out is judged where tools are offered, once per turn, not per call:

- `ToolSet` gains `destination: Option<Classification>` - where the arguments of its calls go;
  `None`: they stay in the system. `ToolProvider` gains `fn destination(&self) ->
  Option<Classification> { None }`, and `DefaultToolExecutor::tool_sets` copies it.
- `GuardedToolExecutor::tool_sets` becomes `async fn tool_sets(&self, context: &SecurityContext,
  channel: &Classification)`: a set whose destination the channel does not flow into
  (`may_publish(ctx, agent, &L, &D)`, the one flow question) is offered as unavailable - "its
  remote side is not cleared for this channel" - with its tools left out, so that
  `tool_sets_prompt_provider` and the web UI say so (issue 32). It is called in
  `AgentActor::request_response` with the channel's classification, and by the actor's
  `tool_sets()` for `MemberView`.
- A call of a tool not offered this turn is answered `unknown_tool` by `admit` and `check`
  (`turn.tools()`), so that the model cannot call a withheld tool by name - today `execute`
  dispatches by name over every provider.
- `mcp_tool` gains `destination`, default `public`, as an external personality that names no roles:
  the server's word is trusted only where configured. Existing configurations lose their MCP tools
  in every channel above public until they name one; the summary of the step says so.
- `open_terminal_tool`'s `destination` is C1's. The workspace adds its two flow questions per call
  (B2), which the door does not replace: the container's ceiling and the workspace's classification
  are two things.

The check is in the guard, not in a provider. 180's question "per agent or per tool set" is
answered: per set, by what the set declares. Whether sets are judged again when a channel is raised
during a turn stays as it is for tools: fixed per turn, and issue 141 gives the turn up when the
channel is lowered.

### E. Existing code that changes

| Where | What |
|---|---|
| `caelumora-model`: `session.rs` | `Session::workspaces`, `SessionUpdateEvent::WorkspacesChanged` |
| `store.rs`, `store_impl/*` | `EntityKind::Workspace`, the store methods, the `Actions` const, the suite |
| `security.rs` | five `ActionType` variants, `describe`, `OwnershipSecurityManager::check`, `AccessQuestions::may_change_workspace` |
| `event.rs` | four `GlobalEvent` variants |
| `tool.rs` | `ToolSet::destination`, `ToolPresentation::Shell` |
| `config.rs` | the `workspaces` section |
| `caelumora-runtime`: `runtime.rs` | holds the `WorkspaceManager`; `RuntimeManager::new` takes the section |
| `session.rs` | `SessionLoop::attach_workspace`, `detach_workspace`; `purge_session` detaches nothing - the workspace is not the session's |
| `controller/session.rs` | applies `WorkspacesChanged` |
| `scoped.rs` | the three loops take the workspaces of a `UserScope` along |
| `view/mod.rs`, `view/session.rs`, new `view/workspace.rs` | `Views::workspaces`, `SessionView::workspaces`, `WorkspaceView`; `Views` holds the manager |
| `event.rs` | the new events' topics |
| `tool.rs` | `GuardedToolExecutor::tool_sets` async, with the channel; `admit` and `check` refuse what was not offered |
| `tool_impl/default_tool_executor.rs`, `tool_impl/mcp_tool.rs` | `destination` |
| `actor_impl/agent.rs` | `request_response` and `Actor::tool_sets` pass the channel's classification |
| `error.rs` | `WorkspaceError` and `TerminalError` in `user_message` |
| `caelumora-rest-server` | `routes/workspaces.rs`, `Subject::Workspace`, `models.rs`, the router, the body limit; `AppState` unchanged - the manager is reached through the runtime manager |
| `caelumora-web-ui` | part G |

### F. Configuration

A section of `AppConfig` like the others (`deny_unknown_fields`, checked in `AppConfig::check`):

```toml
[workspaces]
root = "local/workspaces"   # where the directories are; made if missing
max_file_bytes = 52428800   # the largest file the API takes: 50 MiB
```

Next to Caelumora, in an example `examples/docker-compose.yml`:

```yaml
open-terminal:
  image: ghcr.io/open-webui/open-terminal:0.14.0   # pinned: the API is 0.x
  user: "${UID}:${GID}"                            # files it writes are Caelumora's to read and remove
  environment:
    OPEN_TERMINAL_API_KEY_FILE: /run/secrets/open_terminal_api_key
    OPEN_TERMINAL_FILE_BROWSER_ROOT: /workspaces
  volumes:
    - ./local/workspaces:/workspaces
```

The key is `OPEN_TERMINAL_API_KEY` in `local/secrets.json`, the file chain, and the container reads
the same value. The container's network reaches what the agents are meant to reach - not
Caelumora's own API, not the Docker socket: a compose network of its own, which Caelumora reaches
and not the other way round.

### G. The web UI

- `App.tsx`'s layout component is called `Workspace` today; it is renamed (`Layout`) first, so
  that the word means one thing.
- `types/index.ts`, `api/index.ts`: `Workspace`, `WorkspaceFile` and the requests;
  `listWorkspaces`, `createWorkspace`, `updateWorkspace`, `setWorkspaceClassification`,
  `setWorkspaceContributors`, `deleteWorkspace`, `restoreWorkspace`, `purgeWorkspace`,
  `listWorkspaceFiles`, `uploadWorkspaceFile` (a `fetch` with the file as its body),
  `workspaceFileUrl` (a plain link; the cookie goes with it), `deleteWorkspaceFile`,
  `sessionWorkspaces`, `attachWorkspace`, `detachWorkspace`. `globalEvents.ts`: selectors for the
  four events and for `WorkspacesChanged`.
- **Workspaces** as a thing of their own, reached from the sidebar as the agents are: a list with
  name, classification and owner; create (name, description, the classification picked as a
  channel's is), rename, describe, contributors, reclassify, delete, and the trash.
- **A session's workspaces**: in the session's header, the attached ones and a picker to attach one
  of the user's, and to detach, where `can_change`.
- **The workspace panel**: a third `sideTab` of `ChatView` (`'channel' | 'notes' | 'workspace'`):
  the session's workspaces as the viewer sees them, a directory listing with breadcrumbs, upload (a
  file input; drag and drop later), download (a link), delete. It reloads when opened and when a
  tool call of the channel finished. A workspace not usable here is greyed, with its classification
  as the reason.
- **The `shell` presentation** (`ToolCallView.tsx`, `PRESENTATIONS.shell`): the command as one line
  of code with the workspace's name; the output as a terminal block - monospace, scrolling, cut as
  the result says; the exit code; "running" with the id while the status says so. The approval card
  shows the command the same way.
- `docs/WEB-UI.md` describes the three.

### H. Steps

Each step builds, passes the tests and leaves the system usable; a step changes stored data only
where it says so.

1. **Issue 253 opened** from part B, with this plan (done with the plan); PLAN.md's W1 and W3 point
   to it.
2. **The workspace in the model** (B1, B2's actions, E's model rows): the type, the traits, the
   stores, the suite, the events, `Session::workspaces`. No behaviour yet; reviewable as a model
   change.
3. **The `WorkspaceManager` and the views** (B3, B4, F): the section, the manager with its directory
   and file operations, attaching on the `SessionLoop`, `scoped.rs`, the views and snapshots.
   Tests: the manager's path rules, one per refusal; the security suite - who may create, read,
   change, delete, restore, purge, attach, read and write files, and that a stranger sees nothing.
4. **REST and the web UI for workspaces** (B5, G without the shell presentation): the routes and
   DTOs, then the list, the session header and the panel. Two commits: REST, then UI.
5. **The component** (C): `tool_impl/open_terminal/` with its stand-in tests,
   `workspace_prompt_provider`, `ToolPresentation::Shell` with its UI handler, the seed personality,
   `examples/docker-compose.yml`, TOOLS-AND-COMMANDS.md, COMPONENTS.md, CONFIGURATION.md. The
   security suite: a call asks `CallTool`, `GetWorkspace` and `Publish` both ways. A run against a
   real container is made by hand, with the key from `local/secrets.json`; it is not in `cargo
   test`.
6. **The door** (D): `destination` on sets and providers, the guard, the refusal of what was not
   offered, `mcp_tool.destination`, CONFIDENTIALITY.md and TOOLS-AND-COMMANDS.md, the suite. Changes
   behaviour for MCP configurations that name no `destination`.
7. **Issues opened** at the end: A2 (a container per workspace, started by Caelumora); W2 (the
   workspace's own tools: reading through `WorkspaceView`, writing through the manager, what was
   read recorded as knowledge - a `KnowledgeSource::Workspace`); W4 (versions: a snapshot around
   every command and every write); 247 updated with this tool as its first user; 208 updated - an
   artifact as a file in a workspace of the channel. Then 247.

Step 6 can come before 5; nothing in 5 depends on it. Steps 2 to 4 are the workspace alone, useful
without Open Terminal: files people share with a session, for W2's tools to read later.

### I. What to watch

- **Paths.** Everything the API takes is relative and normalised; nothing of the root is ever in a
  message, an error or a response. The manager's tests try `..`, an absolute path, a prefix, a
  symbolic link out of the tree, a name with a NUL byte.
- **Ownership of files on disk.** The container writes as its user, and Caelumora must read and
  remove what it wrote. The compose example runs the container as Caelumora's uid; `entrypoint.sh`
  tolerates it (`chown … || true`). A deployment that cannot do so needs the root group-writable
  and a umask - a note in CONFIGURATION.md, not code.
- **No boundary between workspaces in A1** (part A): a command in one workspace can read every
  other under the mount. CONFIDENTIALITY.md says so next to the workspace's classification; A2 is
  the answer.
- **The container reaches the network.** A command can `curl` anything the container reaches,
  Caelumora's API included, and what the model saw in the channel can leave through it. The door
  (D) judges the channel against the container's `destination`; the network is the operator's (F).
  180's "not only MCP" is this.
- **The kill on drop** runs after the tool's future is gone (C4): bounded, logged, never awaited;
  the only such thing, and documented.
- **Output.** A PTY: `TERM=dumb`, carriage returns stripped, bounded by `max_output_bytes` from the
  front, `truncated` said; binary output is read as lossy UTF-8.
- **Open Terminal forgets a process five minutes after it ended**, and its `wait` is at most 300 s:
  `max_wait_secs` is bounded by that, and a forgotten process is a clear failure.
- **The same `X-Session-Id` for every request about one command**, or Open Terminal answers 404. It
  is the channel's id, so a command started in one channel is not seen from another.
- **The API is 0.x**: `wire.rs` names the version; a changed answer is a failed call naming the
  field, never a panic; the stand-in tests pin what is relied on.
- **`env` is per command**; no secret of Caelumora's goes into it, and the API key goes into a
  header, resolved per call.
- **The agent's context.** Which actor the ownership policy judges for `GetWorkspace` in a turn -
  the initiator who asked, or the agent that calls - the security suite settles in step 3. If it is
  the agent, attaching makes the session's agents contributors of the workspace, as a session makes
  them its contributors; then that is the rule, written down.
- **Issue 20.** A command is an effect nothing stages: done when done, and a turn given up does not
  undo it; the transcript tells the agent what ran (issue 145). W4 is where a workspace gets what
  the turn has - a state to go back to.
- **Issue 226.** The tool holds `&mut Turn` while a command runs, so one command at a time per
  agent; 226's first step changes `execute` for this provider as for every other.

### J. Not in this plan

A2 (a container per workspace), W2 (the workspace's own tools), W4 (versions), 247 (the shell
matcher), 208 (artifacts), `X-User-Id`, several files in one upload, an editor in the UI, Open
Terminal's interactive terminal in the UI, the TUI - it shows nothing of workspaces; a `/workspaces`
command may come later.

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
- **2026-10-08** — The open questions decided by the maintainer (*Decisions*): B; 180 as part of
  the work; one container per component with `X-Session-Id` per channel; Open Terminal as the
  executor over a workspace of Caelumora's own, which belongs to a user and is attached to
  sessions, and which is not bound to Open Terminal. The plan written (*Plan*), from the sources of
  Open Terminal 0.14.0 and of the model, runtime, REST server and web UI; the workspace opened as
  issue [253](253-workspace.md). Nothing implemented yet.
