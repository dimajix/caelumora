# 121 — An agent in several sessions has one writer per session

**Status:** open
**Area:** Sessions and channels

## The problem

Since issue [120](120-participant-copies.md), a running session holds one
`AgentController` per agent (`ParticipantControllers`), shared by the
agent's actors in all channels of that session. An agent that takes part in
two running sessions - a global-scope agent, say - still gets one
controller in each. Both are writers of the same document: each stores its
own copy on a change, and takes over the other's only once the change is
announced.

## Why it matters

Issue [86](86-controllers-in-the-model.md) made every controller the one
writer of its document, holding a write lock while it changes and stores
it. Two controllers of one agent hold two locks, so two renames at the same
time can each store a document the other has not seen yet - the last one
wins, and it may not be the one announced last. The same holds for
`UserController`s, which only read today but would not stay that way once
users are changed through them (issue [25](25-file-user-backend.md)).

## Considerations

- **A controller per entity for the process.** The `ControllerManager`
  keeps one controller per agent (and user) id, handed to every session
  that needs it, and drops it when the last one is gone (weak references).
  Mirrors `AgentStates` (issue 62), which already shares an agent's state
  across its actors in all sessions. Costs: a cache with a lifetime;
  `AgentController` announces with the session id of whoever made it
  (`AgentUpdated(session, ...)`) and would need to stop doing so.
- **Keep one per session**, and make the store refuse a stale write (a
  version on the document). Detects the conflict instead of avoiding it;
  every writer then has to handle a refusal.

## Done when

An agent's document has one writer in the process, however many sessions
it takes part in.

## History

- **2026-09-29** — Opened while finishing issue 120, which brought the
  writers down from one per actor to one per session (option A there);
  option C there is this issue.
