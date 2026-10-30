# 498 — The mark of a participant who left says whether its purge has begun

**Status:** done
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
- **2026-10-30** — Decided for neither option: deleting and purging are two acts, each with a
  permission of its own. `Removal` is the mark of a deletion alone - which, who, when - and no
  longer carries `purging`; whether an entity's purge has begun is the entity's, apart from its
  mark: `Stored::purging`, set by `Stored::set_purging` in `begin_purge` alone, as `revision` is
  the store's. Nothing is created or changed with its purge begun (`Validation`). A mark of its
  own for a deleted entity, `DeletionMark` holding the `Removal` and `purging`, was built first and
  dropped: it bound the purge to the deletion again, where a purge without a deletion before it is
  conceivable. That `Stored::set_purging` is public, as `set_revision` is, was accepted: the store
  refuses what it did not set itself. Stored data: an entity whose purge was begun and not
  finished loses that record; finish its purge before, or delete it. Done.
