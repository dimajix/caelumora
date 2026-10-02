# 171 — A participant added again while its old actor still stops runs twice

**Status:** open
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
