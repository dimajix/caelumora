# 125 — There is no administrator who may see every channel

**Status:** open
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
