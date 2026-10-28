# 472 — Every listing of what was deleted repeats one body

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** The views list what was deleted of each kind with a body of their own each, which differ only in the store, the query and the kind.

## The problem

`Views::deleted_agents`, `deleted_groups`, `deleted_workspaces`, `deleted_projects` and
`deleted_sessions` (`caelumora-runtime/src/view/mod.rs:473`, `:639`, `:662`, `:686`, `:710`), and
the plugins' `deleted_artifacts` (`caelumora-artifacts/src/view.rs:89`) and `deleted_skills`, each
list the deleted of their kind in the viewer's name and make each into a
`DeletedSnapshot { id, name, deleted, can_restore, can_purge }`, asking `may_restore` and
`may_purge` with the kind. Issue [467](467-projects-hold-what-outlasts-a-session.md) added the
fifth of the core's.

## Why it matters

What a deleted entry shows, and what is asked for it, is decided in seven places; a change of one -
a field more, another question - has to be made in all of them, and the next kind adds an eighth.

## Considerations

- **A. One generic function** on `Views`, `deleted_of<T: Stored + Secured>(store, query, what)`,
  built from `Stored::id`, `Stored::deleted`, `T::KIND` and the name (`Scoped::scoped_name`, or a
  small trait for kinds without one, as a session); each method becomes one line, and the plugins
  call it too.
- **B. Leave it**, since each is short.

Recommendation: A.

## Done when

- The deleted entries of every kind are made in one place.

## History

- **2026-10-28** — Opened at the maintainer's request from the review of step 2c of issue 467.
