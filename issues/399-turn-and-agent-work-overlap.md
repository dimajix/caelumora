# 399 — A turn repeats what its work reads, and the work names one workspace three ways

**Status:** done
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

**The name.** `AgentWork` sounds like an entity - the agent's work - and, to some ears, like
changing something, though it is a handle to read: who works, where, in whose name, bound by what,
what it has seen so far, the agent's states, and the way into its session. What it is, the views
already have a word for in part: a `Viewer` reads as a `SecurityContext` from a `Place`
(`caelumora-runtime/src/view/mod.rs`) - the channel, the classification it reads at, the session.
A name for it says where a piece of work stands and what it sees from there. Weighed:

- `Situation` - the circumstances a piece of work is in. Reads best where the type is used most,
  as a question put to a component: the prompt part, the tools that suit this situation
  (`get_prompt(&self, situation: &Situation)`, `offered(situation)`). Does not sound like
  changing, is taken nowhere in the code, and is the same word in German. A little general.
- `Perspective` - what is seen, from where and how far; says most clearly that it only reads, and
  fits `Viewer` and `Place`, and that it shifts as the work sees more (`label`). Reads awkwardly in
  the providers' signatures ("the prompt from a perspective").
- `Vantage`, `Purview` - more precise than `Perspective`, but rare words.
- `Environment` - too broad, and taken by environment variables and the configuration;
  `Workplace` - a place that outlives a piece of work, and too close to `Workspace`; `Access` -
  sounds like a permission, next to `AccessPolicy` and `AccessQuestions`.
- `Mandate`, `Remit`, `Brief` - only the authority and the bounds, not the states and the way into
  the session; `Brief` next to the briefing prompt part.
- `Desk`, `Workbench`, `Station`, `Seat` - sound like tools and doing, or outlive a piece of work
  (`turn_id`, `label` are a piece of work's).
- `WorkView`, `WorkContext`, `WorkRef` - each ending means something else already: a view is
  reached through `Views` and hands out no terminal (296, 344), a context is five things
  (`SecurityContext`, `EventContext`, `ContextRef`, `ContextProvider`, `ContextResolver`), a ref
  is a reference by digest (`ContextRef`, `KnowledgeRef`).

Decided: `Situation`, reached as `Turn::situation()` in place of `Turn::work()`; the parameters
named `work` that hold one are named `situation`. The model's `Work` keeps its name - it is a
piece of work, what a turn and an external agent's exchange have in common, and it changes as the
work goes on (`see`, `record_knowledge`); a situation holds the work it is about. The renaming
comes first, as part of A, since A touches the same call sites (`turn.work().…`).

**A. The turn reads through its situation.** `Turn` passes nothing on that its `Situation` reads;
what reads is `turn.situation().…`. `Turn`'s own methods change something, act in the channel, or
read what the turn itself holds (`staged_memory`, `staged_notes`, `requested_state`, the plan, the
tool calls). The rule of AGENT-TURN.md then shows in the API. Borrowing does not change: what
`turn.situation()` returns borrows the turn as `turn.security_context()` does today. It costs about
a hundred call sites, mechanically, many in tests (`turn.label()` alone thirty times). Passing on
every method of `Situation` instead was weighed and rejected: it doubles more, and every new method
of the situation would have to be added twice.

**B. Names by one rule: a view is named after the thing, a handle after the handle.**

```rust
// Situation - reading
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

- `AgentWork` is `Situation`, reached as `Turn::situation()`; the model's `Work` is unchanged.
- `Turn` has no method that only passes on one of `Situation`; components read through
  `turn.situation()`.
- `Situation` offers `session`, `channel` and `workspace` as views in one shape of answer, and
  `workspace_terminal` for reading; `workspace_for_reading` is gone.
- No caller writes `session.channel(work.channel_id())` any more.
- AGENT-TURN.md names the rule for both types and for views and handles.
- A: a refactoring of its own, without behaviour changes; B after it.

## History

- **2026-10-17** — Opened at the maintainer's request, after a review of how `AgentWork` and
  `Turn` are used: both A and B, A first.
- **2026-10-17** — Decided with the maintainer: `AgentWork` is named `Situation` (*The name*),
  renamed as the first step of A.
- **2026-10-17** — A in the working tree of the branch: `AgentWork` is `Situation`
  (`caelumora-agents/src/situation.rs`), reached as `Turn::situation()` and, for an external
  agent's work, `ActorBody::situation`; the parameters that hold one are named `situation`. `Turn`
  lost `session`, `channel_id`, `session_id`, `classification`, `label`, `security_context`,
  `agent_id`, `agent_state` and `agent_channel_state`; its callers, and the turn's own files, read
  through `situation()`. `turn_id` and `agent_name` stay on `Turn`, since a situation has neither.
  AGENT-TURN.md describes the situation and the rule, TOOLS.md and MEMORY.md name it. No behaviour
  changes.
- **2026-10-17** — Decided with the maintainer, departing from B as written: where a session that
  is reached cannot be read, a prompt part or a tool passes the error on (`?`), as it passes on
  one of its channel already, instead of going on as if there were no session - a part falls under
  its fail policy, a tool tells why. Where nothing can be passed on - the agent's memory, the notes
  part, which goes on without the others' notes - it is logged and the work goes on as before.
- **2026-10-17** — B in the working tree of the branch: `ActorRuntime::session_view` and
  `Situation::session` answer `ViewResult<Option<SessionView>>` - `None` where no session is
  reached, `SessionNotFound` where it may not be read -, `Situation::channel` is new and takes the
  place of the copies of `session.channel(work.channel_id())`, `Situation::workspace` follows it,
  and `workspace_for_reading` is `Situation::workspace_terminal`. `AgentMemory` reads the session
  through one function of its own that logs a failure. AGENT-TURN.md names the rule for views and
  handles, PERMISSION-CHECKS.md the refusal.
- **2026-10-17** — Done, with a test that work which may not read the session it reaches fails a
  prompt part rather than telling of nobody
  (`work_that_may_not_read_its_session_fails_the_part_rather_than_naming_nobody`).
