# 494 — A session put into a project while the project is deleted stays there

**Status:** open
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
