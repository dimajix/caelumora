# 348 — The group of a session's participants can be in the trash while the session is not

**Status:** done
**Area:** Identity and access management

## The problem

The group of the participants of a session or a channel (issue [286](286-groups-as-subjects.md)) is
what the session's or channel's own access list lets read it. It lies in their scope and goes and
comes back with it (`ScopedEntities`, `caelumora-runtime/src/scoped.rs`), as whoever deletes or
restores it may (issue [320](320-scoped-deletion-in-the-runtimes-name.md)). Two ways leave it in
the trash while what it belongs to is there:

- **A deletion that fails half-way.** `SessionManager::delete_session` marks the session, then
  deletes what lies in its scope, the groups last (`ScopedEntities::delete_lying_in`). If a group
  cannot be deleted - refused, or the store fails after some were - the session's mark is taken
  away again, but the groups deleted until then stay deleted. `delete_channel` does the same for a
  channel.
- **A restoring that does not reach the groups.** `restore_session` restores the session, then
  what lies in its scope (`restore_lying_in`); a group that is refused or that the store fails on
  stays deleted, and the failure is only logged.

Until the participants change again - `keep_participants` restores a group it finds in the trash
since issue [347](347-entries-naming-purged-groups.md) -, the session's or channel's members do not
reach it by its own list, and whatever else names the group (a workspace attached, an artifact
published in the channel) reaches nobody either.

## Why it matters

- The members of a session lose it without anybody having removed them, and nothing tells them
  why.
- The failure is logged where it happens and then forgotten: the next deletion or restoring does
  not correct it, only a change of the participants does.

## Considerations

- **The groups follow their holder, not the caller.** They are not things of someone's that are
  deleted as far as the caller may, like an artifact of another's in the scope (issue 320): they
  are derived from the participants and written in the runtime's name by what changes them. So
  their deletion and restoring could be the runtime's as well - done with the session's or
  channel's mark, in the system's name -, rather than asked of the caller like the other members of
  the scope. Whether issue 320's reasons (the record of who deleted, the policy asked) apply to
  them is to be decided.
- **Undo what was done.** Where the mark of the session is taken away again, the groups deleted
  with it would be restored with it, in the runtime's name: the deletion did not happen.
- **Restore them first.** `restore_lying_in` restores the groups last, after the other members of
  the scope; restoring them with the session itself, before anything else, would let a failure
  further on leave the rest in the trash but not the members' access.
- **Or correct as read.** Wherever a session or channel is opened, a group of its participants in
  the trash could be restored as `keep_participants` does now. That repairs late and in a place
  that reads, which is not where writing belongs.

## Done when

- A session or channel that is there always has the group of its participants there as well,
  whatever failed while it was deleted or restored.
- A test in the security suite deletes a session where a group cannot be deleted, and restores one
  where a group cannot be restored, and finds the members still reaching it.
- SECURITY.md (*Groups*) and CHANNELS.md say how the group goes and comes back with its holder.

## History

- **2026-10-14** — Opened while issue [347](347-entries-naming-purged-groups.md) was worked on:
  `keep_participants` failed for a group left in the trash, which it now restores; that corrects
  the group at the next change of the participants, not where it was left behind.
- **2026-10-14** — Decided with the maintainer: the groups follow their holder, not the caller.
  Every group in a session's or channel's scope is the participants of one (a team lies only in
  the global scope or a user's), and every other write of it - making it, following the
  participants, purging it - was the runtime's already; only deleting and restoring went through
  `ScopedEntities` in the caller's name. Issue 320's reasons do not hold for it: it is nobody's to
  keep or leave behind, and the record of who deleted stays on the session or channel. Kept from
  issue [319](319-scoped-artifact-events-miss-version-readers.md): the groups are announced as
  they go and come back.
- **2026-10-14** — Done. The `GroupManager` is no `ScopeMember` any more; `ScopedEntities` deals
  with what lies in a scope alone. `GroupManager::delete_participants` and
  `restore_participants` delete and restore the groups within a session's or channel's scope in
  the runtime's name, all or none - one that fails takes back those done before - and announce
  them once all are done. `delete_session` and `delete_channel` delete them last, once nothing
  else of the scope is left, and take the mark away again if they cannot go; `restore_session`
  and `restore_channel` restore them first, with the mark, and delete the session or channel
  again if they cannot come back. `purge_session` purges the session's group by
  `purge_participants`, as the channels' were already, which now takes a group that is not there
  as nothing to purge. Since no policy can refuse the groups any more, the test of the security
  suite asked for in *Done when* became
  `the_group_of_a_sessions_participants_follows_it_whoever_may_delete_groups`: a group alice may
  neither delete nor restore goes and comes back with her session, nothing is asked of her about
  it, and bob reaches the session by it again; the store failing half-way is
  `group::tests::the_participants_go_and_come_back_all_or_none`. Stored data need not be deleted.
