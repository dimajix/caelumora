# 451 — A page of a history judges what its reader sees only after reading it

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Persistence
**Summary:** Whether a reader is given an item of a history is judged in Rust after the store read it, so a page of a reader who sees little of a long history reads it far back, also in Postgres.

## The problem

A page of a history (`HistoryStore::page`) holds the items its reader may see (issue 417). Every
store reads the items backwards and asks the model of each one whether the reader is given it
(`seen`, over `Recorded::seen_by`), until the page is full. The Postgres store of issue
[04](04-postgres-state-store.md) does the same: it puts the filter of the page into its query, as far
as SQL can judge it (`Condition`, issue 421), but not who reads - it reads the items in batches and
judges each in Rust.

For the channel itself and for a participant who sees most of what is said, that costs nothing. For
a reader who sees little - an outsider who is given only what went to every user, a participant
who joined late or for a short while, an approver -, a page reads back over everything they are not
given: a long history is read nearly whole for a page of a few items.

`seen_by` was written as one predicate over what a store keeps of an item - its sender, its
recipients, whether anything of it is published, its position - and the reader's ranges, so that a
database answers it as a `WHERE` (`docs/STORAGE.md`); only no store does yet.

## Considerations

- **Who owns the rule.** The rule is the model's and has to stay one: translating `seen_by` into
  SQL in the Postgres store would be a second copy, which drifts. Better if the model can say it as
  data - a `Filter<HistoryField>` of the reader, as `Filter::readable` does for a reader from
  another channel -, which every store judges (`Filter::matches`) or translates (`Condition`).
- **What the filter cannot say yet.** A participant is given what went to every member of their kind
  while they were one: a condition on the item's position against the reader's ranges. The position
  is not a field of the item as recorded, and `Filter` has no comparison of ranges; it would need
  one, or the position as a field of what is judged.
- **What is published** (`HistoryEntry::is_published`) is computed from the message - whether
  anything of it is left to show. A store that translates it needs it as data of the item, or as a
  condition over its fields.
- **The same question for entities** is issue [332](332-policy-as-a-predicate.md): the policy decides
  entity by entity, so no database can put who may read into its query. Whatever form the predicate
  takes should be the same for both.

## History

- **2026-10-22** — Issue created while finishing [04](04-postgres-state-store.md), whose
  Postgres store reads a history's items in batches and judges in Rust what each reader is given.
