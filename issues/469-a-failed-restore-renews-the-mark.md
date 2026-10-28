# 469 — A restoring that fails marks anew, and a second one leaves behind what was deleted with it

**Status:** in progress
**Type:** bug
**Priority:** medium
**Area:** Persistence
**Summary:** When restoring a session, a channel or a project fails part of the way, the roll-back marks it deleted again with a new time, so that a second restoring no longer finds what was deleted with it at the first.

## The problem

What goes with a holder - the groups of its participants, what lies in its scope, a session's own
project, a project's sessions - is restored by the time the holder was deleted: what was deleted at
`since` or later comes back with it (`scoped::deleted_since`, issue
[178](178-scoped-lookup-by-name.md)). When restoring fails after the holder's mark was taken
away, the roll-back deletes the holder again (`EntityStore::delete`), and the store stamps a new
`Removal` (`caelumora-model/src/store/documents.rs:246`, `Removal::now`). The next restoring takes
`since` from that new mark, and everything deleted at the first time is older: it stays deleted.

The places:

- `SessionManager::restore` (`caelumora-runtime/src/session/manager.rs:684`): the groups of its
  participants restored, then its own project; failing, the groups are deleted again and the session
  is marked again - the groups with a time before the session's new mark, what lies in its scope
  with the old one.
- `SessionLoop::restore_channel` (`caelumora-runtime/src/session/channels.rs:457`), alike for a
  channel and its group.
- `ProjectManager::restore` (`caelumora-runtime/src/project/manager.rs:146`): its members' group,
  its sessions, what lies in its scope.

Step 2a of issue [467](467-projects-hold-what-outlasts-a-session.md) works around it for a
session's own project alone: it is taken back whenever it was deleted (`max_by_key` at
`session/manager.rs:696`), since it goes only with its session. The test
`a_session_keeps_its_groups_and_its_project_all_or_none` asserts the failure path and stops short of
a second restoring for this reason.

## Why it matters

A store that fails once - a backend not reachable for a moment - turns a restoring that is retried
into one that loses the groups of the participants, the agents and workspaces of the scope, a
project's sessions: the holder comes back without them, and they can be restored only one by one,
by whoever owns each.

## Considerations

- **A. Put the former mark back.** A store operation that marks with a given `Removal`
  (`EntityStore::delete_as(ctx, id, removal)`, or `restore` answering a token that undoes it): the
  roll-back sets back exactly what was there. One operation more on every store, used only by
  roll-backs; the backends keep what they are given.
- **B. Re-mark what was restored, not the holder.** The roll-back marks the groups again with the
  holder's original time instead of `now`. Needs the same store operation, for the groups instead.
- **C. Order the roll-back so that every re-mark is later than the holder's.** The holder first,
  then what goes with it. Covers the groups, not what lies in the scope and was never restored,
  whose mark stays older than the holder's new one.

- **D. One `Removal` per deletion.** The cause is that "went with it" is read from times that
  every step of a deletion stamps anew, and a roll-back with them. Instead, a deletion is one
  `Removal`, made once by the act - who, when - and given to everything that goes with the
  holder: the holder, the groups of its participants, what lies in its scope, a session's own
  project, a project's sessions. What went with it is then what carries its `Removal`, compared
  for equality, not by time; and a roll-back marks again with the `Removal` it read before it
  restored. `EntityStore::delete` takes the `Removal` (`delete(ctx, id, removal)`) - there is no
  second operation beside it, and the store no longer stamps a mark itself. Costs: every caller
  of `delete` names the `Removal` (`Removal::now(ctx.caller_id)` for a deletion of its own);
  `ScopeMember` - implemented by the plugins of skills and artifacts as well - takes it instead of
  a time; the store no longer guarantees that `by` is its caller, only for a caller other than
  the system (the groups are deleted in the runtime's name, but by whoever deleted the session).
  In return `since`, `deleted_since` and the reliance on the order of the clock go, and no
  roll-back needs an operation of its own.

Recommendation was A; the maintainer asked whether a `Removal` could not be given to every
`delete`, and chose D.

## Decision

D, in two steps:

1. **A deletion is one `Removal`** (a restructuring without a change of what comes back):
   `EntityStore::delete(ctx, id, removal: &Removal)`, refused (`Validation`) if `removal.by` is
   not the caller and the caller is not the system. The act that deletes makes the `Removal`
   once and gives it to all that goes with it: `ScopeMember::delete_lying_in` and
   `ScopedEntities::delete_lying_in` take it, as does `GroupManager::delete_participants`; the
   `SessionManager` deletes a session with a given `Removal` (`pub(crate) delete_with`) for the
   `ProjectManager`, whose deletion takes its sessions along. Restoring still compares times
   (`since`), which the shared mark keeps true: everything deleted with the holder carries its
   time. The groups' `Removal::by` names whoever deleted the session or channel, no longer the
   system.
2. **What carries the mark comes back with it**: `ScopeMember::restore_lying_in`,
   `ScopedEntities::restore_lying_in` and `GroupManager::restore_participants` take the
   holder's `Removal` instead of `since`, and restore what carries it (`deleted_since` becomes a
   comparison for equality). The roll-backs of `SessionManager::restore`,
   `SessionLoop::restore_channel`, `ProjectManager::restore` and of `restore_participants` mark
   again with the `Removal` they read before restoring; the work-around for a session's own
   project goes. The tests retry a restoring that failed.

## Done when

- A restoring that fails, retried, brings back everything that was deleted with the holder.
- Everything deleted with a holder carries its `Removal`, and restoring takes back what carries it.
- `SessionManager::restore`, `SessionLoop::restore_channel` and `ProjectManager::restore` roll
  back with the mark they read; the work-around for a session's own project is gone.
- The tests retry a failed restoring and find what went with the holder.

## History

- **2026-10-28** — Opened at the maintainer's request from the findings of issue 467, where the
  test of steps 2a showed it for the groups of a session and the review of step 2b for a project.
- **2026-10-28** — The maintainer asked whether `delete` could not always be given a `Removal`;
  considered as D, one `Removal` per deletion, which compares marks instead of times. Chosen, in
  two steps (*Decision*); work begun on branch `issue-469-one-removal-per-deletion`.
- **2026-10-28** — Step 1 done: `EntityStore::delete(ctx, id, removal)` sets the mark it is given,
  refused if it names another than the caller and the caller is not the system (`Access::delete`,
  asked after the permission). A session's, a channel's and a project's deletion make one
  `Removal` and give it to all that goes with it - `ScopeMember::delete_lying_in`,
  `ScopedEntities::delete_lying_in`, `GroupManager::delete_participants`, the session's own
  project, and a project's sessions through `SessionManager::delete_with` (`pub(crate)`, beside the
  public `delete`, which makes the mark). The groups' `Removal::by` now names whoever deleted the
  session or channel. Restoring still compares times; the roll-backs of a restoring still mark
  anew - both are step 2. `delete_kept` rolls its groups back from the session's time, which they
  now carry.
