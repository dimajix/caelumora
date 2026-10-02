# 190 — The channel writes each actor kind's private state when someone joins

**Status:** open
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
