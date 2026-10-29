# 495 — A lock per id is taken out of its table while it is held, and kept for ids never purged

**Status:** open
**Type:** bug
**Priority:** medium
**Area:** Sessions and channels
**Summary:** `SessionManager::delete` takes a session's lock out of its table while it holds it, so that a caller who comes afterwards is given a new lock while another still waits on the old one, and both tables of locks by id keep an entry for every id ever asked.

## The problem

The `SessionManager` keeps one lock per session id (`starts`, `start_lock`), held while a session
is started, stopped, deleted, restored or purged, so that none of them meets another half done.
The `ProjectManager` keeps one per project id the same way since issue
[492](492-a-document-is-written-under-two-locks.md) (`lifecycles`, `lifecycle_lock`). Two things
are wrong with how the tables are kept:

- **An entry is removed while its lock is held.** `SessionManager::delete_with` takes the lock and
  removes the entry at its end (`self.starts.remove(&id)`), before the guard is dropped. A caller
  that was already waiting holds the old `Arc` and goes on with the old mutex; a caller that
  arrives after the removal is given a new one. The two then run at once - a restoring and a
  start of the same session, say -, which is what the lock is there to prevent. `purge` removes
  the entry the same way; there the id is gone for good, and a caller after it finds nothing.
- **Every id asked keeps an entry.** The lock is taken before anything is read, so an id that does
  not exist, or is not the caller's, gets an entry as well. The `ProjectManager` removes an entry
  only after a purge, so a project deleted and never purged, or an id asked by mistake, keeps one
  for as long as the process runs.

## Why it matters

The first undoes the lock's purpose in a window that a request at the wrong moment hits: two
lifecycle operations of one session at once leave its channels, its groups or its scope half done.
The second is a slow growth of memory, by every id ever asked.

## Considerations

- **Remove an entry only when nobody else holds it.** `DashMap::remove_if` with
  `Arc::strong_count(lock) == 1` under the map's shard lock, after the guard is dropped: a waiter
  keeps the entry, the last one out removes it. Ends both problems; the cost is that the removal
  moves out of the operations into one place that drops the guard - which suggests the next.
- **One type for a table of locks by id.** Both managers use the same pattern; a small type that
  hands out a guard and takes the entry out when the last guard of an id is dropped would hold the
  rule once. A new type, but with a responsibility of its own, and two users today.
- **Striped locks.** A fixed number of locks, chosen by the id's hash: nothing grows and nothing
  is removed; unrelated ids may share a lock and wait for each other, never run at once. Simplest,
  at the price of a wait that has no reason.

## Done when

- No two callers hold the lock of one id at once, however the table is kept.
- The tables do not grow with ids that are not deleted, restored or purged any more.

## History

- **2026-10-29** — Opened from the findings of issue
  [492](492-a-document-is-written-under-two-locks.md), whose third step gave the `ProjectManager`
  a lock per project id after the `SessionManager`'s and kept the entry until a purge for this
  reason.
