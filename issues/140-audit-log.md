# 140 — There is no audit log of security-relevant acts

**Status:** open
**Area:** Identity and access management

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
