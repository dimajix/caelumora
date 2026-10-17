# 314 — The `SessionManager` deletes, restores and purges agents

**Status:** done
**Area:** Code structure
**Summary:** The `SessionManager` also deletes, restores and purges agents, though only deleting has to do with sessions, and agents have no owner of their own as groups do.

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
- **2026-10-11** — Done, as decided with the maintainer, on a different line than the
  considerations above: an `AgentManager` in the `RuntimeManager`, beside the other managers, owns
  deleting, restoring and purging agents, and the REST server calls it for all three; the
  `SessionManager` has no operation on agents left.
  - **Why not an `AgentManager` that calls the `SessionManager`:** making the agent leave the
    running sessions' channels would have needed it, and so a manager outside the `RuntimeManager`
    or one making the other act on its behalf. Instead, deleting changes no channel: a deleted agent
    rests in the channels it takes part in, and they follow its mark. A running session, subscribed
    to its agents' events, stops the agent's actor and announces it as a leave, and looks at all its
    agents again as it starts and after it lost events; a channel that starts leaves it out, since
    `Member::of` asks the store whether an agent exists, not the controllers, which keep the
    controller of one that still runs; the views show it as a former member, removed as its mark
    says, and without when once it was purged.
  - **Restoring brings it back** into every channel it rested in - the maintainer's decision; a
    running session rebuilds its actor and announces it as a join. That answers
    [227](227-agent-management.md)'s concern that a mistaken delete is final. Removing a resting
    agent from a channel changes the document alone, so that it does not come back there.
  - **What lies in a scope** stays with `scoped`, which deletes, restores and purges an agent of a
    session's or a channel's scope with it; such an agent takes part nowhere else and leaves
    nothing.
  - **The question asked before deciding** - whether deleting a user or a personality needs the
    same - is moot: nothing in the runtime deletes a user, a personality is not a participant, and
    an agent no longer leaves anything either.
  - `AgentManager::delete` and `purge` answer `false` for nothing the caller can see, as the
    `GroupManager`'s do; making an agent from a personality, without a template, is left to 227 and
    [221](221-adding-session-participants.md), and would be the `AgentManager`'s as well. A running
    session subscribes to the agents among its participants as it starts; once 221 lets participants
    be added, the subscription has to grow with them.
  - Stored data: agents deleted before this change are kept in their channels as former
    participants, and are not brought back by restoring them.
