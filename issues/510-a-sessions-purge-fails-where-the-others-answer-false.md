# 510 — A session's purge fails where every other purge answers `false`

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** `SessionManager::purge` answers a session that is not deleted, or not the caller's to see, with `SessionError::NotFound`, while the purges of agents, workspaces, groups, projects, skills and artifacts answer the same case with `Ok(false)`, so that their callers and the REST handlers tell it apart in two ways.

## The problem

Every manager purges through `store::purge_behind` or the store's `purge`, which answer `false`
when there was nothing deleted the caller could see. `AgentManager::purge`,
`WorkspaceManager::purge`, `GroupManager::purge`, `ProjectManager::purge`, `SkillManager::purge`
and `ArtifactManager::purge` pass that on as `bool`, and their REST handlers turn `false` into a
404 of their own (`routes/agents.rs`, `routes/projects.rs`, ...). `SessionManager::purge` returns
`Result<()>` and turns `false` into `SessionError::NotFound`, which its handler
(`routes/sessions.rs`, `purge_session`) passes on through the error's mapping. Since issue
[496](496-a-sessions-purge-removes-its-channels-documents-first.md), the session's purge goes
through the same `purge_behind` and turns its answer into an error afterwards.

## Why it matters

Similar things are to fail the same way. A caller of several purges - `ProjectManager::purge`,
which purges the sessions in a project, a retention policy later - handles one kind's "nothing
there" as an answer and the session's as an error, and a new kind has two examples to follow.

## Considerations

- **All answer `bool`.** `SessionManager::purge` returns `Result<bool>`, as the others; its handler
  answers `false` with a 404 as the others do. The smallest change, and the one the store's own
  answer suggests.
- **All fail with `NotFound`.** Each manager turns `false` into its own `NotFound`, and the handlers
  lose their `match`. Six signatures and handlers change instead of one, and a caller that purges
  what may be gone meanwhile has to tell an error apart.
- **Leave it.** The difference is one signature; but the session's purge is the one a project's
  purge calls.

## Done when

- Every manager's purge answers "nothing deleted the caller could see" the same way, and its REST
  handler maps it the same way.

## History

- **2026-10-30** — Opened from the findings of issue
  [496](496-a-sessions-purge-removes-its-channels-documents-first.md), whose review noticed the
  difference once both purges went through `purge_behind`.
