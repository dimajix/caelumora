# 190 — The channel writes each actor kind's private state when someone joins

**Status:** done
**Area:** Code structure

## The problem

When a participant joins, `ChannelLoop::add_participant` matches on the member's kind and writes the
history visible to it into that kind's state itself: `seed_transcript` into an `AgentChannelState`,
`seed_heard` into an `ExternalAgentChannelState` (issue [179](179-external-agents-via-a2a.md)). The
channel knows both layouts, and the external agent was added as a parallel method and a further
arm.

## Why it matters

Every new actor kind needs another `seed_*` method and another arm, and the layout of an actor's
state is known outside the actor.

## Considerations

- The member or the actor seeds itself - `Member::seed(visible, store)`, or `visible` handed to
  `create_actor` so that the actor appends to its own state when it is built. `missing_after` stays
  shared.

## Done when

- `ChannelLoop` no longer matches on member kinds to write actor state.

## History

- **2026-10-02** — Opened after a cleanup review of the issue-179 series.
- **2026-10-02** — Decided with the maintainer: the actor takes in what it missed itself. Step 1,
  without behaviour changes: `Actor::catch_up(visible)` - nothing to do for users and approvers -
  is called by `ChannelLoop::add_participant` after the actor is built and before it is adopted; a
  failure takes the participant back out as before. `AgentActor` keeps what it missed in its
  transcript, `ExternalAgentActor` in its history, each stored by the actor. `seed_transcript`,
  `seed_heard` and the `match` over the member kinds are gone from the channel; `missing_after`
  moved to `actor_impl`, beside the actors that use it. Step 2 follows: an agent run here takes in
  what it missed as what it hears - its history, then its perceive processors, then its transcript
  (issue [196](196-perceive-processors-write-state.md)).
- **2026-10-02** — Step 2, done: an agent run here takes in what it missed as what it hears -
  `catch_up` passes it to `perceive`: all of it into its history, what passes its perceive
  processors into its transcript (issue [196](196-perceive-processors-write-state.md)). What is
  missing is now told against the history for both actors, so `missing_after` became
  `MessageHistory::missing_from`, in the model; it counts a message on any branch as known. A
  joining agent now keeps the visible part in its history too, not only in its transcript. The
  channel tests build an agent's channel without running it (`agent_channel`), as they did for an
  external agent, so that `catch_up` is tested on the actor itself.
