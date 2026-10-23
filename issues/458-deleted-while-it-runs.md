# 458 — A session or channel is deleted while it runs, and stopped only once it is gone

**Status:** open
**Type:** bug
**Priority:** low
**Area:** Sessions and channels
**Summary:** Deleting a session or a channel marks it, deletes what lies in its scope and the groups of its participants while it still runs, and stops it only afterwards, so what its agents write into its scope meanwhile is left behind, and what it announces as it stops reaches its owner alone.

## The problem

`SessionManager::delete` marks the session, deletes what lies in its scope and in its channels'
(`delete_lying_in`, issue [320](320-scoped-deletion-in-the-runtimes-name.md)), then the groups of
its participants (issue [348](348-participants-groups-out-of-step-with-their-holder.md)), and only
then stops it (`stop_running`, its actors leaving, issue
[377](377-stopping-a-session-aborts-its-actors.md)). `SessionLoop::delete_channel` does the same for
a channel. The order is deliberate: deleting can fail part of the way - something of the scope the
caller may not delete (`ScopeNotEmpty`), the groups not going -, the deletion is then rolled back,
and the session runs on if it ran, without what was deleted. Before 320 a session was stopped first.

While it is deleted, it runs:

- **What is written meanwhile is left behind.** An agent in one of its channels may write into its
  scope - an artifact it publishes, say - after `delete_lying_in` has gone through it. That stays,
  not deleted, in the scope of a deleted session, and does not come back with it as what was
  deleted with it does.
- **What it announces as it stops reaches its owner alone.** The participants read a session and
  its channels through the groups of the participants, which are gone by the time `session::STOPPED`
  and `channel::STOPPED` are published; a deleted group admits nobody but to tell of a deletion
  (issue [332](332-policy-as-a-predicate.md), `Verb::HearDeleted`). They hear of the deletion, which
  says as much, so this costs little on its own.

## Considerations

- **Stop first, then delete.** Nothing writes into the scope while it is deleted, and what it
  announces as it stops goes out before its groups go - though the bus asks the policy later, as it
  delivers, so that alone does not make the participants hear it. A deletion that fails leaves the
  session stopped: the turns under way are lost, and it is opened again as any stored session.
  Whether that is acceptable decides the issue; a failure is rare - the scope is asked about as far
  as the caller may delete it, the groups fail only with the store.
- **Stop first, and start again on failure**, if it ran: as close to "runs on" as it gets, but the
  turns under way are lost all the same, and a session started by the deletion's failure is a
  surprise of its own.
- **Keep the order, and refuse writes into a scope being deleted**: the session runs on after a
  failure, but every writer into a scope would have to ask whether its holder is being deleted - a
  second state beside the mark.

## Done when

- What is written into the scope of a session or channel while it is deleted cannot be left behind.
- What a failed deletion does to a running session or channel is decided and described in
  `docs/CHANNELS.md`.

## History

- **2026-10-23** — Opened while implementing step 1 of [332](332-policy-as-a-predicate.md): with
  deleted groups admitting nobody, the `STOPPED` events of a deleted session reach its owner alone,
  which showed that it is stopped only once it is deleted.
