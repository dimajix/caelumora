# 458 — A session or channel is deleted while it runs, and stopped only once it is gone

**Status:** done
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
- **Keep the order, ask the mark before and after.** No second state: the mark is the one the
  writers need, asked at the right moments. The holder is marked before its scope is gone
  through, so whoever puts something into a scope asks first that its holder is there and not
  deleted (`ScopeHolders::require`), writes, and asks again: if the holder went meanwhile, what
  was written came after the deletion went through the scope and is taken back - purged with
  what lies behind it, or marked deleted again with the mark it carried (`keep_made`,
  `restore`). What was written before the scope was gone through is deleted with the
  holder and comes back with it. No lock, no wait; one more read per write. Where a deletion is
  rolled back, what went with it stays deleted, as it does today - a writer may then be answered
  with an entity that is deleted.

**Decision (2026-10-29):** the last. A lock per holder - writers reading it, a deletion writing
it - was weighed and dropped: a new type with a table of locks, two guards and an order of
locking every caller has to know, against a window of one store write. Stopping first would not
close the window (a REST request writes into a scope as a tool does) and would leave a session
stopped whose deletion failed. The checks are the managers' - `create`, `restore`, `publish` -,
where every way into a scope ends: the tools and the REST routes change nothing. A session put
into a project while the project is deleted (`SessionLoop::update_project`) is the same shape but
no `ScopeMember`, and is left to an issue of its own. What a stopped session announces reaching
its owner alone stays, as above.

## Done when

- What is written into the scope of a session or channel while it is deleted cannot be left behind.
- What a failed deletion does to a running session or channel is decided and described in
  `docs/CHANNELS.md`.

## History

- **2026-10-23** — Opened while implementing step 1 of [332](332-policy-as-a-predicate.md): with
  deleted groups admitting nobody, the `STOPPED` events of a deleted session reach its owner alone,
  which showed that it is stopped only once it is deleted.
- **2026-10-29** — Decided, as above. Step 1: `ScopeHolders::require`, the one check that the
  holder of a scope is there - a channel's session among it, which is not marked with the
  session -, in place of the agents', workspaces' and artifacts' own; the managers take it as a
  service (`RuntimeBuilder::scope_holders`), `ArtifactManager::new` its services from the builder.
- **2026-10-29** — Step 2, done: `ScopeHolders::keep_made`, the check once written, in every
  `create` and `publish` of a kind that lies in a scope, and `ScopeHolders::restore`, which
  restores and checks for the four managers alike; the skills ask the holder for the first time, as
  does every restoring. The review found the order broken for a session's own project, which was
  marked only after the scope was gone through: it is marked with the session now, and unmarked
  with it if the deletion is rolled back. Documented in CHANNELS.md, PLUGINS.md and
  PERMISSION-CHECKS.md. A session put into a project that is deleted meanwhile
  (`SessionLoop::update_project`) is left to an issue of its own.
