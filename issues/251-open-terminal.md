# 251 — Agents have no workspace to run commands and keep files in

**Status:** in progress
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
   it is reached in three layers (plan, part A): a *storage*, where the files lie; an *access*,
   how Caelumora reads and writes them - `local`, or an Open Terminal's file API; and a *toolbox*,
   what agents do with it - Open Terminal's shell now, tools of our own later. Not every
   implementation of one layer serves every one of another, and the configuration says which do.
6. **One workspace per session**, shared by all its channels, at most. Several per session is an
   extension the model allows and nothing needs now.
7. **A terminal is a configured service**, referred to by name, so that its endpoint and key stand
   in one place - superseded by 8.
8. **One trait pair per storage** (2026-10-08, later): `TerminalProvider` makes and removes a
   workspace's place and hands out its `Terminal`, the one handle for files and - as a capability,
   `Shell` - commands; `terminal_tool` is one generic toolbox over it, knowing no provider. The
   access layer and the service that owned the instances fold into this: the same handle for the
   same workspace is structural. `local` is a provider too, with a shell on the host as an opt-in
   stand-in for tests.
9. **The workspace's own file tools are a later step of `terminal_tool`**, not an issue of their
   own (PLAN.md's W2).
10. **A workspace is used in a session through its channels** (2026-10-08, before step 3): who is
    a member of a channel in which the session's workspace may be used uses it there - a person in
    the panel, an agent for whoever asked it there. A session for a request may have a channel
    with the person who asked and channels with helpers behind, and only the latter work on the
    files. The boundary is the flow rule between the workspace and the channel; the purpose is a
    switch on the channel.
11. **The switch: `Channel::uses_workspace`**, on by default, set by whoever may change the channel.
    It permits nothing the flow rule refuses.
12. **No contributors**: a workspace is its owner's. Outside a session only its owner works on it;
    in a session it is shared through the channels. Only its owner attaches it.
13. **A workspace lies in a scope** (2026-10-08, before step 4), as an agent does: a `ScopedName`
    in a session's scope, its owner's or the global one - not a channel's. The scope is the name
    space and the life, not the owner: a workspace is a user's wherever it lies. A session template
    may name one (`WorkspaceTemplate`): found in its scope, if in the storage the template names,
    or made there, and attached - in the session's scope one for every session, in the owner's or
    the global one shared by every session that finds it.
14. **What lies in a session's scope is used in that session alone** (`Scope::admits`), as an
    agent: not attached to another session, not used directly by its owner outside it, and
    deleted, restored and purged with its session, its files too.

## Plan

The plan has two subjects with one owner each: the workspace (issue [253](253-workspace.md); parts
B, E, F and G below) and the terminal it is reached through - Open Terminal being its first remote
implementation (this issue; parts A, C and D). They are planned together because the second cannot
be built without the first, and implemented in the steps of part H, each reviewable on its own.
Names of types and functions are proposals for the implementation to keep or to better; what is
fixed is where a responsibility lies.

### A. A workspace is reached through its terminal

Every workspace lies in a **storage**: a named place, configured once (`[workspaces.storage.
<name>]`), of which the workspace names only the name - no path is in a document or at the REST
API. Each storage is served by exactly one **`TerminalProvider`**, which makes and removes the
place of a workspace in it and hands out the **`Terminal`** of a workspace: the one handle through
which its files are read and written and, if the terminal has one, its **`Shell`** runs commands.
Whatever works on a workspace - the panel, uploads and downloads, the tools of agents - holds that
handle; nothing else reaches the files.

```rust
#[async_trait]
pub(crate) trait TerminalProvider: Send + Sync + Debug {
    async fn create(&self, id: WorkspaceId) -> TerminalResult<()>;              // the workspace's place: a directory, a volume
    async fn remove(&self, id: WorkspaceId) -> TerminalResult<()>;              // on purge, with everything in it
    async fn terminal_for(&self, id: WorkspaceId) -> TerminalResult<Arc<dyn Terminal>>; // the same handle for the same workspace
    fn has_shell(&self) -> bool;                                                // asked without reaching a terminal
    fn destination(&self) -> Option<&Classification>;                           // what may be put into this storage (B2)
}

#[async_trait]
pub(crate) trait Terminal: Send + Sync + Debug {
    async fn list(&self, dir: &WorkspacePath) -> TerminalResult<Vec<FileEntry>>;  // name, is_dir, size, modified
    async fn stat(&self, path: &WorkspacePath) -> TerminalResult<FileEntry>;
    async fn read(&self, path: &WorkspacePath) -> TerminalResult<(u64, ByteStream)>;
    async fn write(&self, path: &WorkspacePath, content: ByteStream, limit: u64) -> TerminalResult<()>;
    async fn delete(&self, path: &WorkspacePath) -> TerminalResult<()>;          // a file, or an empty directory
    async fn mkdir(&self, path: &WorkspacePath) -> TerminalResult<()>;
    async fn move_entry(&self, from: &WorkspacePath, to: &WorkspacePath) -> TerminalResult<()>;
    fn shell(&self) -> Option<Arc<dyn Shell>>;                                  // the capability, or not
}

#[async_trait]
pub(crate) trait Shell: Send + Sync + Debug {
    async fn start(&self, scope: &str, command: &str) -> TerminalResult<String>;   // its id, at once
    async fn output(&self, scope: &str, id: &str, offset: u64, wait: Duration) -> TerminalResult<CommandState>;
    async fn kill(&self, scope: &str, id: &str, offset: u64) -> TerminalResult<CommandState>;
}
```

A `CommandState` is `{ id, status: Completed(exit_code) | Running | Killed | Failed, output: String,
next_offset, truncated }`. A command runs in the workspace's directory and is seen only under the
`scope` it was started with - the tool passes the channel's id, so that a command started in one
channel is not seen from another. A `WorkspacePath` is a workspace's id and a path inside it, made
only by the `WorkspaceManager` (B3): relative, normalised, UTF-8; an implementation prepends what
it knows and never sees `..`.

**The providers**, chosen per storage by `provider`:

- **`local`** - a directory per workspace under `root`, read and written with `std::fs`, streaming;
  `create` is `mkdir`, `remove` is `remove_dir_all`. A symbolic link is followed only if its
  target stays inside (`canonicalize` and a prefix check). Its shell is off unless `shell = true`:
  then commands run on Caelumora's host, as its user, in the workspace's directory
  (`tokio::process::Command`, `kill_on_drop`, a process group of their own, output bounded) - a
  shell on the host is everything the server process may do, so it is opt-in, warned about in
  CONFIGURATION.md, and meant for development and as the stand-in `cargo test` runs the whole chain
  against without a container.
- **`open_terminal_shared`** - one Open Terminal instance, started outside Caelumora, that has the
  storage mounted at `mount`; the terminal of a workspace is that instance with the path
  `<mount>/<id>`. Files through its `/files/*` API, the shell through `/execute`. No boundary
  between the workspaces of the storage: a command in one can read every other under the mount.
  Fit for one server and a trusted team, where every workspace of the storage may be seen by
  everyone who can run a command in it - one classification per storage, and a storage per
  classification where there are several.
- **`open_terminal_docker`** - a volume and a container per workspace, started by Caelumora over the
  Docker daemon (crate `bollard`): `create` makes the volume, `terminal_for` starts the container on
  first use - image pinned, the volume at `/workspace`, labelled with the workspace's id, with an
  API key generated for it and kept in memory - stops it after `idle_stop_secs`, and `remove` takes
  container and volume away. Real isolation between workspaces. Its costs: the Docker socket is root
  on the host, so Caelumora's process gains that (a launcher service with that one right is the
  later answer); the first command of a session waits for a start; after a restart of Caelumora the
  keys are gone, so containers still running - found by their label - are replaced, and orphans are
  removed at start. (Amended in step 7: the key lives in the container's configuration, which only
  who holds the daemon reads, and is read back from it, so a container outlives a restart and is
  found again, not replaced; a task stops what no workspace in use holds - also what an earlier run
  left running - and nothing is removed but with its workspace. The volume is given to the image's
  user, who may not write a fresh one, and a container is reached without a proxy.)

Where a workspace lies decides what its terminal can do, and the tools are generic over it (C):
a storage whose terminal has no shell gives agents files alone. The provider's own configuration
is all `AppConfig::check` checks; there is no table of what fits what.

Weighed and not taken: Open Terminal's multi-user mode (`X-User-Id`) separates homes, not
workspaces, and is no boundary by its own word; the "Terminals" orchestrator needs an enterprise
license; a container per *agent* separates the wrong thing - a workspace is shared by the agents
and the people of a session; two traits for files and for the instance, with a manager between
them to make both use the same container - the one handle makes that structural instead.

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
    pub name: ScopedName,                  // in a session's scope, its owner's or the global one (decision 13)
    pub storage: String,                   // the name of its storage (part A); checked against the configuration
    pub description: Option<Description>,  // what it holds, as its owner describes it
    pub classification: Classification,    // one for the whole tree (issue 169)
    pub security_attributes: SecurityAttributes, // the owner, a user; no contributors (decision 12)
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted: Option<Removal>,
}

pub enum WorkspaceUpdateEvent {
    NameChanged(String),
    DescriptionChanged(Option<String>),
    ClassificationChanged(Classification),
}
```

What it does *not* hold: where its files are - the storage is named, the place is the id; no path
is stored or shown, as no path goes into a message - which session it is attached to, which is the
session's business, and its files: the place is the record. The storage is fixed when the
workspace is made; moving one to another storage is copying, not in this plan.

**One workspace per session, shared by its channels.** `Session` gains `workspace:
Option<WorkspaceId>` (`#[serde(default)]`) and `SessionUpdateEvent::WorkspaceChanged(Option<
WorkspaceId>)`. A session has at most one: its agents need no parameter to name it, the model is
told of *the* workspace, and the panel shows one. Which channels may use it is decided per channel
by the flow rule (B2) - a session's channels of several classifications share one workspace only
where it flows both ways, and only those whose switch `uses_workspace` is on (B2). Several per
session is an extension the model allows (a list instead of the option) and nothing here needs.
`Channel` and `ChannelTemplate` gain `uses_workspace: bool` (default `true`),
`ChannelUpdateEvent::UsesWorkspaceChanged(bool)`. `SessionTemplate` gains `workspace:
Option<WorkspaceTemplate { name, scope, storage, classification, description }>` (decision 13),
found or made as the session is made, as its agents are.

`GlobalEvent` gains `WorkspaceCreated(WorkspaceId)`, `WorkspaceUpdated(WorkspaceId,
WorkspaceUpdateEvent)`, `WorkspaceDeleted(WorkspaceId)` and `WorkspaceRestored(WorkspaceId)`;
`EventBus::publish` routes them to `All` - a topic of their own comes when something subscribes to
one workspace - and `PublishedEvent::read_action` answers `GetWorkspace`.

The classification is a `Classification`, not a level: a workspace may have an audience.

Stored data: nothing to delete; every new field has a default.

#### B2. Permissions and flow

Two questions, kept apart as everywhere (SECURITY.md):

**Who may** - the five actions under the ownership policy, as for every kind: `CreateWorkspace`
(with oneself as owner), `GetWorkspace`, `UpdateWorkspace` and `DeleteWorkspace` (the owner - a
workspace has no contributors, decision 12), `ListWorkspaces` (filtered by `Get`), and the trash
(`ReadDeleted`, `Restore`, `Purge` with `EntityKind::Workspace`). A workspace is used in two ways:

- *Directly*, outside a session: its owner, asked as `GetWorkspace`.
- *In a channel* of a session it is attached to (decision 10): a current member of the channel -
  a person, or an agent, which acts there for whoever asked it - if the channel's switch is on
  and the workspace and the channel flow both ways (below). No `GetWorkspace`: attaching shared
  it with the session's channels, and the channel decides who is in.

Attaching is the owner's alone (`UpdateWorkspace`), to a session they may change
(`UpdateSession`); detaching only asks `UpdateSession`, since it shares nothing. Changing the
document - name, description, classification - is `UpdateWorkspace`.

**What may flow** - the workspace is one container classified W, and its files carry no label of
their own:

- *Its owner* works on it directly only if cleared for W, as a member of a channel is:
  `JoinChannel { participant, roles, channel: &W }`, the clearance question with the workspace in
  the channel's place - asked when the workspace is made and when it is raised. Uploading needs no
  further rule: what a cleared person puts in is theirs to classify as W.
- *A member of a channel*, person or agent, reads and writes through the channel, classified L,
  and both directions must flow: W into
  L, since what a command prints reaches the channel, and L into W, since what the model puts into a
  command may be anything it saw there. `W.flows_to(L) && L.flows_to(W)`, asked as the two `Publish`
  questions the policy already answers - `may_publish(ctx, agent, &W, &L)` and `(…, &L, &W)`. With
  compartments and a read relation that is not symmetric this is stricter than equality, which is
  right. A member is cleared for L - asked when they joined and whenever the channel is raised -
  so no clearance for W is asked besides. It is asked per use by the operation (B3), and the
  result of a call is classified W
  (`ToolOutput::classified`), so that the `GuardedToolExecutor` withholds it where it does not
  flow, as for any tool.
- *The storage has a ceiling.* A storage's terminal is a place things are sent to - a container
  with a network, a host with a shell - and its `destination` says what may be put into it: W must
  flow into it, asked as `Publish` is, when a workspace is created in the storage and when it is
  raised. By transitivity, what an agent sends through a channel L into a workspace W in that
  storage flows into its destination too, so no question is asked per call for it. The defaults:
  `public` for a provider with a remote side or a shell - the operator says what the place is
  cleared for, as for an external agent that names no roles - and none for `local` without a shell,
  where nothing leaves the process.
- *Changing the classification* is the owner's act (`UpdateWorkspace`, then `Classify(new)`):
  lowering releases the whole tree, raising re-asks the owner's clearance and the storage's
  ceiling; which channels may use it follows at their next use. Nothing is fitted; there are no
  labels inside. Who did it is the audit log's (140).

These are existing questions with a workspace, or a storage, in the place of a channel. That
`Publish`, `JoinChannel` and `may_see_channel` name a channel where they mean a place is worth a
rename once 180 and 169 settle what a place is; it is not done here. New `ActionType` variants: the
five of the kind, nothing else.

#### B3. The runtime: `WorkspaceManager` (`caelumora-runtime/src/workspace.rs`)

One owner of "a workspace and its place", held by the `RuntimeManager` - made with the entity
store, the event bus, the security manager and the `workspaces` configuration, from which it builds
one `TerminalProvider` per storage - the one writer of workspace documents (under a lock per id:
store first, then announce) and the only code that holds the providers:

- `create(ctx, name, storage, description, classification) -> Workspace`: the storage must be
  configured and its ceiling admit W; `Classify(new)`; the store (which asks `CreateWorkspace`);
  `provider.create(id)` - a place that cannot be made is a creation that fails, and the document is
  purged again.
- `rename`, `describe`, `set_classification` (`UpdateWorkspace`, `Classify`; raising re-asks the
  owner's `JoinChannel` and the storage's ceiling).
- `delete` (marks; the files stay), `restore`, `purge` (`Purge` asked first, then
  `provider.remove(id)`, then the document; a place that cannot be removed leaves the document, so
  that the next purge tries again). Reading is the views' (B4).
- The two uses of B2, each the one check of its way, through `Enforcement::require`, before
  anything else: `terminal_of_owner(ctx, id)` (`GetWorkspace`) and `usable_in(ctx, session,
  channel)` - the session's workspace, if the channel belongs to the session, its switch is on,
  the caller is a current member and both flow questions are answered yes; `None` otherwise,
  without saying which of them failed. Then `provider.terminal_for(id)`.
- Paths: a `WorkspacePath` is made by parsing what a caller wrote - relative, `Path::components`
  with `Normal` components only, `..`, a root or a prefix refused, UTF-8 - so that no provider
  ever sees one that leaves the workspace. What a command creates under a name that is not UTF-8
  is listed lossily and cannot be opened. A path that does not fit is `TerminalError::InvalidPath`,
  which names the path the user wrote, never a root. Writing is bounded by `max_file_bytes`.

A workspace whose storage is no longer configured is kept, listed with "its storage is not
configured", and refused by every operation on its files.

`scoped.rs` takes a session's workspaces along: it marks them deleted and restores them in the
store, as it does every kind that lies in a scope, and has the manager purge them
(`purge_lying_in`), their places first. `owned` refuses one of a
session's scope, `usable_in` one whose scope the session is not, and `SessionLoop::set_workspace`
one of another session's scope (decision 14). `create_from_template` finds or makes the template's
workspace (`template::resolve_workspace`) and takes back one it made if the session cannot be
created.

Attaching is the session's: `SessionLoop::set_workspace(ctx, Option<WorkspaceId>)` asks
`UpdateSession` first, then for one attached `GetWorkspace` and `UpdateWorkspace` - its owner
shares it - and changes the session through its controller (`SessionUpdateEvent::WorkspaceChanged`).
The switch is the channel's: `ChannelLoop::set_uses_workspace(ctx, bool)` (`UpdateChannel`).
No flow question is asked at attaching: which channels may use it is decided per channel and turn
(B2). A workspace deleted while attached stays named and is read as absent, with a warning, as a
participant whose record is gone.

#### B4. The views

Reading goes through the views, for people and agents alike (SECURITY.md, *Views*):

- `Views::workspaces() -> Vec<WorkspaceView>` (the viewer's: `ListWorkspaces` filtered by `Get`),
  `Views::workspace(id)`, `Views::deleted_workspaces()`.
- `ChannelView::workspace() -> Option<WorkspaceView>`: the session's workspace if the viewer may
  use it in this channel (`usable_in`); nothing otherwise, also not why.
- `WorkspaceView`: `id`, `name`, `storage`, `description`, `classification`, `may_change`
  (`may_change_workspace`), `terminal() -> Result<Arc<dyn Terminal>, WorkspaceError>` (the
  manager's check of the way it was reached, asked again, then the terminal: a view node that
  hands out an operation after its check, as `MemberView::context()` asks `ReadAgentContext`),
  and `snapshot() -> WorkspaceSnapshot { id, name, storage, description, classification,
  can_change }`. `has_shell` comes with the shell (step 5).
- `Views` is given the `WorkspaceManager` - it holds `entities`, `states`, `components` and
  `running` today - and `WorkspaceView` reads through it with the viewer's context; nothing of a
  storage is reachable past the view.

The `Turn` needs nothing new: a tool reaches the workspace of its channel by
`turn.session()?.channel(turn.channel_id())?.workspace()`, as `channel_history` reaches the
channels, and its terminal by `workspace.terminal()`. No service is added to the `ComponentContext`.

#### B5. REST (`caelumora-rest-server`)

`routes/workspaces.rs`, handlers as the others: `Result<_, ApiError>`, `Subject::Workspace`, DTOs in
`models.rs`, errors through `ApiError::from_runtime`.

| Route | Does | Asks |
|---|---|---|
| `GET /workspaces` | the user's own workspaces | `ListWorkspaces`, `GetWorkspace` |
| `GET /workspaces/storages` | the configured storages by name, each with whether its terminals have a shell, for the picker | - |
| `POST /workspaces` `{name, storage, description?, classification?}` | creates; `classification` defaults to the channel default | `CreateWorkspace`, `Classify`, the storage's ceiling |
| `GET /workspaces/{id}` | the snapshot | `GetWorkspace` |
| `PATCH /workspaces/{id}` `{name?, description?}` | renames, describes | `UpdateWorkspace` |
| `PUT /workspaces/{id}/classification` | reclassifies | `UpdateWorkspace`, `Classify`, the owner's `JoinChannel`, the ceiling |
| `DELETE /workspaces/{id}`, `POST …/restore`, `POST …/purge` | the trash, as for agents | `DeleteWorkspace`, `ReadDeleted`, `Restore`, `Purge` |
| `GET /workspaces/{id}/files?path=` | lists a directory, for its owner | `GetWorkspace` |
| `GET /workspaces/{id}/files/{*path}` | downloads one file, streamed, as `Content-Disposition: attachment`, its media type guessed from its name (`mime_guess`), never inline - 208's rule: an HTML file would run in the web UI's origin | the same |
| `PUT /workspaces/{id}/files/{*path}` | uploads one file: the request body is the file, bounded by a `DefaultBodyLimit` from `max_file_bytes`; parent directories are made | the same |
| `DELETE /workspaces/{id}/files/{*path}` | removes a file or an empty directory | the same |
| `GET`, `PUT`, `DELETE /sessions/{sid}/channels/{cid}/workspace/files…` | the same four, for a member of the channel where the session's workspace may be used there | `GetChannel`, membership, both flow questions |
| `PUT /sessions/{sid}/workspace` `{workspace_id}`, `DELETE /sessions/{sid}/workspace` | attaches, detaches | `UpdateSession`; to attach `GetWorkspace`, `UpdateWorkspace` |
| `PUT /sessions/{sid}/channels/{cid}/uses-workspace` `{uses}` | the switch | `UpdateChannel` |

No multipart at Caelumora's API: one file per request is what the panel needs, and the body is the
file. These are the first routes that serve bytes - `Body::from_stream` for a download, the
request's data stream for an upload - and they stream through the terminal, so a file of
`max_file_bytes` is never held in memory. Error codes: `workspace_not_found` (also for one not
readable), `workspace_forbidden`, `invalid_path` (400), `file_too_large` (413), `not_cleared`
(403), `storage_unavailable` (503: the terminal failed - the instance down, the directory gone).

On the global stream: the four `Workspace…` events and `SessionUpdated(…, WorkspaceChanged)`.
Nothing announces a changed file - commands change files outside Caelumora's knowledge - so the
panel reloads when it is opened and after a tool call of the channel finished.

#### B6. The seed

No workspace in the seed: workspaces are someone's, and the seed is the system's. A personality in
`examples/seed.yaml` gets the component (C); a template nothing.

### C. The component `terminal_tool`

One generic toolbox over `Terminal`, knowing no provider: what its tools can do is what the
session's workspace's terminal can do.

#### C1. Configuration

```json
{ "id": "shell", "kind": "terminal_tool", "params": {
    "max_wait_secs": 60,
    "max_output_bytes": 65536,
    "requires_approval": true,
    "description": "The session's workspace: a shell in it." } }
```

- `max_wait_secs` (default 60, at most 300 - Open Terminal's own bound): how long one call follows
  a command before it hands it back as running.
- `max_output_bytes` (default 64 KiB): what of the output a result may hold; the rest is cut from
  the front, and the result says so.
- `requires_approval` (`true` by default, `false`, or `"unless_read_only"` once the file tools are
  there, H9): for the tools that change something - `run_command` and, later, the writing file
  tools. `command_output` and `kill_command` concern only a command the agent started and was
  approved for, and never ask. The `Approval` enum of `mcp_tool` moves to a shared place when the
  file tools come (H9), not before.
- `description`: of the tool set (issue 194); a text is built in.
- No `destination`: where the arguments go is the workspace's storage, bounded when the workspace
  was made (B2), so the door (D) has nothing to judge for this set - its `destination` is `None`.

Unknown fields are refused (`deny_unknown_fields`).

#### C2. The tools

The first version offers the shell; the file tools follow in a step of their own (H9):

| Tool | Parameters | Approval | Does |
|---|---|---|---|
| `run_command` | `command` (string); `wait_secs` (optional, at most `max_wait_secs`) | as configured | Runs a shell command in the session's workspace and follows it for `wait_secs`; answers with the output so far and the exit code if it ended, else with the command's id |
| `command_output` | `id`; `offset` (optional, from the last result); `wait_secs` (optional) | no | New output of a running command since `offset`, waiting up to `wait_secs` for it to end |
| `kill_command` | `id` | no | Ends a command that still runs |
| *later (H9)* `list_files`, `read_file`, `grep_files` | paths, a pattern, ranges | none under `"unless_read_only"` | Over `Terminal`, with what was read recorded as knowledge (`KnowledgeSource::Workspace`) |
| *later (H9)* `write_file`, `replace_in_file`, `delete_file` | path, content or old and new text | as configured | Over `Terminal`; the matcher proposes the path |

The result of a shell tool is a JSON object `{ id, status: "completed" | "running" | "killed" |
"failed", exit_code?, output, next_offset, truncated }`, classified as the workspace; `output` is
the command's text, stdout and stderr in the order they came. A command still running says so and
what to call next. The tool set's description tells the model that commands run in the session's
workspace, a directory that persists across turns and calls, that every command starts in it - a
`cd` does not carry over - and that long commands are followed with `command_output`.

Every tool is offered whenever the component is; a call that the workspace's terminal cannot serve
fails clearly - "the session has no workspace"; "the workspace's terminal has no shell" - and the
prompt part (C7) tells the model beforehand what it has. Offering only what the terminal can do
would need tool sets that depend on the session (`ToolProvider::tools` sees no turn); that is an
extension of `ToolProvider` for when a second case needs it, not made here.

#### C3. A call

`run_command`:

1. `turn.session()`; none - a turn without a channel, in tests - is a failed call.
2. `session.channel(channel id)?.workspace()`: none is a failed call that says so - "no workspace
   in this channel".
3. `workspace.terminal()` - membership, the switch and the two flow questions, before anything is
   sent.
4. `terminal.shell()`: none is a failed call that says so.
5. `shell.start(scope: channel id, command)`, then `shell.output(scope, id, 0, wait)` - split, so
   that the tool knows the id before it waits and can kill the command if the call is dropped
   (C4).
6. The output cut to `max_output_bytes` from the front; the result as C2 says.

`command_output(id, offset)`: `shell.output(scope, id, offset, wait)`. The provider keeps no state
between calls (as `mcp_tool` keeps none): every result carries `next_offset`, and the model hands
it back. A command the shell no longer knows is `failed` with "no longer known; its output was
given when it ended".

`kill_command(id)`: `shell.kill(scope, id, 0)`.

Errors of the terminal are `ToolError::ExecutionFailed` with what the `TerminalError` says - an
endpoint shown without credentials (`RemoteEndpoint::shown`, `redact_in`), a body shortened,
never a path of the host. A terminal unreachable at call time is a failed call, not an unavailable
set: the set was offered, and the next call may succeed.

**The Open Terminal shell** (`open_terminal/mod.rs`, behind `Shell`; the requests in
`open_terminal/client.rs`): `start` is `POST /execute?wait=0` with `{ command, cwd:
"<mount or /workspace>/<id>", env: { TERM: "dumb", NO_COLOR: "1", CAELUMORA_WORKSPACE: …,
OPEN_TERMINAL_API_KEY: "" } }` - the instance hands every command its own environment, its key
among it -, the headers `Authorization: Bearer …` (the key resolved per call
through the secret chain for `shared`, the generated one for `docker`) and `X-Session-Id: {scope}` -
Open Terminal answers 404 for a process asked for under another one. It runs the command in a PTY:
`TERM=dumb` keeps colours and cursor movement out, carriage returns are stripped. The entries
(`{type: stdout | stderr | output, data}`) are joined; `log_path`, a path inside the container, is
dropped. `output` is `GET /execute/{id}/status?offset=&wait=&tail=256` - offsets count entries,
not bytes, and 256 entries of a PTY are about the 1 MiB the local shell keeps; a process Open
Terminal has forgotten - it drops one five minutes after it ended - is `UnknownCommand`. `kill` is
`DELETE /execute/{id}?force=true` - SIGKILL to the process group, as the local shell sends -, then
`status?wait=2` for the exit code. The files are `/files/list`, `/files/view` (the bytes),
`/files/upload` (multipart), `/files/delete`, `/files/mkdir`, `/files/move`; `create` is
`/files/mkdir`, `remove` is `/files/delete`. There is no `stat`: what a path names is read from its
directory's list; and `/files/delete` removes a directory with everything in it, so emptiness is
checked first.

**The local shell** (`terminal/local_shell.rs`): `bash -c` in the workspace's directory, a process
group of its own, `kill_on_drop`, output read into a bounded buffer per command, commands kept by
`(scope, id)` until five minutes after they ended, as Open Terminal keeps them.

#### C4. Interruption

A call dropped at an `await` (issue 161) drops its request; the command would run on. This tool
kills it: a guard holding the `Arc<dyn Shell>`, the scope and the id calls `kill` from its `Drop`,
in a task of its own bounded by five seconds, which the drop does not await. It is the one thing a
dropped tool does after its future is gone - a deliberate exception to "everything a tool starts is
bound to its future", documented in TOOLS-AND-COMMANDS.md with why: a command left running keeps
writing into the workspace after the turn that started it is gone. A `run_command` that handed its
command back as `running` kills nothing: the model chose to let it run, and `kill_command` ends it.

#### C5. Approval, description, matcher

- `describe(name, params)`: what the approver reads - "Run in the workspace:" and the command
  verbatim on a line of its own, never shortened. `command_output` and `kill_command` are never put
  up.
- `matcher`: the default, `ToolMatcher::exact`, in this plan; the shell matcher is issue 247, with
  this tool as its first user. Until then the card proposes the exact command, and a person who
  widens the pattern by hand is on their own - TOOLS-AND-COMMANDS.md says so already.
- `questions`: none.
- `presentation`: `ToolPresentation::Shell` on `run_command` (G).

#### C6. Module layout

- `caelumora-runtime/src/terminal/`: `mod.rs` with the three traits, `WorkspacePath`, `FileEntry`,
  `CommandState`, `TerminalError` (in `user_message`) and the construction of a provider from its
  configuration; `local.rs`; `open_terminal/mod.rs` (what `shared` and `docker` share: the handle
  with the files and the shell over one instance), `open_terminal/shared.rs`,
  `open_terminal/docker.rs` (`bollard`), `open_terminal/client.rs` (`reqwest`, the headers, the
  key, errors through `describe_request_error`) and `open_terminal/wire.rs` (Open Terminal's
  request and answer shapes, `pub(super)`, with the version they were read from, 0.14.0, in the
  module doc - the API is 0.x).
- `caelumora-runtime/src/workspace.rs`: the `WorkspaceManager` (B3).
- `caelumora-runtime/src/tool_impl/terminal_tool.rs`: the component (C1-C5), with its tests
  against the `local` provider with its shell on - no network, no container.
- `caelumora-runtime/src/component_impl/workspace_prompt_provider.rs` (C7).
- Tests of `open_terminal`: the client against an axum stand-in that answers `/execute`,
  `/execute/{id}/status` and the `/files/*` used, as Open Terminal 0.14.0 does; `docker` by hand,
  or under an environment flag where a daemon is there, never in plain `cargo test`.

#### C7. What the model is told

`workspace_prompt_provider` (`component_impl/`, a `PromptProvider`, built with the security manager
and context as `notes` is): a part `[Your workspace]` naming the session's workspace by name,
classification and description ("as its owner describes it"), whether it may be used in this
channel, and what its terminal can do ("it has a shell"; "files only") - or that the session has
none, so that the model does not try. It is not part of `terminal_tool`: it says what is there,
whatever toolbox a personality has, and a personality with several gets it once. It reads through
the turn's views at the start of every turn, is public (names and descriptions are, as a
briefing's), and goes to `Start` by default.

### D. The door (issue 180)

What a tool with a fixed remote side sends out is judged where tools are offered, once per turn,
not per call:

- `ToolSet` gains `destination: Option<Classification>` - where the arguments of its calls go;
  `None`: they stay in the system, or are bounded otherwise. `ToolProvider` gains `fn
  destination(&self) -> Option<Classification> { None }`, and `DefaultToolExecutor::tool_sets`
  copies it.
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
- `terminal_tool` declares none: its remote side is the workspace's storage, whose ceiling bounds
  the workspace when it is made (B2), and the workspace bounds the channel per call. Two rules
  with one effect, each where its subject is.

The check is in the guard, not in a provider. 180's question "per agent or per tool set" is
answered: per set, by what the set declares. Whether sets are judged again when a channel is raised
during a turn stays as it is for tools: fixed per turn, and issue 141 gives the turn up when the
channel is lowered.

### E. Existing code that changes

| Where | What |
|---|---|
| `caelumora-model`: `session.rs` | `Session::workspace`, `SessionUpdateEvent::WorkspaceChanged` |
| `store.rs`, `store_impl/*` | `EntityKind::Workspace`, the store methods, the `Actions` const, the suite |
| `security.rs` | five `ActionType` variants, `describe`, `OwnershipSecurityManager::check`, `AccessQuestions::may_change_workspace` |
| `event.rs` | four `GlobalEvent` variants |
| `tool.rs` | `ToolSet::destination`, `ToolPresentation::Shell` |
| `config.rs` | the section `workspaces` with its storages, each provider's fields, their checks |
| `context.rs` | `KnowledgeSource::Workspace` (H9) |
| `caelumora-runtime`: `runtime.rs` | holds the `WorkspaceManager`; `RuntimeManager::new` takes the section |
| `session.rs` | `SessionLoop::set_workspace`; `purge_session` detaches nothing - the workspace is not the session's |
| `controller/session.rs` | applies `WorkspaceChanged` |
| `scoped.rs` | the three loops take the workspaces of a `UserScope` along |
| `view/mod.rs`, `view/channel.rs`, new `view/workspace.rs` | `Views::workspaces`, `ChannelView::workspace`, `WorkspaceView`; `Views` holds the manager |
| `event.rs` | the new events' topics |
| `tool.rs` | `GuardedToolExecutor::tool_sets` async, with the channel; `admit` and `check` refuse what was not offered |
| `tool_impl/default_tool_executor.rs`, `tool_impl/mcp_tool.rs` | `destination`; `Approval` moved out of `mcp_tool` (H9) |
| `actor_impl/agent.rs` | `request_response` and `Actor::tool_sets` pass the channel's classification |
| `error.rs` | `WorkspaceError` and `TerminalError` in `user_message` |
| `caelumora-rest-server` | `routes/workspaces.rs`, `Subject::Workspace`, `models.rs`, the router, the body limit; `AppState` unchanged - the manager is reached through the runtime manager |
| `caelumora-web-ui` | part G |

Nothing is added to `ComponentContext`: a component reaches a terminal through the views.

### F. Configuration

One section of `AppConfig`, which is YAML like the others (`deny_unknown_fields`, checked in
`AppConfig::check`; every provider checks its own fields):

```yaml
workspaces:
  max_file_bytes: 52428800       # the largest file the API takes: 50 MiB
  storage:
    dev:                         # a storage: its workspaces are made and reached by its provider
      provider: local
      root: local/workspaces
      shell: true                # opt-in: commands run on Caelumora's host, as its user
      destination: { level: internal }   # what may be put here; none for local without a shell
    sandbox:
      provider: open_terminal_shared
      endpoint: { url: "http://open-terminal:8000", auth: { type: bearer, secret: OPEN_TERMINAL_API_KEY }, timeout_secs: 10 }
      mount: /workspaces         # where the instance has this storage
      destination: { level: internal }
    isolated:
      provider: open_terminal_docker
      image: ghcr.io/open-webui/open-terminal:0.14.0   # pinned: the API is 0.x
      network: caelumora         # the compose network Caelumora reaches the containers in
      idle_stop_secs: 900
      destination: { level: confidential }
```

The checks: a `local` storage has a `root`; a `shared` one an `endpoint` and a `mount`; a
`docker` one an `image`; `destination` where a provider needs one and the operator gave none is
`public`. No storage configured is fine: workspaces are then refused with "no storage is
configured".

For `shared`, an example `examples/docker-compose.yml`:

```yaml
open-terminal:
  image: ghcr.io/open-webui/open-terminal:0.14.0
  environment:
    OPEN_TERMINAL_API_KEY: ${OPEN_TERMINAL_API_KEY:?the key, as in local/secrets.json}
  volumes:
    - home:/home/user              # a volume only the instance sees; Caelumora reaches the files through it
```

(Amended in step 6: the image runs as `user`, who may not write a fresh volume at `/workspaces`;
the storage is `/home/user/workspaces`.)

The key is `OPEN_TERMINAL_API_KEY` in `local/secrets.json`, the file chain, and the container reads
the same value. For `docker`, the daemon's socket is given to Caelumora's container or process, and
the containers are started in `network`, which reaches what the agents are meant to reach - not
Caelumora's own API, not the socket.

### G. The web UI

- `App.tsx`'s layout component is called `Workspace` today; it is renamed (`Layout`) first, so
  that the word means one thing.
- `types/index.ts`, `api/index.ts`: `Workspace`, `WorkspaceFile`, `Storage` and the requests;
  `listWorkspaces`, `listStorages`, `createWorkspace`, `updateWorkspace`,
  `setWorkspaceClassification`, `deleteWorkspace`,
  `restoreWorkspace`, `purgeWorkspace`, `listWorkspaceFiles`, `uploadWorkspaceFile` (a `fetch`
  with the file as its body), `workspaceFileUrl` (a plain link; the cookie goes with it),
  `deleteWorkspaceFile`, `sessionWorkspace`, `setSessionWorkspace`, `clearSessionWorkspace`.
  `globalEvents.ts`: selectors for the four events and for `WorkspaceChanged`.
- **Workspaces** as a thing of their own, reached from the sidebar as the agents are: a list with
  name, storage, classification and owner; create (name, storage from the configured ones - with
  whether it has a shell - description, the classification picked as a channel's is), rename,
  describe, reclassify, delete, and the trash.
- **The session's workspace**: in the session's header, the attached one, a picker to attach one of
  the user's, and detaching, where `can_change`.
- **The workspace panel**: a third `sideTab` of `ChatView` (`'channel' | 'notes' | 'workspace'`):
  the session's workspace as the viewer sees it, a directory listing with breadcrumbs, upload (a
  file input; drag and drop later), download (a link), delete. It reloads when opened and when a
  tool call of the channel finished. A workspace not usable in this channel is shown greyed, with
  its classification as the reason; a session without one says so and offers the picker where
  `can_change`.
- **The `shell` presentation** (`ToolCallView.tsx`, `PRESENTATIONS.shell`): the command as one line
  of code; the output as a terminal block - monospace, scrolling, cut as the result says; the exit
  code; "running" with the id while the status says so. The approval card shows the command the
  same way.
- `docs/WEB-UI.md` describes the three.

### H. Steps

Each step builds, passes the tests and leaves the system usable; a step changes stored data only
where it says so.

1. **Issue 253 opened** from part B, with this plan (done with the plan); PLAN.md's W1 and W3 point
   to it.
2. **The workspace in the model** (B1, B2's actions, E's model rows): the type, the traits, the
   stores, the suite, the events, `Session::workspace`. No behaviour yet; reviewable as a model
   change.
3. **The terminal traits, the `local` provider and the manager** (A's traits, `local` without its
   shell, B3, B4, F for `local`): the section, `terminal/mod.rs` and `local.rs`, the
   `WorkspaceManager` with its file operations, attaching on the `SessionLoop`, `scoped.rs`, the
   views and snapshots. Tests: the path rules, one per refusal; the security suite - who may
   create, read, change, delete, restore, purge, attach, read and write files, and that a stranger
   sees nothing.
3b. **The workspace's scope and the template** (decisions 13, 14): `ScopedName`, the scopes a
    workspace lies in, `WorkspaceTemplate`, finding or making it with the session, the rule of
    `Scope::admits`, deleting, restoring and purging with the session; the suite.
4. **REST and the web UI for workspaces** (B5, G without the shell presentation): the routes and
   DTOs, then the list, the session header and the panel. Two commits: REST, then UI.
5. **The shell of `local` and `terminal_tool`** (A's `Shell`, C1-C7): the opt-in host shell, the
   component with its shell tools and tests against it, `workspace_prompt_provider`,
   `ToolPresentation::Shell` with its UI handler, the seed personality, TOOLS-AND-COMMANDS.md,
   COMPONENTS.md, CONFIGURATION.md (with the warning). The security suite: a call asks `CallTool`
   and `Publish` both ways - the first in the asker's name, the second in the owner's. With this,
   the whole chain runs in `cargo test` without a container.
6. **`open_terminal_shared`** (A, C3's Open Terminal paragraph, C6's `open_terminal/`, F): the
   client, the wire, the handle with files and shell, the provider, the stand-in and its tests, the
   compose example. A run against a real instance is made by hand, with the key from
   `local/secrets.json`.
7. **`open_terminal_docker`** (A): `bollard`, the volume and the container per workspace, the
   label, the key, idle stop, removal, orphans at start; tests under an environment flag;
   CONFIGURATION.md on the socket and the network. Can be moved back without anything waiting for
   it.
8. **The door** (D): `destination` on sets and providers, the guard, the refusal of what was not
   offered, `mcp_tool.destination`, CONFIDENTIALITY.md and TOOLS-AND-COMMANDS.md, the suite.
   Independent of 5 to 7; changes behaviour for MCP configurations that name no `destination`.
9. **The file tools of `terminal_tool`** (C2's later rows): `list_files`, `read_file`,
   `grep_files`, `write_file`, `replace_in_file`, `delete_file` over `Terminal`;
   `KnowledgeSource::Workspace` and what was read recorded as knowledge; `"unless_read_only"`,
   with `Approval` moved out of `mcp_tool` first as a step without behaviour change; the matcher
   proposing the path; the suite. What PLAN.md calls W2.
10. **Issues opened** at the end: W4 (versions: a snapshot around every command and every write,
    through the shell where the terminal has one); 247 updated with this tool as its first user;
    208 updated - an artifact as a file in the session's workspace; tool sets that depend on the
    session, if a second case wants them (C2). Then 247.

Steps 2 to 4 are the workspace alone, useful without any shell: files people share with a session.
Step 5 gives agents a shell wherever the operator trusts the host with one; 6 and 7 move it into a
container.

### I. What to watch

- **The host shell** (`local` with `shell = true`) is Caelumora's own user running what a model
  wrote; `requires_approval` and the rules of a place are all that stands between. Off by default,
  warned about in CONFIGURATION.md, meant for development and tests.
- **Paths.** Everything the API takes is relative and normalised by the manager before any
  provider sees it; nothing of a root is ever in a message, an error or a response. The manager's
  tests try `..`, an absolute path, a prefix, a symbolic link out of the tree (`local`), a name
  with a NUL byte.
- **Isolation is the provider's** (part A): none between the workspaces of a `shared` storage, a
  container each with `docker`, Caelumora's own process with `local`. CONFIDENTIALITY.md says so
  next to the workspace's classification.
- **A terminal reaches the network.** A command can `curl` anything its instance reaches,
  Caelumora's API included, and what the model saw in the channel can leave through it. The
  storage's ceiling bounds what may be put there (B2); the network is the operator's (F). 180's
  "not only MCP" is this.
- **The kill on drop** runs after the tool's future is gone (C4): bounded, logged, never awaited;
  the only such thing, and documented.
- **Output.** A PTY at Open Terminal: `TERM=dumb`, carriage returns stripped; everywhere bounded by
  `max_output_bytes` from the front, `truncated` said; binary output is read as lossy UTF-8.
- **Open Terminal forgets a process five minutes after it ended**, and its `wait` is at most 300 s:
  `max_wait_secs` is bounded by that, the local shell keeps the same five minutes, and a forgotten
  command is a clear failure.
- **The same scope for every request about one command**, or Open Terminal answers 404. It is the
  channel's id; file operations send none.
- **The API is 0.x**, and the panel depends on it as much as the shell does: `wire.rs` names the
  version; a changed answer is a failed call naming the field, never a panic; the stand-in tests
  pin what is relied on.
- **`docker`**: the socket is root on the host; a start takes seconds, so the first command of a
  session waits and the tool's timeout allows for it; keys live in memory and a restart of
  Caelumora replaces the containers; a crash leaves orphans, found by their label and removed at
  start; the image is pinned. (Amended in step 7, see part A: keys are read back from the
  containers, which are found again; what runs unheld is stopped.)
- **`env` is per command**; no secret of Caelumora's goes into it, and a key goes into a header,
  resolved or generated per instance.
- **The agent's context.** The ownership policy judges the initiator. The call is admitted in the
  turn's context - whoever asked the agent - but the workspace is reached through the agent's
  views, in its owner's name, as every other place it reads (SECURITY.md, *In whose name an agent
  acts*; issue [255](255-whose-name-agents-act-in.md)). In a session no `GetWorkspace` is asked
  (decision 10): the channel decides, and the agent is its member.
- **Issue 20.** A command is an effect nothing stages: done when done, and a turn given up does not
  undo it; the transcript tells the agent what ran (issue 145). W4 is where a workspace gets what
  the turn has - a state to go back to.
- **Issue 226.** The tool holds `&mut Turn` while a command runs, so one command at a time per
  agent; 226's first step changes `execute` for this provider as for every other.

### J. Not in this plan

W4 (versions), several workspaces per session, moving a workspace to another storage, tool sets
that depend on the session, 247 (the shell matcher), 208 (artifacts), `X-User-Id`, a launcher
service in place of the Docker socket, several files in one upload, an editor in the UI, Open
Terminal's interactive terminal in the UI, the TUI - it shows nothing of workspaces; a `/workspace`
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
- **2026-10-08** — The plan rewritten on three layers the maintainer named - storage, access,
  toolbox - with a terminal as a configured service used by two of them, and with at most one
  workspace per session, shared by its channels (*Decisions* 5 to 7). The workspace no longer
  needs a directory on Caelumora's host: `WorkspaceAccess` has a `local` and an `open_terminal`
  implementation, and a storage says who sees it where. Mount, uid and "a container per
  workspace" moved from the component to the storage and the terminal.
- **2026-10-08** — Rewritten once more, on the maintainer's proposal to fold the access layer and
  the instances into one `TerminalProvider`/`Terminal` pair with `Shell` as a capability
  (*Decisions* 8 and 9): one provider per storage - `local`, `open_terminal_shared`,
  `open_terminal_docker` - and one generic `terminal_tool`. The compatibility table, the
  `terminals` section and the extension of `ComponentContext` are gone; a component reaches the
  terminal through `WorkspaceView::terminal()`. The storage's `destination` bounds a workspace
  when it is made, so the door of 180 judges `mcp_tool` alone. The file tools are step 9.
- **2026-10-08** — Step 2 of the plan done: the workspace in the model - see issue
  [253](253-workspace.md).
- **2026-10-08** — Before step 3, decided how a workspace is used in a session (*Decisions* 10 to
  12), from a scenario of the maintainer: a session for a request, a channel with the person who
  asked, channels with helpers behind, and only the latter on the files. So a workspace is used
  through the channels of the session it is attached to - membership, a switch on the channel
  (`uses_workspace`, on by default) and the flow rule both ways - and has no contributors: outside
  a session only its owner works on it, and only its owner attaches it. B1 to B5, C3, E, F (YAML,
  not TOML: the configuration is YAML), G and I amended.
- **2026-10-08** — Step 3 of the plan done: the terminal traits, the `local` provider without its
  shell, the `WorkspaceManager`, attaching, the channel's switch, the views and the security suite
  - see issue [253](253-workspace.md).
- **2026-10-08** — Before step 4, on the maintainer's proposal: a workspace lies in a scope as an
  agent does, and a template may name one (*Decisions* 13 and 14). Done as step 3b - see issue
  [253](253-workspace.md).
- **2026-10-08** — Step 4 of the plan done: workspaces over REST and in the web UI - see issue
  [253](253-workspace.md).
- **2026-10-08** — Step 5 of the plan done. The local shell (`terminal/local_shell.rs`, opt-in by
  the storage's `shell`, with `destination` its ceiling - public by default), `terminal_tool` with
  `run_command`, `command_output` and `kill_command`, `workspace_prompt_provider`,
  `ToolPresentation::Shell` with its handler in the web UI, the seed personality, the example
  configuration (the shell storage commented out) and the documents. Plan amended: `Shell` has
  `start` instead of `run`, so that the tool knows a command's id before it waits and can kill it
  on drop, and `kill` takes the offset; `TerminalProvider::has_shell` answers without reaching a
  terminal; the local shell has a module of its own. The suite showed that the call is admitted in
  the asker's name and the workspace reached in the owner's; SECURITY.md describes it, and issue
  [255](255-whose-name-agents-act-in.md) asks whether that is right. A command handed back as
  running is ended by nothing but the model: issue [254](254-commands-run-unbounded.md).
- **2026-10-08** — Step 6 of the plan done: `open_terminal_shared` (`terminal/open_terminal/`: the
  wire, the client, one handle per workspace for files and shell, the provider), tested against an
  axum stand-in that answers as 0.14.0 and runs commands, and by hand - an ignored test and the REST
  API - against a real instance of 0.14.0. Before it, on the maintainer's decision, the endpoint
  moved into the model (`caelumora_model::remote::RemoteEndpoint`), so that a storage's
  configuration is typed and checked when it is loaded (`StorageConfig::check`); a step of its own
  without a change of behaviour. The storage's key is read in the system's name, as the server's
  own keys are. Found and documented (CONFIDENTIALITY.md, STORAGE.md): the instance hands every
  command its own key - blanked in what Caelumora starts, but readable from the server's process -
  so `X-Session-Id` keeps the tool's channels apart, not a command from the API. C3's paragraph on
  the Open Terminal shell and F's compose example amended.
- **2026-10-08** — Step 7 of the plan done: `open_terminal_docker`
  (`terminal/open_terminal/docker.rs`, crate `bollard` 0.21) - a volume and a container per
  workspace, made and started on first use, the image pulled if missing, the volume given to the
  image's user, reached without a proxy at its address in `network` (`bridge` by default), stopped
  after `idle_stop_secs` unused. The client takes a key made for its container as well as one from
  the secret chain. Unlike part A said, the key is read back from the container's configuration, so
  containers are found again after a restart instead of being replaced, and the task that stops idle
  containers also stops what an earlier run left running; nothing is removed but with its workspace.
  Tested by hand against the local daemon under `CAELUMORA_DOCKER_TESTS` (with the image `slim`,
  whose server runs as `user` too): start, a command, stopping when idle, finding it again from a
  later provider, stopping what was left, removing container and volume. Pulling an image was not
  tried: the published 0.14.0 is 1.4 GB.

