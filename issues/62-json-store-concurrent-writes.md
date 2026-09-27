# 62 — Concurrent writes of one JSON store file fail and can lose state

**Status:** open
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
