# 399 — A turn repeats what its work reads, and the work names one workspace three ways

**Status:** open
**Type:** refactoring
**Priority:** medium
**Area:** Code structure
**Summary:** `Turn` passes on nine of `AgentWork`'s methods to read and not the others, and `AgentWork` names its session, its workspace and the workspace's terminal by no common rule and fails in three shapes, so neither the line between the two types nor the one between views and handles can be read off their methods.

## The problem

The line between the two types is written down - in the module comment of
`caelumora-agents/src/work.rs`, in AGENT-TURN.md ("What only reads takes an `AgentWork`; what
stages changes takes the `Turn`") and in [344](344-workspace-manager-owns-their-use.md): an
`AgentWork` is what every piece of work of an agent has, an external agent's as much as a turn's,
to read; a `Turn` is what only a turn has - the changes it stages, its tool calls, its plan, what
it says - and acts in its channel (`workspace_terminal`, `workspace_shell`).

The methods do not show it:

- **`Turn` passes on part of what its work reads.** `session`, `channel_id`, `session_id`,
  `classification`, `label`, `security_context`, `agent_id`, `agent_state` and
  `agent_channel_state` are `Turn`'s too, each calling `self.work()` or doing what it does;
  `workspace`, `tool_sets`, `token_positions`, `profile` and `channel_attributes` are reached
  through `turn.work()` alone. Which are passed on follows no rule, every one passed on has two
  doc comments, and `turn.session()` does not say whether it is something of the turn's own.
- **The same operation has two names.** `AgentWork::workspace_for_reading()` is
  `Turn::workspace_terminal(Grant::Read)`: both ask `ActorRuntime::workspace_terminal` in the
  work's name. `workspace()` and `workspace_for_reading()` read as the same thing in two modes,
  though the one is a view and the other a handle.
- **Three methods of one family fail in three shapes.** `session()` answers an `Option`, the
  reason it has none - not attached, gone, or not to be read - swallowed in a `debug!` of
  `ActorRuntime::session_view`; `workspace()` a `ViewResult<Option<_>>`; `workspace_for_reading()`
  a `WorkspaceResult<_>`. The callers turn the `None` into errors of their own
  (`session().await.ok_or…`, four times in `caelumora-skills` and `memory.rs`).
- **A step of the navigation is missing.** `workspace()` goes from the session to the work's
  channel to its workspace, but the channel itself is not offered: five places write
  `session.channel(work.channel_id())` themselves (the briefing, members and language prompt
  providers, `notes.rs`, `caelumora-skills`).

The split between views and handles is not in question: a view tells what the work may know and
decides nothing, a terminal or shell is reached through the `ActorRuntime` and checked there.
Reaching terminals through the views was weighed and rejected in
[344](344-workspace-manager-owns-their-use.md) and [296](296-workspace-operations-on-the-view.md).
What is wrong is that the names and signatures do not follow it.

## Considerations

**A. The turn reads through its work.** `Turn` passes nothing on that `AgentWork` reads; what reads
is `turn.work().…`. `Turn`'s own methods change something, act in the channel, or read what the
turn itself holds (`staged_memory`, `staged_notes`, `requested_state`, the plan, the tool calls).
The rule of AGENT-TURN.md then shows in the API. Borrowing does not change: what `turn.work()`
returns borrows the turn as `turn.security_context()` does today. It costs about a hundred call
sites, mechanically, many in tests (`turn.label()` alone thirty times). The opposite - passing on
every method of `AgentWork` - was weighed and rejected: it doubles more, and every new method of
the work would have to be added twice.

**B. Names by one rule: a view is named after the thing, a handle after the handle.**

```rust
// AgentWork - reading
fn session(&self)   -> ViewResult<Option<SessionView>>
fn channel(&self)   -> ViewResult<Option<ChannelView>>   // new
fn workspace(&self) -> ViewResult<Option<WorkspaceView>> // channel()?.workspace()
fn workspace_terminal(&self) -> WorkspaceResult<(Workspace, Arc<dyn Terminal>)> // was workspace_for_reading, Grant::Read

// Turn - acting
fn workspace_terminal(&self, need: Grant) -> WorkspaceResult<(Workspace, Arc<dyn Terminal>)>
fn workspace_shell(&self) -> WorkspaceResult<(Workspace, Option<Arc<dyn Shell>>)>
```

The same operation has the same name on both types; the turn says what it needs, the work only
reads. `channel()` takes the place of the five copies of `session.channel(work.channel_id())`.

`session()` answering `ViewResult<Option<_>>` tells apart what is one `None` today: `None` - no
session, not attached or gone -, `Err` - it may not be read, or reading it failed. It needs
`ActorRuntime::session_view` to answer the same shape. It is the part of B open to most
discussion: a session the agent may not read becomes an error its callers see. To keep the
behaviour, the callers that today go on without a session go on so on an `Err` as well, logging
it; it can be split off from B if it is not wanted.

Seen beside it and left as it is: `WorkspacePromptProvider` reaches the view and the terminal one
after the other, asking the permission twice. It needs both - the view for what it tells, the
terminal for the instruction files - and B does not change that.

## Done when

- `Turn` has no method that only passes on one of `AgentWork`; components read through
  `turn.work()`.
- `AgentWork` offers `session`, `channel` and `workspace` as views in one shape of answer, and
  `workspace_terminal` for reading; `workspace_for_reading` is gone.
- No caller writes `session.channel(work.channel_id())` any more.
- AGENT-TURN.md names the rule for both types and for views and handles.
- A: a refactoring of its own, without behaviour changes; B after it.

## History

- **2026-10-17** — Opened at the maintainer's request, after a review of how `AgentWork` and
  `Turn` are used: both A and B, A first.
