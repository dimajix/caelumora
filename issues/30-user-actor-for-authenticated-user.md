# 30 — Messages come from the channel's first user actor, not from the sender

**Status:** done
**Area:** Sessions and channels
**Summary:** Messages and approvals from the REST API go through the channel's first `UserActor`, so they are recorded in the name of someone other than the authenticated sender.

## The problem

Since [10](10-user-authentication.md) the REST server knows who sends a request, but it does not use
that when acting inside a channel. `send_message` and `send_approval`
(`caelumora-rest-server/src/routes/channels.rs`) both take `ChannelLoop::user_actor()`, which
returns the *first* `UserActor` of the channel, whoever that is. The authenticated user only goes
into the `SecurityContext` used to load the session.

So when bob opens a session alice created and writes into it, the message appears in the history as
alice's: `sender_id`, `sender_name`, the `SecurityContext` of the envelope, everything. The same
goes for approving a tool call.

## Why it matters

- **The record is wrong.** The history attributes messages to someone who did not write them. For an
  audit trail ([05](05-postgres-event-recorder.md), [06](06-context-in-message-for-auditability.md))
  that is worse than no attribution at all.
- **Approvals cannot be tied to a person.** [14](14-approval-privilege.md) wants to decide who may
  approve a tool call. That is meaningless while every approval is made in the name of the first
  user actor.
- **Agents see the wrong person.** They address and remember the sender by name; an `@`-mention of
  the real sender ([29](29-mentions.md)) addresses someone else's actor.
- It is the step that makes authentication visible inside a channel at all. Until it is done,
  logging in only decides *whether* someone may write, not *as whom*.

## Considerations

### Picking the actor

The lookup itself is easy: the `UserActor` whose `ActorId` is `ActorId::User(authenticated.id)`.
`ChannelLoop` has `actor_by_id` already, so `user_actor()` should either take the user or disappear.
The hard part is what happens when there is no such actor.

### A user who is not a participant

Channels are built once from their template. The session creator fills the template's "user"
participant; nobody else is a participant, and `ChannelLoop` creates its actors in `new()` and never
adds one. `GlobalEvent::UserChannelJoined` updates the channel *model*, but no actor is created for
it.

The options:

- **Reject with `403`.** Simple and safe, and correct once [11](11-agent-ownership.md) decides who
  may see a session at all. But until there is a way to add participants, it makes every session
  single-user.
- **Join on first write.** Adds the user as a participant and creates a `UserActor` for them. Needs
  `ChannelLoop` to add actors at runtime (spawn the actor, register it with the moderator, update
  the channel model and store) — which is exactly the missing piece in
  [09](09-creating-channels.md). It also means that reading a session is enough to become a
  participant of it, which is a policy decision, not a technicality.
- **Explicit join.** A separate endpoint (and UI action) to join a channel, with `403` for writes
  until then. Same runtime mechanism as above, but joining is a deliberate act and can be
  permission-checked.

The first option can be done now and is safe. The third is where it should end up; it depends on 09.

### What else follows the same rule

- `send_approval`: the approving user is the authenticated one, and approvals by non-participants
  are rejected — this is the hook [14](14-approval-privilege.md) builds on.
- `cancel_channel`: currently acts on the channel as a whole, with no user. Whether a
  non-participant may cancel is the same question.
- Reading (`/messages`, `/events`): whether a non-participant may *read* a channel is
  [11](11-agent-ownership.md)'s question. This issue is only about acting in it.

### Not affected

The TUI runs in-process with exactly one user and uses `user_actor()` legitimately. If
`user_actor()` goes away, the TUI needs the id of its own user, which it has.

## Done when

- `send_message` and `send_approval` act through the `UserActor` of the authenticated user; no REST
  route uses "the first user actor" anymore.
- A user who is not a participant of the channel gets `403` for writes (or is joined, if
  [09](09-creating-channels.md) has made that possible by then — either way, decided and written
  down).
- A message written by bob appears in the history as bob's, with his `sender_id` and
  `SecurityContext`.

## History

- **2026-09-21** — Issue created after [10](10-user-authentication.md) went in: authentication
  works, but every message and approval is still sent through `ChannelLoop::user_actor()`, i.e. in
  the name of the channel's first user.
- **2026-09-23** — Implemented. Decisions:
  - **The actor is looked up by the user.** `ChannelLoop::user_actor()` is gone;
    `user_actor(UserId)` finds the actor with `ActorId::User(id)`, so "the first user actor" is no
    longer reachable from anywhere. The TUI passes the id of its own user, which it had all along.
  - **A non-participant is refused** with `403 not_a_participant` - option 1 of the three above.
    Joining on first write and an explicit join both need actors to be added to a running channel,
    which is [09](09-creating-channels.md); this issue does not wait for it. Until then a channel's
    participants are the ones it was created with, and a session is effectively single-user.
    Sessions created before [10](10-user-authentication.md), whose user participant is some older
    id, become read-only for everyone - deliberate, and cheaper than guessing who they belonged to.
  - **`cancel_channel` gets the same participant check.** Cancelling ends the turn of every agent in
    the channel, so it is an action *in* the channel; there is no reading reason for it.
  - **Cancel and wakeup additionally need a permission,** `CancelChannel` and `WakeChannel`, checked
    in `ChannelLoop` rather than in the actors: the permission is about the channel, a per-actor
    check would have to summarise N answers and could leave a channel half cancelled, and
    `Actor::cancel`/`wakeup` also run where there is no user at all (channel teardown, mentions,
    `WakeupRequested`) - a check there would need an invented "system" context, which is how
    permission checks become decoration. Refusals are `ChannelError::Forbidden`, which
    `caelumora_runtime::error::is_forbidden` tells apart from a failure.
  - **Only the deliberate wakeup is checked.** Waking because something is being delivered goes
    through `ChannelLoop::wake_for_delivery`, which asks nothing: it follows from an action that was
    already permitted, and a user allowed to write but not to wake could otherwise not write at all.
    The checked `wakeup` gets its first caller with `/wakeup` from [28](28-user-commands.md).
  - **The `SecurityManager` now reaches the runtime.** `RuntimeManager::new` built its own
    `DummySecurityManager` and ignored the one the binaries configure, so every check inside the
    runtime - including the existing `CallTool` - was answered by a default nobody chose. It is a
    constructor argument now. Nothing changes in behaviour while [24](24-opa-security-manager.md)
    leaves `DummySecurityManager` as the only implementation, but a policy can now be plugged in.
  - Still open, as before: who may *read* a session is [11](11-agent-ownership.md), and which
    participants may approve is [14](14-approval-privilege.md) - which can now be built, because an
    approval finally carries the person who made it.
  - Verified against the REST server with two users: alice creates a session and writes; the history
    shows her `sender_id` and name. bob, who may see the session, gets `403 not_a_participant` for
    writing, approving and cancelling; alice cancels successfully. A refusal by the
    `SecurityManager` itself is covered by a unit test with a deny-all manager - live it cannot be
    shown, because `DummySecurityManager` allows everything.
