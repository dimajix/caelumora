# 562 — Nobody can leave a channel by their own act

**Status:** open
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
