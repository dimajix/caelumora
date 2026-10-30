# 497 — The purge of what has something behind it is written four times

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** Begin the purge, remove what lies behind, purge the document: the sequence is written in `WorkspaceManager::purge`, `ArtifactManager::purge` and `AgentManager::purge`, and once more as `scoped::purge_behind`, which alone has a name.

## The problem

Issue [492](492-a-document-is-written-under-two-locks.md) gave every purge of an entity with
something behind it the same three steps: `EntityStore::begin_purge`, what lies behind removed,
`EntityStore::purge`. They are written four times: in the `purge` of the managers of workspaces,
artifacts and agents, in the caller's name and with each manager's own errors (`StorageGone`,
`TerminalError`, `Content`), and in `scoped::purge_behind`, in the runtime's name, through
`ScopeMember::remove_behind`, which answers a `StoreError` - a failure to remove a place or a
content as `StoreError::Backend`.

## Why it matters

A rule that is written four times is changed four times, and the next kind with something behind
it copies it a fifth. The order of the steps is the whole point (issue 492); a copy that gets it
wrong passes every test that does not provoke a failure between them.

## Considerations

- **`purge_behind` takes the caller's context, the managers call it.** One place for the order.
  The managers' typed errors then come through `remove_behind` as `StoreError::Backend` - what a
  caller of the REST API is told changes from a storage that is gone to an internal error.
- **Generic over the error.** `purge_behind` given what removes the rest as a closure answering
  the manager's error: one place for the order, the errors kept; a signature with two type
  parameters for three callers.
- **Leave it, and name the difference.** The manager's own purge says why it does not call
  `purge_behind`: its errors are its own.

## Done when

- The order of a purge with something behind it is written once, or its copies say why they are
  copies.

## History

- **2026-10-29** — Opened from the review of the third step of issue
  [492](492-a-document-is-written-under-two-locks.md).
- **2026-10-30** — Done by the second consideration: the order is written once, in
  `caelumora_model::store::purge_behind`, given the store, the context and what removes the rest
  as a closure answering the caller's error (as `written_as_next` takes what writes). The managers
  of workspaces, artifacts and agents purge through it in their caller's name with their own
  errors - `StorageGone` still reaches the REST API as itself -, `ScopedKind::purge_lying_in` and
  `ScopeHolders::keep_made` in the runtime's name with `ScopeMember::remove_behind`; the runtime's
  own `scoped::purge_behind` is gone. The agents' purge, which alone logged only what it purged,
  set the pattern: the other managers' purges - of workspaces, artifacts, skills, groups and
  projects - now log only a purge that removed something too.
