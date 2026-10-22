# 450 — A store overwrites a change made meanwhile, and checks a change against what may be outdated

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Persistence
**Summary:** `CheckedStore` reads a document, asks the policy and writes in steps of their own, so a second process that changes the same entity in between is overwritten without notice, and the permission is judged on what was stored before it.

## The problem

Every change of an entity goes through `CheckedStore` (issue
[446](446-stores-check-access-each-and-are-enumerated.md)): `update` reads the stored document
(`DocumentStorage::get`), asks the policy whether the caller may change it into the new one
(`Access::change`), and writes the new one in its place (`DocumentStorage::replace`); `remove`,
`delete`, `restore` and `purge` read and write the same way. The three steps are calls of their
own, and nothing between them holds.

In one process that is enough: every document has exactly one writer, which changes it under a lock
(the controller of a `LiveDocument`, the manager of its kind, see
[ARCHITECTURE.md](../docs/ARCHITECTURE.md)). With a store that several processes share - Postgres,
[03](03-postgres-entity-store.md) - each process has a writer of its own:

- **A change made meanwhile is lost.** Two processes read the same version and write; the second
  write replaces the first, and neither learns of it.
- **A permission is judged on what may be outdated.** The policy is asked about the document as it
  was read; if another process changed its owner or its grants since, the change is allowed or
  refused by the attributes it no longer has.

## Why it matters

A shared store is what [03](03-postgres-entity-store.md) is for, and several instances of the
server ([122](122-several-instances.md)) need it. 122 names versions among its considerations -
a store that refuses a write made on an outdated version, events that carry the version they result
in -; this is the store's part of that, which the rest builds on.

## Considerations

- **A version in every document, and a write against it.** The store hands the storage the version
  it read with the document it writes; the storage writes only if that is still the version kept -
  in Postgres `UPDATE … WHERE id = $1 AND version = $2`, in memory and in the JSON files under the
  lock they write by - and refuses it otherwise, as a conflict. The permission was judged on that
  version, so it holds for what is replaced. Where the version is kept - a column beside the body,
  a field of the document - and what a caller does on a conflict (read again and retry, or hand it
  up) is to be decided; a controller that holds its document live would read it again and apply its
  change once more.
- **A transaction around the three steps** - `SELECT … FOR UPDATE`, ask, write - would hold a lock
  of the database while the policy is asked, which may itself read the stores (the members of a
  group, issue 23), and the seam would need a transaction that every storage offers.
- **Events** would carry the version (122), so that a controller skips an announcement older than
  what it holds.

## Done when

- A write of an entity made on an outdated version is refused, in every storage, and the store
  suite holds it.
- What a caller does on such a refusal is decided and done where it matters.

## History

- **2026-10-22** — Opened from the discussion of [03](03-postgres-entity-store.md) with the
  maintainer: not solved in 03, which brings the first store shared by several processes.
