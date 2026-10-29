# 498 — The mark of a participant who left says whether its purge has begun

**Status:** open
**Type:** question
**Priority:** low
**Area:** Persistence
**Summary:** `Removal` marks a deleted entity and a participant who left a channel alike, and since issue 492 carries `purging`, which means something for the one and nothing for the other.

## The problem

`Removal` is the mark of a deletion - which deletion, by whom, when - and is used twice: as the
mark of a deleted entity (`Stored::deleted`) and as when and by whom a participant left a channel
(`FormerParticipant::left`). Issue [492](492-a-document-is-written-under-two-locks.md) added
`purging` to it, whether the entity's purge has begun (`EntityStore::begin_purge`). A participant
is never purged: for them it is always `false`, a field that can hold a state that never occurs.

## Why it matters

"States are types" (CODING-GUIDELINES.md): a field that makes sense only in one of a type's uses
is the sign of two concepts under one name. Today nothing reads `purging` of a participant; the
next change of the mark of a deletion, though, has to ask each time whether it is about a deleted
entity or a participant.

## Considerations

- **A mark of its own for a deleted entity.** `Stored::deleted` answers a type that holds the
  `Removal` and whether its purge has begun; the participant keeps the `Removal` alone. Costs:
  every kind's field changes its type, and so do the stores' and the views' readers of the mark,
  and stored data has to be deleted.
- **Leave it.** One small field, documented as the entity's; the participant's is always false.

## Done when

- Decided, and if decided for a type of its own, built.

## History

- **2026-10-29** — Opened from the review of the third step of issue
  [492](492-a-document-is-written-under-two-locks.md), which put `purging` on `Removal` as agreed.
