# 121 — An agent in several sessions has one writer per session

**Status:** done
**Area:** Sessions and channels

## The problem

Since issue [120](120-participant-copies.md), a running session holds one `AgentController` per
agent (`ParticipantControllers`), shared by the agent's actors in all channels of that session. An
agent that takes part in two running sessions - a global-scope agent, say - still gets one
controller in each. Both are writers of the same document: each stores its own copy on a change, and
takes over the other's only once the change is announced.

## Why it matters

Issue [86](86-controllers-in-the-model.md) made every controller the one writer of its document,
holding a write lock while it changes and stores it. Two controllers of one agent hold two locks, so
two renames at the same time can each store a document the other has not seen yet - the last one
wins, and it may not be the one announced last. The same holds for `UserController`s, which only
read today but would not stay that way once users are changed through them (issue
[25](25-file-user-backend.md)).

## Considerations

- **A controller per entity for the process.** The `ControllerManager` keeps one controller per
  agent (and user) id, handed to every session that needs it, and drops it when the last one is gone
  (weak references). Mirrors `AgentStates` (issue 62), which already shares an agent's state across
  its actors in all sessions. Costs: a cache with a lifetime; `AgentController` announces with the
  session id of whoever made it (`AgentUpdated(session, ...)`) and would need to stop doing so.
- **Keep one per session**, and make the store refuse a stale write (a version on the document).
  Detects the conflict instead of avoiding it; every writer then has to handle a refusal.

## Done when

An agent's document has one writer in the process, however many sessions it takes part in.

## History

- **2026-09-29** — Opened while finishing issue 120, which brought the writers down from one per
  actor to one per session (option A there); option C there is this issue.
- **2026-09-29** — Discussed with the maintainer. Agents and users do not belong to a session, and
  one day the server runs in several instances (issue [122](122-several-instances.md)): a session in
  exactly one, agents and users - with their state - in several. So controllers stay, and must go on
  taking in announced changes, since changes made in another instance arrive only as events; and
  users are treated like agents - how many controllers there are is not a question of whether one
  writes.
- **2026-09-29** — Done. The `ControllerManager` keeps one `AgentController` and one
  `UserController` per id and instance (`agent(id)`, `user(id)`), loaded from the store as the
  system and held through weak references while actors use them, like `AgentStates`. It also takes
  every announced agent and user change into the live controllers, from one task started with the
  first controller - once per instance, and the place where another instance's changes will arrive.
  `ParticipantControllers` (issue 120, step 2) is gone again, and so is the sessions' subscription
  to their participants' topics: the channel asks the `ControllerManager` when it builds an actor
  (`Member::of`). Agent events lost their `SessionId` - `AgentCreated/Updated/Deleted(AgentId, …)` -
  and `AgentController` its `session_id`. Controllers are made only through the manager now (`new`
  is `pub(crate)`). Concurrent writes from several instances, and a late own announcement briefly
  restoring an older value, need versions: issue 122.
- **2026-09-29** — Renamed with the maintainer: `agent(id)`/`user(id)` are
  `agent_controller(id)`/`user_controller(id)`, and the `ControllerManager` documents why they
  differ from the `new_…_controller(document)` methods (shared per instance and loaded by id, versus
  made for a document a running session owns).
