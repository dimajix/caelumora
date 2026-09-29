# 120 — Sessions and channels store copies of their participants

**Status:** open
**Area:** Sessions and channels

## The problem

`Participant` (`caelumora-model/src/channel.rs:133`) is `User(User)`,
`Agent(Agent)` or `Approver(Approver)`: the whole record, not a reference.
`ParticipantTemplate::instantiate` (`template.rs`) reads the user from the
store and puts the copy into `Session.participants` and
`Channel.participants`, which are stored with the session and the channel.
`ChannelLoop::create_actor` then builds the `UserActor` - and with it the
`UserController` - from that copy, not from the store.

Agents are embedded the same way.

## Why it matters

- **The copy goes stale.** A renamed user keeps the old name in every
  session made before, and after a restart the `UserController` starts from
  that old name again; `UserUpdated` only reaches controllers that happen to
  be running.
- **It discloses more than the session needs.** A user record belongs to the
  system and holds `email` and `roles` (issue [94](94-users-unchecked-in-store.md));
  every participant who may read the session reads these of every other
  participant, and they end up in every session document and REST response.
- **Two sources of truth.** Whoever changes a user through the store changes
  one of them.

## Considerations

- **Reference, resolve at load (the maintainer's direction).** A participant
  holds only the id (`Participant::User(UserId)`); the running session keeps
  one `UserController` per user participant, built from the store when the
  session loads and kept current through `UserUpdated` - the same way it
  keeps its `SessionController`, `ChannelController` and `AgentController`s.
  The `UserActor` gets that controller instead of a `User`. What the
  session shows about a user (the name) comes from the controller.
  Costs: the session and channel documents change shape; REST responses
  and the web UI that read `participants` must get names another way
  (resolved by the server, or a separate lookup); a user that no longer
  exists must be handled when a session loads.
- **Keep the copy, trimmed.** Store only `id` and `username`, refresh on
  `UserUpdated`. Smaller change, but still a copy that goes stale while the
  session is not running.
- **Agents.** `Participant::Agent(Agent)` has the same shape and the same
  staleness; whether agents follow in the same step or get their own issue
  is to be decided with the design.

## Done when

No session or channel document holds a copy of a user record; the running
session's view of a user is its `UserController`, current across renames
and restarts.

## History

- **2026-09-29** — Opened while working on issue 94: once user records
  belong to the system and are readable by everybody, the copies in
  sessions are the remaining place where a user's `email` and `roles`
  travel with every session. The maintainer suggested that the session use
  a `UserController` to take changes in.
