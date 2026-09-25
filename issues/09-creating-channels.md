# 09 — Creating channels

**Status:** open
**Area:** Sessions & channels

## The problem

Channels can only come into existence by instantiating a `SessionTemplate`.
The REST API exposes `GET /sessions/{id}/channels`,
`GET /sessions/{id}/channels/{cid}` and the per-channel message, approval,
cancel and event routes — but no `POST`. There is no `create_channel` anywhere
in the runtime either; `SessionLoop` manages the channels it was built with.

So the set of channels in a session is fixed at creation time, and the only
way to get a differently-shaped conversation is to define another template and
start another session.

## Why it matters

A session is meant to be the container for an interaction context and a
channel one medium within it. Fixing the channels at template time collapses
that distinction: in practice a session *is* its channels. Anything that wants
a side conversation — a sub-task delegated to one agent, a private thread with
one participant — has nowhere to go.

## Considerations

- The lifecycle needs to be symmetric: creating a channel implies closing one.
  Both need `GlobalEvent`s, and `02faff5` already added the session/channel
  management events they would use.
- Participants are the substance of the question, not the channel itself. A
  new channel is only useful if agents and users can be placed into it, which
  means the `Participant` set has to be mutable at runtime and the
  `ChannelLoop` has to spawn actors on demand. `ApproverActor` already
  recomputes pending approvals on join/leave events, so part of the dynamic
  case is anticipated.
- Where does a dynamically created channel's `Moderator` come from — the
  request, the session template, a default?
- If an *agent* may create a channel (delegating a sub-task), this becomes a
  capability question and needs the security work in
  [12](12-user-and-agent-permissions.md).

## Done when

- Channels can be created in and removed from a running session via the
  client API and REST.
- Participants can be added to a channel after it exists.

## History

- **2026-09-10** — Issue created from `TODO.md`.
- **2026-09-25** — Scope and approach decided:
  - **Split:** adding and removing participants moves to
    [46](46-channel-participants.md). This issue is about creating and
    deleting channels. The "done when" point on participants is covered there.
  - **Structure:** creating and deleting a channel changes the session
    document, like renaming it: stored first (where the permission is checked),
    then changed live, then announced. `SessionController` changes the
    document, and `SessionLoop` starts or stops the `ChannelLoop`. Every
    channel runs in its own task with an `AbortHandle` (the unused
    `ChannelHandle`), instead of a `JoinSet` filled once in `run()`.
  - **Building a channel:** one function builds a channel from participants,
    adding the approver and deriving the security attributes. It is used by
    both `ChannelTemplate::instantiate` and the new creation path.
  - **Participants of a new channel:** chosen from the session's participants.
    The creator is always included.
  - **Moderator:** from the request, `round_robin` by default like
    `ChannelTemplate`.
  - **Permission:** creating and deleting count as changing the session
    (`UpdateSession`), so only the owner may do it. Separate actions are
    introduced only when contributors are to be allowed.
  - **The last channel** may be deleted; a session without channels is
    harmless and can get a new one.
  - **State:** deleting a channel also deletes its channel state and the
    agents' channel states, through new `delete_…` methods of the
    `StateStore`. `delete_session` uses them too, since it leaves the same
    orphans today.
  - **API and UI:** `POST /sessions/{id}/channels` with
    `{ name, moderator?, participants }` and
    `DELETE /sessions/{id}/channels/{cid}`. In the web UI, "+" on a session
    opens a dialog for name, moderator and participants, and the channel's
    menu gets "Delete".
  - Agents creating channels stay out until
    [12](12-user-and-agent-permissions.md).
