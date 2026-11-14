# 565 — The terminal misses what a channel joined meanwhile shows at first

**Status:** done
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
- **2026-11-14** — Decided with the maintainer: both, as the channel stream of the REST server
  reads its snapshot. Where the terminal follows a channel (`Followed::new`), it reads after the
  watch began what waits for the user there: the approvals pending, as the agents' turns hold
  them (`ChannelView::pending_approvals`, which so has its first caller outside the tests), and
  the open questions put to them, as their actor tells. What comes in both is shown once: an
  approval request known already is no longer shown again (`Followed::take_in`), also one an
  agent that resumed its turn asks again, as questions were before. The questions printed became
  the questions known (`known_questions`, `Followed::knows_questions`): `/answer` and `/decline`
  with an id found questions shown only by `/switch` in no channel; `Followed::waiting` shows
  only the questions known, so that questions asked while channels are followed are not shown
  twice. Of a channel joined meanwhile, the terminal tells what waits there with the join, marked
  with its name (`Channels::follow_anew`). Approvals that cannot be read are not known, logged,
  and the channel is followed all the same. Left as it is, and documented at `Followed::new`: an
  approval decided just before the watch began may still be held by a turn not stored anew; it
  stays known as pending while the terminal runs, and the approver refuses an answer to it, which
  the user is told.
- **2026-11-14** — Decided with the maintainer on the review: since an approval known as the
  channel is followed is no longer shown again, one an agent asks again after a restart in a
  channel not talked in would be told by nothing until `/switch`. So the terminal tells at its
  start what waits in every channel followed, those not talked in marked with their names, as
  following a channel anew does (`Channels::opening`) - unlike the decision of issue
  [566](566-the-terminal-tells-little-at-its-start-and-of-withdrawals.md), which listed only the
  channel talked in; this also closes the window 561 accepted at the start. Done.
