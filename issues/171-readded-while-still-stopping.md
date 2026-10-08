# 171 — A participant added again while its old actor still stops runs twice

**Status:** done
**Area:** Sessions and channels

## The problem

A removed actor is told to stop and ends its task itself, once its step is done and its state
stored; the channel aborts it only after `STOP_GRACE` (issue 46). `add_participant` does not wait
for that. A participant added again within that time gets a new actor while the old one still runs:

- **Both write the agent's state in the channel.** The new actor starts from the stored state; the
  old one stores itself idle as it ends, possibly after the new one stored a turn of its own - which
  is then lost.
- **Both speak under one id.** The channel drops what a removed actor sends by its sender (issue
  [162](162-ui-keeps-a-removed-agents-turn.md)). Once the same id runs again, the old actor's late
  events - a tool result after a hanging tool ([161](161-tools-cannot-be-interrupted.md)) - pass as
  the new actor's.

Found while implementing [162](162-ui-keeps-a-removed-agents-turn.md).

## Why it matters

Rare - a remove and an add within seconds, during a turn - but what it breaks is the agent's stored
state, silently.

## Considerations

- `add_participant` waits for the old actor's task to end, or refuses until it has (`ChannelError`
  saying it is still leaving).
- Events carry no run of an actor, only its id; telling two runs apart in the channel would need
  one.

## Done when

- An actor removed and added again never runs alongside its predecessor.

## History

- **2026-10-01** — Opened while implementing [162](162-ui-keeps-a-removed-agents-turn.md).
- **2026-10-05** — Done: `add_participant` waits for the old actor. `ChannelLoop::leaving` keeps the
  `TaskTracker` of every removed actor; `add_participant` - after the permission and
  `AlreadyParticipant`, before anything is stored or loaded - closes it and waits, at most
  `LEAVE_GRACE` (`STOP_GRACE` + 1 s, by when `run` has aborted the task), else refuses with
  `ChannelError::StillLeaving` (REST `409`). Waiting rather than refusing at once, because since
  [161](161-tools-cannot-be-interrupted.md) an agent ends quickly when stopped, and a user
  re-adding an agent should not see an error for something that settles by itself.

  The analysis found the state race worse than described: a new actor built while the old one was
  in a turn loaded that turn from the store and resumed it, while the old one gave the same turn up.

  Two further gaps closed with it. `run`'s `select!` is `biased` towards the actors' events, so
  that what the old actor sent before it ended is handled before the new actor's `Start` - which
  the announcement of [162](162-ui-keeps-a-removed-agents-turn.md) relied on by chance as well. And
  `run` no longer starts an actor that was removed again before it got to start it. A run id in the
  events (the second consideration) was not needed: two runs of one actor no longer overlap.
- **2026-10-08** — Since [262](262-stopped-turn-not-recorded-while-reflecting.md) the removal is
  announced once the old actor's task has ended, which `run` learns from its `JoinSet`; `left`
  waits on the `TaskTracker`, so a new actor's `Start` may reach `run` before it saw the task end.
  `run` then announces the removal before it starts the new actor, so that nothing of the new one
  precedes it.
