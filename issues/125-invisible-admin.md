# 125 — There is no administrator who may see every channel

**Status:** rejected
**Area:** Identity and access management

## The problem

Every user's access follows from ownership and participation (issue 11). There is no role that may
see - and follow live - any channel of any session without taking part in it, for operating and
supporting the system.

## Why it matters

Following a channel live is for its participants (issue [104](104-sse-stream-and-subscribe.md)); an
operator who has to look into a session should neither have to join it nor be seen by its
participants and agents.

## Considerations

- **A role, not a participant.** The administrator is invisible: no actor in the channel, no member,
  nothing in the agents' context. So, unlike an observer (issue
  [124](124-observer-participants.md)), they cannot watch through a `UserActor`; they need a
  read-only way to the channel's events of their own - the one deliberate exception to "live only
  through one's actor", named as such.
- **Where the role lives**: `User::roles` exists but decides nothing yet; the
  `OwnershipSecurityManager` (or a policy on top of it) would grant an admin
  `GetChannel`/`GetSession` for everything. Whether an admin may also act (cancel, wake, delete) is
  a separate decision.
- **Audit**: an invisible reader should at least be logged.

## Done when

A user with the admin role can list and follow any channel live without being a participant, and
nobody in the channel sees them.

## History

- **2026-09-29** — Opened with issue 124 while working on issue 104: the maintainer wants live
  following for participants only, and an invisible admin with a way of their own as the exception.
- **2026-10-05** — From [164](164-soft-deletion-and-former-members.md): what users delete is only
  marked, and removing it for good - `ActionType::Purge` - is the administrator's. Until this role
  exists, the default policy grants `Purge` to the system alone, so only the retention policy
  ([07](07-session-deletion-audit.md)) purges. The administrator is to be granted, for every
  entity: `ReadDeleted` (the trash of everybody), `Restore` and `Purge`. This answers part of
  "whether an admin may also act" above: these three, at least.
- **2026-10-15** — Rejected by the maintainer: a wild idea, out of the way for now. Both halves go -
  the invisible live following and the administrator role with `ReadDeleted`, `Restore` and
  `Purge`. `Purge` stays the system's alone, so only the retention policy
  ([07](07-session-deletion-audit.md)) removes for good; purging or restoring what others deleted
  by hand becomes an issue of its own if it is wanted.
