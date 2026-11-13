# 566 — The terminal tells little at its start, and nothing of an approval withdrawn elsewhere

**Status:** open
**Type:** feature
**Priority:** low
**Area:** User interaction
**Summary:** At its start the terminal tells only who answers in the channel it talks in - not which channel that is, nor the others the user takes part in; and of a channel it does not talk in, it tells an approval requested but not that it was withdrawn.

## The problem

With issue [561](561-the-terminal-shows-one-channel-of-a-session.md) the terminal follows every
channel of its session the user takes part in, talks in one of them, and switches to another by
`/switch`. Two things it tells less of than it could:

1. **The start.** It prints who answers in the channel it talks in ("Moderator: …, responders:
   …", `Followed::who_answers`) and nothing else of the channels. `/switch` tells more: which
   channel is talked in now, who answers there and what waits for the user there
   (`Channels::switch`). Which channels the user could switch to is told only where a switch
   fails or no channel is talked in (`Channels::hint`): a user of several channels does not learn
   at the start that there are others.
2. **An approval withdrawn elsewhere.** Of a channel not talked in, the terminal tells what waits
   for the user (`Followed::waits_for_user`) - an approval requested among it, marked with the
   channel's name. That the approval was withdrawn (`ApprovalRequestWithdrawn`) it tells of the
   channel talked in alone; of another, the user learns it only as `/approve` answers that none is
   pending.

## Considerations

- **The start:** the lines `/switch` prints for the channel talked in, and the hint where the user
  takes part in more than one channel.
- **A withdrawal:** told of another channel as what waits for the user is, marked with its name,
  where the request was told: `waits_for_user` would then answer what concerns the user rather
  than what waits for them, or a withdrawal becomes a case of its own beside it.

## Done when

- At its start the terminal tells which channel it talks in and, as decided, the others the user
  takes part in.
- The withdrawal of an approval told of another channel is told as well, as decided.

## History

- **2026-11-13** — Opened at the maintainer's request on the review of step 12 of issue
  [561](561-the-terminal-shows-one-channel-of-a-session.md).
