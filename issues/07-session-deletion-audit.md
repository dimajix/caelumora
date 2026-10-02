# 07 — Session deletion and its audit trail

**Status:** in progress
**Area:** Sessions & channels

## The problem

Deleting a session is implemented: `SessionManager::delete_session`
(`caelumora-runtime/src/session.rs:222`) stops the running loop, removes the persisted session and
emits `SessionStopped` followed by `SessionDeleted`; `DELETE /sessions/{id}` exposes it.

What is not settled is the question the original TODO entry attached to it: *deleting sessions =>
audit?* A deletion currently removes the session and its state outright. Nothing records that it
existed, who deleted it, or what it contained.

## Why it matters

Deletion is the one operation that destroys evidence. If the system is ever used where an answer to
"who removed this conversation, and when" is expected, that answer has to be produced at deletion
time — it cannot be reconstructed afterwards.

There is a competing requirement: deletion is also how a user removes content they do not want kept.
An audit trail that preserves the content defeats that.

## Considerations

- Separating *tombstone* from *content* resolves most of the tension: keep who deleted what and
  when; do not keep the messages. The `GlobalEvent` stream already emits `SessionDeleted`, so the
  tombstone largely exists — it just is not persisted anywhere
  ([05](05-postgres-event-recorder.md)).
- Soft deletion (mark, don't remove) is the other option and is cheaper to implement, but it turns
  every store query into a filtered query and leaves the content in place, which is the opposite of
  what a deletion request usually means.
- Cascade behaviour deserves a note: deleting a session removes its channels and agent state.
  Whether agent-global knowledge outlives the session is a question for [19](19-memory-tiers.md).

## Done when

- The retention policy for deleted sessions is decided and written down here.
- Whatever that policy prescribes is enforced by `delete_session`.

## History

- **2026-09-10** — Issue created from `TODO.md`. Deletion itself had already landed in `540e482`;
  the audit question is what remains open, hence *in progress* rather than *open*.
