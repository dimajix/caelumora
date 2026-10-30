# 511 — PERMISSION-CHECKS.md records a question the store asks twice in some rows and not in others

**Status:** done
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
- **2026-10-30** — Decided for the first option and done. The notation of the map says what the
  column *Again* names: where a question of the operation is asked once more on the way, *(pair)*
  for the operation's own and the store's, *(store)* for the store asking what was asked before - by
  the operation, a reach or the store -, anything else in words, in another context or by another
  act too; a write made again after `Outdated` counts once. *(both store)* went into *(store)*,
  which said the same. Going through every row showed more than the issue named: the store asks
  `Get…` - and `cleared_for` for what is classified - before every change and deletion, so the
  deletion of a session, an agent, a project, a skill and a group asked a read twice unnoticed, and
  the channel's deletion named its read as a pair, without `cleared_for`; the purges through
  `store::purge_behind` asked `ReadDeleted` + `Purge` twice; attaching a workspace, taking someone
  into a channel, setting a project's members and restoring and purging a project asked more than
  they said. The table of the groups has the column now. *Refusal* names `NotFound` no longer for
  the deletion of an agent and of a project. Joining is no pair: it stores in S, and asks
  `JoinSession`, and with a channel `JoinChannel`, a personality and the clearance, twice by itself.
  Where a participant is taken in or joins as a founder, its personality and clearance are asked by
  the session and again by the channel. Creating a session asks no personality since issue 420,
  which makes its channels empty, so its row and opening's no longer say so. The finding on pairs
  names sharing and purging no longer, which no manager asks itself.
