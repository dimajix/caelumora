# 262 — An agent removed while its turn ends leaves no end in the channel's history

**Status:** open
**Area:** Sessions and channels

## The problem

An agent removed from a channel cannot tell the channel that its turn ended: the channel drops what
a removed actor still sends. So `ChannelLoop::remove_participant` asks the actor for its turn under
way before it stops it, and records that turn as given up, `Stopped`
([258](258-what-was-said-is-lost.md)). `RunControl::turn_under_way` names a turn only while it is
interruptible - planning, running tools, waiting for a person. An agent removed while its turn
reflects or is being cancelled - taken over, its `TurnFinished` not yet sent - leaves the turn in
the channel's history without an end; the agent's own chains record it, as given up.

## Why it matters

The channel's history then holds messages of a turn that never ends there; a reader sees an answer
without its end, and the web UI no notice. Rare - the window is one step - but the record is wrong.

## Considerations

- `Actor::turn_under_way` could name the turn in every state of a turn, not only the
  interruptible ones; the retry of a turn, which asks the same, would then have to check the state
  itself.
- Or the channel records `Stopped` for the last turn of the participant whose messages have no end
  yet, from its own history.

## Done when

- Every turn whose messages are in the channel's history has its end there, also when its agent was
  removed while the turn ended.

## History

- **2026-10-08** — Opened when closing [258](258-what-was-said-is-lost.md), whose plan noted it
  under "What to watch".
