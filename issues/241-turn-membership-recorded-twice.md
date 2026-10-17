# 241 — Which messages belong to a turn is recorded twice

**Status:** done
**Area:** Code structure
**Summary:** Which messages belong to a turn is recorded twice - as the turn's id on each message and as a count in its `ContextRef` - and read by different rules.

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

## Plan

Decided with the maintainer. Stored data of an older shape need not be read: it is deleted (as
[108](108-serde-defaults-for-legacy.md) holds), so nothing falls back to the counts.

### What the code shows

- **Two counts, not one.** `ContextRef::turn_entries` tells the `ContextResolver` which entries of
  the transcript are the turn's own (`in_turn`), which decides what of their reasoning the model is
  given (`ReasoningHistory::CurrentTurn`, the default). `GivenUp::turn_entries` tells
  `Transcript::given_up_turns` where on a given-up branch the turn began; it is covered by the
  digest of the record (`caelumora.transcript-given-up.v1`).
- **A third rule.** `AgentChannelState::retried` tells the end of a turn by "its own internal
  steps": a stand-in for what [234](234-modify-revoke-and-retry-messages.md) defined as "the rest
  of the turn it belongs to", for want of a record of the turn.
- **The id answers the same.** The count grows only in `TurnState::push`, which sets `turn_id`; a
  turn writes only messages (changes to what was said come after it); prompt parts are no entries;
  `Message::modify` keeps the id.
- **But the resolver cannot find the turn by itself.** The context of the first message of a turn
  names a transcript without any message of it; one that ends in a previous turn's messages - an
  agent asked by another, nothing heard in between - would take them for the turn's own.

### Weighed

- **A: the caller names the turn** - `ContextResolver::resolve(reference, turn, agent)`: the turn
  under way (`Turn::turn_id`), or the `turn_id` of the message the context is recorded with, which
  lies next to it in the history. Nothing new is stored. Chosen.
- **B: `ContextRef::turn_id`.** The reference is complete in itself, but every agent message is
  stored with the id twice - in the message and in its context - and the work's identity lies in
  the field that says what its next message is produced from; `ContextRef::default()` would carry a
  nil id unnoticed.
- **Keeping the counts** was only needed to read stored data of an older shape.

### The changes

- `ContextRef::turn_entries` goes; `resolve` takes the turn, and an entry is the turn's own if its
  message carries that turn's id.
- `GivenUp::turn_entries` becomes `GivenUp::turn_id`; `given_up_turns` takes the messages of that
  turn at the end of the branch. The digest of the record gets a scheme of its own
  (`caelumora.transcript-given-up.v2`).
- `retried`: after the message only messages of its turn may follow - 234's definition. `CallsRan`
  and a given-up turn's record (`NotLast`) stay. Internal steps of a later turn, taken back with it
  so far, make it `NotLast`.
- `Work::turn_id` loses the default that gave work stored before a new id; it is the only such one
  from 235.
- AGENT-TURN.md, where it names the counts.

## Done when

Which messages belong to a turn is read by one rule, from one record - or the difference between the
two is stated and kept for a reason.

## History

- **2026-10-06** — Opened from open question 3 of [235](235-retry-of-an-answer-under-way.md).
- **2026-10-06** — Planned with the maintainer: option A, the caller names the turn; stored data of
  an older shape is deleted rather than read.
- **2026-10-06** — Done, as planned. `ContextRef::turn_entries` is gone;
  `ContextResolver::resolve(reference, turn, agent)` counts a message as the turn's own if it
  carries `turn`'s id. The live context passes `Turn::turn_id`; a recorded one the turn of its
  message, which `AgentChannelState::recorded_context_of` now returns with the reference
  (`HistoryEntry::context_of` alike), as a tuple: the two are what `resolve` takes. `GivenUp`
  holds `turn_id` instead of a count - `AgentChannelState::give_up_turn` takes it - and
  `given_up_turns` takes the messages of that turn at the end of the branch; the record's digest
  scheme is `caelumora.transcript-given-up.v2`. `retried` lets only the rest of the message's turn
  follow it; a message without a turn is no answer of the agent's. `Work::turn_id` lost its
  default for stored work. Tests: the resolver's test of the turn's own reasoning passes unchanged
  in what it asserts; a given-up turn takes only its own messages, not an earlier turn's internal
  step before them; the turn is part of the record's digest; internal steps of a later turn make a
  retry `NotLast`; a recorded context comes with its message's turn. AGENT-TURN.md (*What is
  recorded*, *Reasoning given back*, the resolver, the given-up record, answering again),
  CHANNELS.md (*Answering again*).
