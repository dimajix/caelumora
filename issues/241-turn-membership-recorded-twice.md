# 241 — Which messages belong to a turn is recorded twice

**Status:** open
**Area:** Code structure

## The problem

Since [235](235-retry-of-an-answer-under-way.md) every message a turn produces carries the turn's id
(`Message::turn_id`). Before it, the only record of which messages belong to one turn was
`ContextRef::turn_entries`: for each message of an agent, how many of the transcript's last entries
before it its turn had produced. The `ContextResolver` counts by it to tell the model's own turn
from what came before; `AgentChannelState::retried` tells the end of a turn by internal steps
instead.
Two records of one fact, read by different rules, can come apart.

## Why it matters

AGENTS.md: one owner per concern, no repeated logic. A third reader - an audit view, a grouping of
the history by turn - would have to choose between them.

## Considerations

- `turn_entries` is part of what is recorded with a message (`ContextRef`) and read for every
  context resolved; messages stored before 235 carry no turn id, so it cannot simply be dropped. A
  reader could prefer the id where it is set and fall back to the count.
- `turn_entries` counts transcript entries, not messages: a prompt part placed in the turn is not
  one, a revoked message still is. Whether the id gives the same answer in every case is to be
  checked first, with the resolver's tests.
- `retried`'s rule ("after the message, only its own internal steps") could become "after the
  message, only messages of its turn", which is what it means.

## Done when

Which messages belong to a turn is read by one rule, from one record - or the difference between the
two is stated and kept for a reason.

## History

- **2026-10-06** — Opened from open question 3 of [235](235-retry-of-an-answer-under-way.md).
