# 243 — The member list does not show what an agent is doing

**Status:** open
**Area:** User interaction

## The problem

The members panel of the web UI lists a channel's participants, but not what an agent is doing:
whether it is idle, asleep, answering, waiting for an approval. Stopping one agent means finding
its answer in the chat (issue [235](235-retry-of-an-answer-under-way.md), step 2); an agent that
works without showing anything - a long tool call, a model that thinks before its first chunk -
looks the same as one that does nothing.

## Why it matters

In a channel with several agents the members panel is where one looks to see who is busy. The
maintainer's wish: a coloured dot per agent for its state, and a cancel icon on an agent that
works.

## Considerations

- **The states.** An agent's actor knows its state (`Actor::state`, an `ActorState`): `Idle`,
  `Sleeping`, and in a turn `Planning`, `ExecutingTools`, `WaitingForApproval`, `Reflecting`,
  `Cancelling`. For users, fewer would do - e.g. idle, asleep, working, waiting for an approval -
  with the turn's states folded into "working". An external agent has a state of its own
  (`set_processing`: `Planning` while it answers, else `Idle`).
- **The snapshot** can carry it as [240](240-answers-under-way-not-in-snapshot.md) carries the turn
  under way: read from the running channel at no cost, `None` where the channel does not run.
- **Keeping it up.** Today the users learn of a turn's start and end (`ResponseStarted`,
  `FullResponseGenerated`, `TurnFailed`) and of approvals, but not of falling asleep (the sleep and
  stop tools) or being woken. Either the panel derives what it can from the events it has and
  reloads on what it lacks, or the agent announces its state - an `ActorEvent` of its own, sent by
  the run loop as it changes state, routed to the users like `ResponseStarted`. The second is
  exact, but every step of a turn would send one; folding to the users' few states would send far
  fewer.
- **The cancel icon** calls `POST …/participants/{aid}/cancel` (`ChannelLoop::cancel_actor`, 235
  step 2), offered where the viewer may cancel the channel - a `can_cancel` the channel snapshot
  does not carry yet; the stop button is offered to everyone today and refused by the server.
- Users and approvers have no state worth showing; the dot is for agents.

## Done when

Every agent in the members panel shows its state as a coloured dot, kept up while the page is open,
and an agent that works can be stopped from there.

## History

- **2026-10-06** — Opened on the maintainer's request while
  [240](240-answers-under-way-not-in-snapshot.md) was implemented.
