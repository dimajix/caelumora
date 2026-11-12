# 562 — Nobody can leave a channel by their own act

**Status:** done
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Summary:** Who takes part in a channel can leave it only by being removed by whoever may change it (`UpdateChannel`), so a participant who is not the channel's owner cannot take themselves - or their own agent - out of a channel, though `Join` lets them take themselves in.

## The problem

Taking someone out of a channel is `ChannelLoop::remove_participant`, whose store asks
`UpdateChannel` - by default the channel's owner alone. Who may join a channel by a `Join` entry,
or was added by its owner, has no way out of it but asking the owner. The command
`/participant remove` of issue [560](560-commands-are-not-grouped-by-what-they-act-on.md) asks
the same, also for oneself.

Joining has a way of one's own: `Join` lets one take oneself, or one's own agent, into a session
or channel (`SessionLoop::join`, `ChannelLoop::join`; [SECURITY.md](../docs/SECURITY.md)).
Leaving has none.

## What is wanted

`channel leave`, proposed with the commands of 560, by which someone takes themselves out of the
channel; the maintainer decided that it is an issue of its own, since it needs a right and an
operation that do not exist yet.

## Considerations

### Who may leave

- **Anyone taking part, for themselves**, without a right of the access list: leaving takes
  nothing from anyone, and a participant who may not leave is held in a conversation. A policy
  could still refuse it, as it can refuse everything, if the action is asked.
- **Their own agent too**, as `Join` lets one take one's own agent in.
- Whether the last participant may leave, or the owner, and what becomes of a channel nobody takes
  part in.

### The action

A new `ActionType` - `LeaveChannel` - asked by a new operation of the channel, or
`remove_participant` asking it instead of `UpdateChannel` when the one removed is whom the context
acts for. The second is one operation with two rules inside it; the first a second way to the same
change.

### What is recorded

A leave is recorded and announced as `ParticipantLeft`, as a removal is; who left is who removed.
The former participant is kept with who removed it (issue 164), here themselves.

### The session

Leaving a channel leaves one in the session, as `/participant remove` does; leaving the session is
not part of this, and has no operation either.

## Done when

- Someone taking part can take themselves, or their own agent, out of a channel without the right
  to change it, as decided above.
- `channel leave` does it by command.
- The security suite covers it.
- The decisions above are recorded here.

## History

- **2026-11-12** — Issue created at the maintainer's request, split off from 560: `channel leave`
  needs a right and an operation of its own. Nothing is decided yet.
- **2026-11-12** — Decided with the maintainer:
  1. Leaving is an operation of its own, `ChannelLoop::leave`, beside `remove_participant`, as
     `join` stands beside `add_participant`: both go out by one private way, as `take_in` is the
     one way in.
  2. It asks `ActionType::LeaveChannel` with the channel's attributes - no `CustomVerb`, since
     leaving needs no right a `Requirement` could name -, in the context of the one who leaves, an
     agent by its owner, once the kernel found that the participant is whom the caller acts for or
     their own agent, as for joining. The `OwnershipPolicy` allows it always.
  3. A leave is stored in the kernel's name; the former participant is kept as removed by who
     left, so `ChannelController::remove_participant` is told who removed apart from in whose name
     it stores.
  4. An agent without an actor - deleted and resting - is taken out by whoever may change the
     channel alone, as before.
  5. No rule for the owner or the last participant: the owner may leave and keeps the channel,
     the last may leave and the channel stays empty, as removing allows already.
  6. A personal invitation is used up as one comes in; who left comes back only by an entry for a
     role, a group or everyone, or added by whoever may change the channel.
  7. `/channel leave [agent]` acts on the channel it is issued in: the issuer leaves, or the agent
     of theirs named.
  8. The REST API and the web UI let one leave, oneself or one's own agent.

  In two steps: first who removed apart from in whose name it is stored, and the question whether
  a participant is the caller's own apart from joining, without a change of behaviour; then
  leaving.

  Step 1 done: `Participation::require_own_act` asks whether a participant is the caller's own for
  any action, `ChannelController::remove_participant` is told who removed.
- **2026-11-12** — Step 2 done: `ActionType::LeaveChannel`, `ChannelLoop::leave` beside
  `remove_participant`, both by `let_go` with `Exit`; `/channel leave [agent]`;
  `POST …/participants/{aid}/leave`; `can_leave` of a participant, and the web UI's "Leave
  channel" and "Take out of channel". Resolving the one who leaves, as joining resolves, asks
  an agent's `GetPersonality` in its own name before it is known to be the caller's; one whose
  personality it may not read cannot leave, as one deleted.
