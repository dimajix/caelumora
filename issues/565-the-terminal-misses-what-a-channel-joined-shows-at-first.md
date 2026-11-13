# 565 — The terminal misses what a channel joined meanwhile shows at first

**Status:** open
**Type:** bug
**Priority:** low
**Area:** User interaction
**Summary:** The terminal follows a channel the user joins while it runs once the event bus tells so, and is shown only what comes after; what the channel showed the newcomer in between - an approval to answer, questions put to them - the terminal never tells.

## The problem

With issue [561](561-the-terminal-shows-one-channel-of-a-session.md) the terminal follows every
channel of its session the user takes part in, also one they join while it runs: it subscribes to
the event bus for its session in the user's name, and at a `channel::JOINED` naming the user it
takes a watch of the user's actor in that channel (`Channels`, `Followed::new`,
`ChannelView::user_follow`). A watch shows what is shown after it is taken (`ChannelWatch`, a
subscription of the user's outbox in the channel).

Between the join and the watch - while the bus delivers the event and the terminal takes it in -
the newcomer's actor may already run and show something, and what it shows then never reaches the
terminal. Of what waits for the user:

- **An approval.** The terminal knows the approvals pending in a channel only from the requests it
  was shown (`ApprovalRequested`): `/approve` answers that none is pending, and `/switch` does not
  show it.
- **Questions put to them.** Their actor knows them (`UserActor::questions_to_answer`), so
  `/answer` finds them; but the terminal never printed them, and nothing tells the user that they
  wait.

The window is short, and the same as for what a channel showed before the terminal started, which
561 accepts (decision 1 of its History); here it opens while the terminal runs.

## Considerations

- **Questions:** where the terminal follows a channel anew, it shows what waits there
  (`Followed::waiting`), which asks the actor - cheap, and without a new source.
- **Approvals:** what is pending is the agents' stored state, which the views read
  (`ChannelView::pending_approvals`, the snapshot's approvals, as the channel stream of the REST
  server begins with them). Read where a channel is followed anew, it would also close the window
  at the start of the terminal.
- **Nothing:** accepted as 561 accepts what came before the start.

## Done when

- What waits for the user in a channel they join while the terminal runs is told by it, or the
  decision to leave it so is recorded here.

## History

- **2026-11-13** — Opened at the maintainer's request on the review of step 12 of issue
  [561](561-the-terminal-shows-one-channel-of-a-session.md), priority low.
