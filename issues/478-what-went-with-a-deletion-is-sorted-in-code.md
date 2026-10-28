# 478 — What went with a deletion is told apart in code, after every deleted entity was read

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Persistence
**Summary:** Restoring what went with a holder lists everything deleted in its scope and keeps in code what carries the holder's mark (`scoped::deleted_with`), since no query can ask for a deletion's mark.

## The problem

Since issue [469](469-a-failed-restore-renews-the-mark.md), what was deleted with a holder is what
carries its `Removal`, and restoring takes back exactly that. Every `ScopeMember`, the
`GroupManager`, the `SessionManager` (a session's own project) and the `ProjectManager` (a
project's sessions) list all that is deleted in the holder's scope or project, and keep what
carries the mark by `scoped::deleted_with` (`caelumora-runtime/src/scoped.rs:280`), comparing it in
code. STORAGE.md names it among what a query cannot say yet.

A query cannot say it, because:

- **The mark is no field.** Each kind has a field type of its own, and the mark is in none of
  them; some kinds have no fields at all (`NoField`). A storage knows of the mark only that a
  document is deleted (`Document::deleted`, a flag), which `Presence` reads.
- **A mark is compared by its time.** `Removal` is `by` and `at`. `FieldValue` has no time, and
  comparing `at` as text relies on the exact form serde writes. A backend that keeps it as a time,
  as a `timestamptz` of Postgres in microseconds, would not even keep it equal to chrono's
  nanoseconds.

## Why it matters

- Restoring reads every deleted entity of a kind in the holder's scope, also those deleted on their
  own, long before, and those of earlier deletions of the holder - in a project with a long
  history, all of them, for every kind, at every restoring.
- "A condition a kind offers for lists is a constructor of `Filter`" (CODING-GUIDELINES.md) is
  broken in one place more, for a condition every kind shares.

## Considerations

- **A. A deletion has an id.** `Removal` gets one (`RemovalId`, a `Uuid`), made once with the mark,
  so that a mark is compared by it - a `FieldValue::Uuid`, which every backend compares already -,
  not by its time. A prerequisite of B and C; alone, it only makes the comparison in code cheaper
  and independent of how a time is kept.
- **B. The mark is a field every kind shares**, as its scope is (`ScopedField`): a trait with the
  path of the mark's id, and `Filter::deleted_with(&Removal)` beside `Filter::lying_within`.
  Every kind needs a field type then, also those with none today.
- **C. The storage keeps the mark**, as it keeps that a document is deleted: `Document` carries the
  deletion's id instead of a flag, and listing the deleted can be narrowed to one deletion, beside
  `Presence`. The mark is no business of a kind's fields, as being deleted is none; the store's
  interface takes the narrowing (`list_deleted` with the mark, or a `Presence` that names it).
- **D. Leave it**, as STORAGE.md says: correct, and costly only where much was deleted.

Recommendation: A with C - the mark is a concept of the store, as presence is, and an id compares
the same in every backend. To be weighed when restoring a large project shows the cost, or when the
storage's columns are changed anyway.

## Done when

- A deletion's mark is compared by an id, not by a time.
- Restoring what went with a holder reads only what carries its mark; `scoped::deleted_with` is
  gone, and STORAGE.md no longer names it among what stays code.

## History

- **2026-10-28** — Opened at the maintainer's request from the findings of step 2 of issue 469,
  which made the mark what restoring compares.
