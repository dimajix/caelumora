# 511 — PERMISSION-CHECKS.md records a question the store asks twice in some rows and not in others

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Identity and access management
**Summary:** The column *Again* of the tables in `docs/PERMISSION-CHECKS.md` names `ReadDeleted` asked twice by the store in some rows - the restoring of a session, the purge of a project - and leaves it out in others where the code asks it as often - the restoring of a project, the purge of a session or a workspace -, so the column cannot be read as the list of what is asked again.

## The problem

`docs/PERMISSION-CHECKS.md` gives every operation the questions in order and, in the column
*Again*, where the same question is asked again on the way. Where a manager reads the deleted
entity first (`get_deleted`) and the store asks again as it restores or purges, or where
`store::purge_behind` asks once to begin the purge and once to purge, the rows differ:

- *Restore* of a session: `ReadDeleted` (both store).
- *Purge* of a project, since issue
  [496](496-a-sessions-purge-removes-its-channels-documents-first.md): `ReadDeleted` (both store).
- *Restore* of a project, *Purge* of a session, *Purge* of a workspace, *Restore, purge an agent*:
  `-`, though the store asks `ReadDeleted` twice there as well.

## Why it matters

The column is how a reader finds questions asked twice - the ones that cost a round trip to the
policy, and the ones whose answers could differ between the two. A row that leaves one out says it
is asked once.

## Considerations

- **Name every repetition.** Every row where the store asks a question twice says so, as the
  session's restoring does. The column grows by the same phrase in a few rows.
- **Name only what a manager asks again.** The column leaves out what the store asks twice within
  one operation of its own (`purge_behind`, a write read again after `Outdated`), and says so in
  the introduction; the rows that name it now drop it.

## Done when

- The introduction of `docs/PERMISSION-CHECKS.md` says which repetitions the column records, and
  every row follows it.
- The column *Refusal* names `NotFound` only where an operation refuses with it, not where it
  answers `false`.

## History

- **2026-10-30** — Opened from the findings of issue
  [496](496-a-sessions-purge-removes-its-channels-documents-first.md), whose review found the two
  readings side by side.
- **2026-10-30** — A second inconsistency of the same table, found in the review of issue
  [510](510-a-sessions-purge-fails-where-the-others-answer-false.md): the column *Refusal* names
  `NotFound` for the deletion of an agent and of a project, though both answer `false` where there
  is nothing the caller could see, and refuse with no `NotFound`; the rows of a session's deletion
  and of every purge name it no longer. To be put right with the column *Again*.
