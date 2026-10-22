# 451 — A page of a history judges what its reader sees only after reading it

**Status:** in progress
**Type:** feature
**Priority:** low
**Area:** Persistence
**Summary:** Whether a reader is given an item of a history is judged in Rust after the store read it, so a page of a reader who sees little of a long history reads it far back, also in Postgres.

## The problem

A page of a history (`HistoryStore::page`) holds the items its reader may see (issue 417). Every
store reads the items backwards and asks the model of each one whether the reader is given it
(`seen`, over `Recorded::seen_by`), until the page is full. The Postgres store of issue
[04](04-postgres-state-store.md) does the same: it puts the filter of the page into its query, as
far as SQL can judge it (`Condition`, issue 421), but not who reads - it reads the items in batches
and judges each in Rust.

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
- **The same pattern as [332](332-policy-as-a-predicate.md), not the same rule.** There the
  configured `AccessPolicy` decides entity by entity who may read it, over its security
  attributes; it differs by policy and is to become a predicate the policy gives. Here the
  permission to read the history is checked before (`Enforcement::require`); `seen_by` is the
  model's one fixed rule of what a reader was given of it - addressed by name, a member then,
  published - and belongs to the history (417), not to a policy. What both need is the same: a rule
  as data that a store judges in memory or translates (`Filter`), so the form found for one should
  serve the other.

## Steps

1. **Comparisons in the filter.** `Filter::AtLeast` and `Filter::Below`, a field compared with a
   number, judged in memory and translated by the Postgres store.
2. **The model.** The form an item is judged in (`KeptItem`: its position, whether it is
   published, the item as recorded), the fields of the history for them, and the rule of `seen_by`
   as a filter of the reader (`Reader::seeing`), with a test that both decide alike.
3. **Postgres.** The items kept in that form, and the reader's filter in the query of a page.

## History

- **2026-10-22** — Issue created while finishing [04](04-postgres-state-store.md), whose
  Postgres store reads a history's items in batches and judges in Rust what each reader is given.
- **2026-10-22** — Decided with the maintainer: an item is judged in a form of its own, with its
  position and whether it is published - derived as it is written, as the item is -, so that both
  are fields a filter names; the filter compares a field with a number, a general extension; the
  model gives the rule as a filter of the reader, while `seen_by` stays the code that decides in
  memory, a test holding both together - a filter decided per item would serialize every item,
  and the nodes of a chain are no items.
