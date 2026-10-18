# 140 — There is no audit log of security-relevant acts

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Audit
**Summary:** Acts that change who may see what - releases, participants, approvals, deletions - leave no record of who did them and when.

## The problem

Acts that change who may see what leave no record of who did them and when. With step 4 of
[133](133-classification-model.md), an authorised actor can release a channel or a message: lower
its level, or widen its audience. The new classification is stored with the entity, but not what it
was before, who changed it, or when. The same holds for other acts of this kind: adding or removing
a participant of a channel, approving a tool call, deleting a session
([07](07-session-deletion-audit.md)).

## Why it matters

- A release is a deliberate declassification. In an organisation that classifies its information,
  "who released this to finance, and when" is a question that must have an answer - and it cannot be
  reconstructed afterwards.
- The envelope stream ([05](05-postgres-event-recorder.md)) records what was said, not acts on
  entities outside of it; and it is not recorded either.

## Considerations

- **One log, not one per entity.** A release protocol in `ChannelState` was considered for step 4 of
  133 and set aside: it would be the first of several ad-hoc protocols. An audit log is one place -
  a store of its own, written by the runtime for every act of a listed kind, with actor, target,
  time and the change - and each act only names what it records.
- **What is logged, not what is said.** Entries carry ids, kinds and classifications, never
  content - the same line as for logging ([01](01-improve-logging.md)), so that the audit log does
  not become a copy of what it is about.
- **Who may read it** is a question of its own, and likely an administrator's
  ([125](125-invisible-admin.md)).
- Notes and memory entries are chains of changes: a reclassification there is recorded as a change
  of the chain anyway. The log would still be where the act, and its actor, is found.

## Done when

- Security-relevant acts - at least releases, changes of participants, approvals and deletions - are
  recorded with actor, target, time and the change, in one audit log, and who may read it is
  decided.

## History

- **2026-09-30** — Opened while deciding step 4 of [133](133-classification-model.md): the
  maintainer set a release protocol in `ChannelState` aside, since what it would be is an entry in
  an audit log, which does not exist yet.
- **2026-09-30** — [145](145-ended-turns-leave-a-record.md): an agent removed from its channel, or
  whose channel is deleted, gives up a turn under way; the turn is marked `Stopped` with the time,
  but not with who stopped it. The maintainer decided that `Actor::stop`, like `cancel` and
  `wakeup`, knows no caller: who removed the agent or deleted the channel is recorded with that act,
  here, and found for a stopped turn by channel, agent and time. Until then it is only in the log
  (`remove_participant`, `delete_channel`).
- **2026-10-05** — From [164](164-soft-deletion-and-former-members.md): deleting becomes marking,
  and the mark (`Removal`: who, when) is kept in the document - of a deleted session, channel or
  agent, and of a participant that left a channel. That is the entity's state, not this log: it is
  gone when the entity is restored, a participant added again or the entity purged. The acts to
  record here are therefore: delete, restore, purge (by a person or by the retention policy of
  [07](07-session-deletion-audit.md)), and a participant removed or added again. The purge depends
  on this log most: it leaves nothing else that says what was there.
- **2026-10-17** — Decided with [417](417-user-history-from-its-actor.md): the channel's history
  is not the basis of the audit, this log is. The history records the conversation - what entered a
  channel and whom it went to, with content, for its participants -, and is one source of this
  log; an audit is wider than channels and read by others. Approvals are therefore not entries of
  the history: the act is recorded here, and who approved a call is kept at the call for the
  readers of the message ([419](419-approval-recorded-with-the-call.md)). What
  [05](05-postgres-event-recorder.md) wanted of a channel is the history now.
- **2026-10-18** — As it stands after [417](417-user-history-from-its-actor.md): of the acts
  listed here, the channel's history now records, for a channel and with its time, who joined and
  left by whose act (`ParticipantJoined`, `ParticipantLeft`, the latter with where delivery to the
  leaver stopped), what was shared of the history with a newcomer and by whom (`HistoryShared`),
  and a message reclassified - released or raised - with who sent the change
  (`Recorded::sender`). It does not record who reclassified a channel (`Fitted` has no actor),
  approvals ([419](419-approval-recorded-with-the-call.md)), deletions, restores and purges, nor
  anything outside a channel; and it is read by a channel's readers, with content. So this log
  stays what it was decided to be: one store of acts without content, across channels, for its own
  readers - the history one of its sources.
