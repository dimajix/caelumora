# 314 — The `SessionManager` deletes, restores and purges agents

**Status:** open
**Area:** Code structure

## The problem

`SessionManager` starts, stops, deletes, restores and purges sessions - and also deletes, restores
and purges agents (`delete_agent`, `restore_agent`, `purge_agent`), which the REST server calls for
`/agents/{id}`. Its description needs an "and also".

Only deleting has to do with sessions: a deleted agent leaves every channel it takes part in - in a
running session through `SessionLoop::leave_channels`, in a stored one by changing the channels'
documents - under the session's start lock, so that the session does not start meanwhile.
Restoring takes the agent's mark away and announces it; purging removes its document and its state.
Neither touches a session.

## Why it matters

Whoever looks for what happens to an agent looks for an owner of agents and finds part of it on the
owner of sessions. The next operation on agents that is not a store call has no obvious place, and
will likely end up here too, because the other two are here. Groups have an owner of their own,
`GroupManager`, reached through the `RuntimeManager`; agents do not.

## Considerations

- **Leave it.** Deleting needs the running sessions and their start locks, which the
  `SessionManager` alone holds; restore and purge sit next to delete so that the three are found
  together. Cheap, but the "and also" stays.
- **An `AgentManager`, like the `GroupManager`.** It owns deleting, restoring and purging agents.
  Deleting still has to make the agent leave the running sessions' channels: it asks the
  `SessionManager` for that - one method, e.g. "make `actor` leave every channel", which is about
  sessions and belongs there. The REST server calls the `AgentManager`.
- **Only restore and purge move.** Smaller, but it splits the three operations of one lifecycle
  across two owners.

Before deciding: whether deleting a user or a personality ever needs the same - leaving channels -
and whether one owner of "taking someone out of every session" would serve all of them.

## Done when

- The operations on agents have one owner whose description needs no "and also", or this issue
  records why they stay on the `SessionManager`.

## History

- **2026-10-11** — Opened while splitting `session.rs`
  ([311](311-split-channel-and-session-loops.md)), which left the question open.
