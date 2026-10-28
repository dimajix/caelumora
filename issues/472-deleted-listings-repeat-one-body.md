# 472 — Every listing of what was deleted repeats one body

**Status:** in progress
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

## Decision

A, with the finding of issue 469 recorded below, in two steps:

1. **The store hands out what was deleted with its mark.** `get_deleted` answers
   `Deleted<T> { entity, removal }`, `list_deleted` a `Page<Deleted<T>>` - a type, not a pair: it
   is the store's public interface, crosses crates and stands in a `Page`. The store makes it in one
   place; no caller checks for the mark. No `Deref`: what is deleted is reached as such.
2. **One function makes the entries.** `Views::deleted_of(store, query, name)`, which answers
   `StoreResult`, so that each caller keeps its own error; the name of an entry is given by a
   closure, not by a method of `Stored` that every kind would implement for seven callers. The
   deleted channels take it too, and narrow what it answers by their reason: restoring one changes
   the session, and none is purged alone.

## Done when

- The deleted entries of every kind are made in one place.
- No caller of `get_deleted` or `list_deleted` checks for the mark.

## History

- **2026-10-28** — Opened at the maintainer's request from the review of step 2c of issue 467.
- **2026-10-28** — A finding of issue 469, added at the maintainer's request: every one of these
  listings, and every restoring that reads a deleted entity, checks for a mark the store already
  guarantees - `let Some(removal) = x.deleted.clone() else { continue }` in the seven listings
  (`view/mod.rs:479`, `:645`, `:668`, `:692`, `:716`, `view/session.rs:389`, the plugins' views),
  and a let-else answering "not found" in `SessionManager::restore`, `SessionLoop::restore_channel`
  and `ProjectManager::restore` -, since `get_deleted` and `list_deleted` hand out the entity
  alone, its mark an `Option`, though they find only what is marked. Proposed with this issue:
  both answer the entity with its mark (`Deleted<T> { entity, removal }`, or a pair), so that no
  caller checks for it and the one function of option A takes the mark from there. A change of
  the reading interface of `EntityStore`, with its callers.
- **2026-10-28** — Taken up; the plan above agreed with the maintainer, work begun on branch
  `issue-472-deleted-listings`.
- **2026-10-28** — Step 1 done: `get_deleted` answers `Deleted<T> { entity, removal }`,
  `list_deleted` a `Page<Deleted<T>>`; the store makes it in one place (`Deleted::of`,
  `CheckedStore`). The let-else of the three restorings and the `else { continue }` of the seven
  listings are gone; the restorings, purges and listings take the entity and the mark apart where
  they read them. `scoped::deleted_with` answers the entities that carry a mark.
