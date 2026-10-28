# 469 — A restoring that fails marks anew, and a second one leaves behind what was deleted with it

**Status:** open
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

Recommendation: A - a roll-back that sets back what was, as the coding guidelines ask ("set back to
how it was before"), in one place for every holder.

## Done when

- A restoring that fails, retried, brings back everything that was deleted with the holder.
- `SessionManager::restore`, `SessionLoop::restore_channel` and `ProjectManager::restore` roll
  back without a new mark; the work-around for a session's own project is gone.
- The tests retry a failed restoring and find what went with the holder.

## History

- **2026-10-28** — Opened at the maintainer's request from the findings of issue 467, where the
  test of steps 2a showed it for the groups of a session and the review of step 2b for a project.
