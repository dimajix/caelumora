# 46 — Participants cannot be added to or removed from a channel

**Status:** open
**Area:** Sessions & channels

## The problem

The participants of a channel are fixed when the channel is built. That
happens either from a session template or, once [09](09-creating-channels.md)
is done, when a channel is created in a running session. After that nobody can
join or leave:

- `ChannelLoop` holds its actors in a fixed `HashMap`, built in its
  constructor, and builds the `MentionDirectory` once from it.
- `ChannelController::process_global_event` does take `AgentChannelJoined` and
  `AgentChannelLeft` into the channel's participant list, but no actor is
  started or stopped for it, and the moderator never learns of it.
- There is no API to ask for a join or a leave.

So a conversation cannot grow: an agent cannot be brought into a running
discussion, and a second user cannot be invited into a channel.

## Why it matters

- Being able to create channels ([09](09-creating-channels.md)) is of limited
  use if their participants are fixed at that moment.
- Sharing a channel with another user means making them a participant.
  Access to a channel follows from its participants: `participant_attributes`
  makes every user and agent in it a contributor (issue
  [11](11-agent-ownership.md)).

## Considerations

### The runtime: a mutable actor set

- `ChannelLoop.actors` has to become mutable at runtime (e.g. `ArcSwap` or a
  lock), and every place that reads it has to cope with it changing: routing,
  `members()`, `user_actor()`, `actor_by_id()`, cancelling, waking.
- **Joining** starts the participant's actor: its task, its transport,
  `Moderator::add_actor`, and a rebuilt `MentionDirectory`. The `run` loop
  currently starts every actor once, so it needs a way to take on a new one,
  much like the `SessionLoop` after [09](09-creating-channels.md).
- **Leaving** stops the actor. What happens to a turn it is in the middle of,
  and to approvals it is waiting for, has to be decided. `ApproverActor`
  already recomputes pending approvals on join and leave events.
- A newly joined agent has no `AgentChannelState` for this channel. Whether it
  sees the history so far, all of it or from now on, is a decision of its own,
  and it matters for what the agent can later be asked about and for
  [06](06-context-in-message-for-auditability.md).

### Persistence and events

- Channels live inside the session document (`Session.channels`), so a join
  or leave changes the session and is stored through it. This follows the
  pattern [09](09-creating-channels.md) establishes for channels: store first
  (where the permission is checked), then change the running channel, then
  announce it.
- The events already exist: `AgentChannelJoined`/`Left` and
  `UserChannelJoined`/`Left`. The web UI refreshes on them (issue
  [35](35-sidebar-live-updates.md)).
- The channel's `SecurityAttributes` are derived from its participants and
  must be recomputed on every join and leave. Otherwise a removed user keeps
  their access, and an added one does not get it.

### Who may do it

- Only participants of the session can join at first, in line with
  [09](09-creating-channels.md). Bringing a new agent from a personality into
  a running session, or a user who is not yet part of the session, is a
  further step.
- Who may add or remove whom is the real question. Only the owner is the
  obvious start, consistent with `UpdateSession`. Whether contributors may
  invite, and whether anyone may leave on their own, belongs to
  [12](12-user-and-agent-permissions.md) and [23](23-acls-in-security-attributes.md).
- An agent adding participants (delegation) waits for
  [12](12-user-and-agent-permissions.md), like agents creating channels.

### Mentions

The `MentionDirectory` has to follow the set of participants. A handle that
becomes ambiguous by a join is logged, like at channel start (see
[29](29-mentions.md)). Mentions stored in past messages stay as they were;
they refer to `ActorId`s, not to handles.

## Done when

- A participant of the session can be added to and removed from a running
  channel through the client API and REST, and the web UI offers it.
- The added participant's actor runs, is routed to and can be mentioned; the
  removed one's is stopped and is no longer routed to.
- The channel's security attributes follow its participants.
- It is decided and documented which history a newly joined agent sees and
  what happens to a removed agent's running turn and pending approvals.

## History

- **2026-09-25** — Issue created, split off from
  [09](09-creating-channels.md), which covers creating and deleting channels.
  The two are independent, and adding participants touches the security
  questions of [12](12-user-and-agent-permissions.md) and
  [23](23-acls-in-security-attributes.md).
