# 07 — Session deletion and its audit trail

**Status:** in progress
**Type:** feature
**Priority:** low
**Area:** Audit
**Summary:** Deletion works since `540e482`; what remains open is whether and how a deletion is recorded.

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
- **2026-10-05** — The retention policy decided by the maintainer, while planning
  [164](164-soft-deletion-and-former-members.md): **a user's delete is soft, a hard delete is the
  administrator's.** Deleting a session marks it and keeps its content; it is removed for good by a
  purge, which an administrator runs ([125](125-invisible-admin.md)) or a retention policy he
  configured. That answers the tension above between evidence and removal by rule rather than at
  the moment of deleting. Marking, restoring and the purge as an operation
  (`SessionManager::purge_session`) are 164's steps; what remains here, after them:
  - **The retention policy.** A section `retention` in `AppConfig` with `purge_deleted_after:
    Option<Duration>` - `None` keeps what was deleted until someone purges it, zero makes a user's
    delete final at once, for a deployment where deletion must mean removal. Possibly per kind
    (sessions, channels, agents) later; one value first. CONFIGURATION.md.
  - **The purge by it.** A background task of the runtime, started with the `SessionManager`, that
    from time to time lists what is marked (`list_deleted_…` as the system) and purges what was
    deleted longer ago than configured (`Removal::at`), sessions with their channels and state. It
    logs every purge and, once [140](140-audit-log.md) exists, records it there - a purge leaves
    nothing else behind.
  - **Removing one entry of a chain** - a single message of a history or transcript - is not a
    purge: the chains' digests cover every entry, and contexts refer to them
    ([06](06-context-in-message-for-auditability.md)). It needs redaction: the content goes, the
    entry and its digest stay. Not planned; noted here for when single messages are to be deleted.
  - *Done when* then reads: the policy is this, `delete_session` marks (164), and the configured
    retention is enforced by the purge task.
- **2026-10-15** — Issue [125](125-invisible-admin.md) was rejected, the administrator role with
  it. No administrator runs a purge: the retention policy is the only way to remove a deleted
  session for good, in the system's name.
