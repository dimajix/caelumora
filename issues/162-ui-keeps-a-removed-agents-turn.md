# 162 — The web UI keeps showing the turn of an agent removed during it

**Status:** open
**Area:** User interaction

## The problem

When an agent is removed from a channel while its turn runs, the turn is
given up without an event to the channel: a stopped agent announces
nothing (`GivenUpReason::Stopped`, no `TurnFailed`, no
`FullResponseGenerated`). The web UI learns that the agent left - it
disappears from the members - but not that its turn ended. Its tool call
stays "EXECUTING", a "generating…" placeholder stays below it, and the
stop button stays red, until the page is reloaded.

Found in the web UI while testing [152](152-three-signals-end-a-turn.md).
Together with [161](161-tools-cannot-be-interrupted.md) the actor was even
aborted, but the UI shows the same without it.

## Why it matters

The user sees a turn under way that will never end, and a stop button that
has nothing to stop.

## Considerations

- **The UI ends what it shows** for a participant that leaves: when the
  channel's members change (it learns of it through the global
  `ChannelUpdated`, `globalEvents.ts`), it clears the pending message and
  the "executing" calls of whoever is no longer a member. No change to the
  runtime.
- **The runtime announces the end:** a stopped agent sends a last event -
  `TurnFailed` with a message, as a lowering does - before its inbox
  closes. But it has left the channel by then; whether its last event may
  still be routed is a question of its own.
- The first is smaller and covers an agent aborted without a word too.

## Done when

- Removing an agent during its turn leaves nothing in the UI that looks
  like a turn under way.

## History

- **2026-09-30** — Found in the UI test of
  [152](152-three-signals-end-a-turn.md).
