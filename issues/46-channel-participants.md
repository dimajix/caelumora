# 46 — Participants cannot be added to or removed from a channel

**Status:** done
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
- **2026-09-27** — Channels now get a confidentiality level
  ([61](61-reasoning-leaves-the-agent.md)); what may be read and published in
  a channel depends on its level, not on its participants. That guarantee is
  only as good as the control over who joins: adding participants must be
  checked against the channel's level (e.g. `ActionType::JoinChannel`) -
  part of this issue.
- **2026-09-28** — Decided with the maintainer: adding and removing
  participants are operations of the `ChannelLoop` (like `cancel`, `wakeup`),
  not of the `SessionLoop`, which will only add participants new to the
  session. When adding, the caller chooses how much history the newcomer may
  see - all, none, the last n days or the last n messages - stored as a fixed
  boundary (a message id), so it does not move day by day; it limits what an
  agent gets into its transcript, what a user gets over REST and in the
  snapshot, and `read_history` on the own channel. Notes stay visible to
  everyone, "written on paper". Only the owner adds and removes, checked
  also as `ActionType::JoinChannel` with the channel's classification. A
  removed agent is stopped at once, its pending approvals are discarded, its
  state kept. Since channels live inside the session document, storing a
  channel change would need the `SessionController`; channels become
  entities of their own first: [70](70-channels-as-entities.md).
- **2026-09-28** — Step 1, runtime and REST. The `ChannelController` is the
  one place that changes the participants: `add_participant` (with an
  optional `HistoryStart`) and `remove_participant` recompute the security
  attributes, store the channel (`UpdateChannel`) and announce the join or
  leave; `process_global_event` is gone, so an event no longer changes a
  channel. `Channel.history_limits` holds the boundary per participant
  (`HistoryAccess::resolve` turns the caller's choice into a
  `HistoryStart`, a message id); `Channel::visible_history` applies it. The
  `ChannelLoop` checks `JoinChannel`, starts or stops the actor (the run loop
  takes changes over a queue and ends once its last actor has ended), tells
  the moderator and rebuilds the mentions. A joining agent gets the visible
  history into its transcript, as far as it does not have it - rejoining
  adds only what it missed. A removed actor is told through `watch_leaving`,
  which ends a user's SSE stream of the channel; the approver answers a
  leaving agent's pending approvals as denied, so no turn keeps waiting. A
  channel the viewer may not read (`GetChannel`) is now answered as absent by
  REST and left out of lists and snapshots. REST:
  `POST …/channels/{cid}/participants` (`{participant, history}`) and
  `DELETE …/channels/{cid}/participants/{aid}`. The web UI follows as step 2.
- **2026-09-28** — Step 2, web UI. Channel responses carry `can_change`
  (`UpdateChannel`); where it is set, the member list offers "Add
  participant" (a dialog with the session's participants not yet in the
  channel and the history choice) and a remove button per member except the
  approver. The channel is fetched again on `AgentChannelJoined`/`Left` and
  `UserChannelJoined`/`Left` for it. Tried in the browser: an agent added with
  the last message sees exactly that one, also through `read_history`, and is
  stopped on removal. That trial showed that an agent rejoining with a wider
  history got the older messages appended after its own answers; decided
  with the maintainer: a rejoining agent's transcript gets only what was said
  after the last message it has, older ones stay reachable through
  `read_history`. Where access helpers such as `may_be_changed_by` should
  live is [71](71-access-helpers-on-entities.md). Status set to done.
- **2026-09-30** — `JoinChannel` now decides by clearance (step 5 of
  [133](133-classification-model.md)): it carries the participant's roles,
  and the ownership policy refuses whoever is not cleared for the channel's
  classification - when added, when the channel is created, and when it is
  raised above them.
