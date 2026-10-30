# 494 — A session put into a project while the project is deleted stays there

**Status:** rejected
**Type:** bug
**Priority:** low
**Area:** Sessions and channels
**Summary:** `SessionLoop::update_project` reads the project it puts the session into and stores the session afterwards, with nothing against the project being deleted in between: the session then lies in a deleted project, not deleted with it and not restored with it.

## The problem

Issue [458](458-deleted-while-it-runs.md) closed the window between checking the holder of a
scope and writing into it: whoever makes or restores an entity in a session's, channel's or
project's scope asks `ScopeHolders` before and after, and what was written after the holder's
deletion went through the scope is taken back. A session in a shared project has the same shape -
`ProjectManager::delete` marks the project, then deletes the sessions that lie in it
(`delete_sessions`), each as `SessionManager::delete_with` - but is not covered: a session is no
`ScopeMember`, and it is put into a project by `SessionLoop::update_project`
(`caelumora-runtime/src/session/settings.rs`), which reads the project in the caller's name
(`reached_project`), shares it with the participants, and stores the session as lying there. Read
before the mark and stored after `delete_sessions` listed them, the session stays in the project:
live, in a project that is deleted, and listed by nothing that restores or purges the project.

`ProjectManager::delete` holds its `writing` lock throughout; `update_project` holds the session's
`moving` lock. Neither sees the other.

## Why it matters

Rare - the window is one store write - and silent: the session runs on, its results go to a
project nobody reaches, and purging the project leaves the session behind. The same reasoning that
decided 458 applies, and the fix is the same shape, so it should look the same.

## Considerations

- **Ask before and after, as 458 does.** `update_project` asks `ScopeHolders::require` for the
  project's scope before (in place of, or beside, `reached_project`), stores the session, and asks
  again in the runtime's name: gone, it stores the session back into the project it came from and
  takes back what it did for the target (`take_back_entering`). The taking back is the session's
  own, not a `ScopeMember`'s - storing the session again, unsharing -, so `keep_made` does not
  serve; what is shared is the question, `ScopeHolders::require`.
- **A session as a `ScopeMember` of the project's scope.** Would let `delete_lying_in` and
  `keep_made` cover it, but a session is deleted by its manager, which stops it and takes its
  channels, groups and own project with it; `ProjectManager::delete_sessions` exists for that
  reason. Making it a member would duplicate that or hollow it out.
- **Leave it.** The window is small; but 458 promised that nothing is left in a deleted scope,
  and this is the one way left.

## Done when

- A session stored as lying in a project after the project's deletion went through its sessions
  is taken back out of it, and its caller told the project is gone.
- `docs/PROJECTS.md` or `docs/CHANNELS.md` says so where they describe putting a session into a
  project.

## History

- **2026-10-29** — Opened while deciding issue [458](458-deleted-while-it-runs.md), which left it
  out as the one writer into a scope that is no `ScopeMember`.
- **2026-10-30** — Assessed; the priority stays low. The window is narrower than described above:
  `ProjectController::share` reads the project from the store (`reached`), so a project already
  marked refuses the sharing, and the session is left behind only if the mark falls between the
  sharing and storing the session, and the listing in `ProjectManager::delete` before the storing.
  Nor do its results go silently to a project nobody reaches: since 458, what the session puts
  into the project's scope is refused by `ScopeHolders::require` (`NotFound`). Restoring the
  project leaves things consistent again - the session was never deleted, and lies in it once
  more -; purging it leaves the session pointing to a project that no longer exists, from which
  it can still be moved away. One thing more is left behind, also by `take_back_entering` today:
  the entry of the session's participants on the project's access list, which `unshare` cannot
  take away from a deleted project and which comes back with it on restoring.
- **2026-10-30** — Rejected: accepted as it is. Every way of taking the move back writes again
  into a project that may be deleted meanwhile - back into the one it came from, or the sharing
  with the participants, which cannot be taken away from a deleted project -, and whichever comes
  first, storing the session or sharing, leaves its own gap; closing them would take holding the
  projects' lifecycle locks across both managers. Without transactions over several entities this
  is working around them, at a cost out of proportion to a case that is rare, shows itself - what
  the session puts into the project is refused - and is mended by moving the session away.
