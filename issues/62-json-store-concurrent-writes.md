# 62 — Concurrent writes of one JSON store file fail and can lose state

**Status:** in progress
**Area:** Persistence

## The problem

The JSON file stores write a file atomically by writing `<file>.tmp` and
renaming it over `<file>` (`write_json_atomic` in
`caelumora-model/src/store_impl/json.rs`). The temporary name is fixed. When
two writers store the same file at the same time, they share the temporary
file: the first `rename` moves it away, the second fails with "No such file
or directory", and which content ends up in the file depends on the
interleaving - possibly the older one.

It shows at every start of a session whose agent takes part in more than one
channel, e.g.:

```
WARN caelumora_runtime::actor: Cannot store agent state of agent 14fff58d-...: backend: No such file or directory (os error 2)
WARN caelumora_runtime::actor: Cannot store agent channel state of agent 14fff58d-...: backend: No such file or directory (os error 2)
WARN caelumora_runtime::channel: Cannot store state of channel 6537d0a0-...: backend: No such file or directory (os error 2)
```

## Why it matters

- A failed write is only logged (`warn!`); the state is then persisted in
  an older version, or its newest change is lost. After a restart an agent
  may have forgotten notes, messages or a pending approval.
- The warnings look alarming and hide real storage errors among them.

## Considerations

- **The temporary name must be unique per write**: a random or counter
  suffix (`<file>.<uuid>.tmp`), or `tempfile::NamedTempFile::persist` in the
  same directory. Then every writer's rename succeeds, and the last rename
  wins - atomic, but still last-writer-wins.
- **Why there are concurrent writers at all.** `AgentState` belongs to the
  agent, not to a channel, but each channel runs its own `AgentActor` for
  the agent, and each stores the agent state it holds. If those actors hold
  separate copies, they overwrite each other's changes (lost update) no
  matter how atomic the file write is. That is a question of ownership, not
  of the store, and needs checking: does every actor of an agent share one
  `AgentState`, and who is its single writer? Channel state is written by
  more than one path as well (the channel loop's `append_history` and at
  least one other writer at start).
- Per-file serialisation inside the store (a lock per path) removes the
  interleaving within one process but not a lost update between copies,
  and not between processes ([21](21-scaling.md)); the Postgres stores
  ([03](03-postgres-entity-store.md), [04](04-postgres-state-store.md))
  will need a concurrency rule (versions / optimistic locking) anyway.
- Minor: the store does blocking `std::fs` I/O inside `async fn`s.

## Done when

- Concurrent writes of the same file no longer fail.
- It is clear who writes `AgentState` and channel state, and concurrent
  changes are not lost.

## History

- **2026-09-27** — Issue created at the maintainer's request, after the
  warnings appeared while verifying [54](54-private-notes-visible-to-users.md)
  with an agent in two channels.
- **2026-09-28** — Analysed with the maintainer; three causes, not one:
  1. **A session can start twice.** `SessionManager::open_session` checks
     `get_session(id)` and starts the session if it is not running, with
     nothing in between: concurrent requests (the web UI opens several at
     once, e.g. the event streams of two channels) each find none and each
     start one - two `SessionLoop`s, with their own `ChannelLoop`s and
     `AgentActor`s for the same channels and agents. The log of the check of
     [54](54-private-notes-visible-to-users.md) shows every `ApproverActor`
     starting twice and the `UserActor` four times. Two actors of the same
     agent and channel, two loops of the same channel, write the same files -
     all three warnings above - and both process messages and overwrite each
     other's state.
  2. **The fixed temporary name** in `write_json_atomic`, as described.
  3. **`AgentState` belongs to the agent but is held by every instance of
     it**: each `AgentActor` (one per channel) loads its own copy at start and
     writes it back whole after every step. `AgentState` is empty today, so
     the lost update is latent; once it holds anything (cross-channel
     knowledge, generic component state, [65](65-generic-component-state.md)),
     the last writer wins, and a channel never sees changes made in another.
  Decided, in this order:
  1. **Single-flight for sessions:** starting a session is done once per
     session id; concurrent `open_session` calls wait for the same start
     instead of each starting one. `create_session` and `stop_session` go
     through the same mechanism.
  2. **Unique temporary names** (`<file>.<uuid>.tmp`) and non-blocking file
     I/O in the JSON stores.
  3. **One owner per agent and session, changes as deltas:** the
     `SessionLoop` keeps an `AgentStateHandle` per agent (the state behind a
     mutex, with its persistence), shared by all `AgentActor`s of that agent
     through the session handle they already have. The turn no longer holds
     `&mut AgentState` - a shared lock held across a model call would block
     every other channel; it reads a snapshot (or the handle) and stages its
     changes, which `record_turn` applies as deltas to the current state
     under the lock and persists there. No turn overwrites another's changes,
     and every next turn sees the current state, from other channels too.
     Persisted on change only. `AgentChannelState` keeps its single writer
     (the actor of that agent and channel) and `ChannelState` its (the
     `ChannelLoop`) - true once step 1 is done; documented.
  Multiple processes ([21](21-scaling.md)) are not part of this; the stores
  will need versions / optimistic locking ([03](03-postgres-entity-store.md),
  [04](04-postgres-state-store.md)), and the handle is where a conflict
  would be detected and the delta reapplied. Open for step 3: whether it is
  designed at once as the generic, delta-based state of
  [65](65-generic-component-state.md), and which first user `AgentState`
  gets.
- **2026-09-28** — Step 1 implemented: `SessionManager` keeps a lock per
  session id (`starts`), held while the session is started; `load_session`
  takes it and checks again whether the session is running meanwhile, so
  whoever comes second gets the loop the first started (if they may read
  it). `open_session` goes through it, `create_session` holds the same lock,
  `delete_session` drops it. Test `a_session_asked_for_at_the_same_time_starts_once`
  (fails without the lock). Verified with the web UI: every channel,
  approver and actor starts once (the user once per channel, an agent once
  per channel it is in); the warnings about channel state and agent channel
  state are gone. One warning about the agent state of an agent in three
  channels remains - cause 3, its three actors writing the same `AgentState`
  at once, for steps 2 and 3. Still open: `stop_session` is synchronous and
  does not take the lock, so a stop racing a start of the same session may
  leave it running.
