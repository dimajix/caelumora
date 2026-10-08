# 260 — A turn keeps its messages apart from the transcript until it ends

**Status:** open
**Area:** Code structure

## The problem

A turn's messages live in `TurnState.entries` until the turn ends; only then do `record_turn` and
`give_up_turn` push them into the agent's transcript and history. Each entry is given the digest it
will have in the transcript ahead of time, and a `TranscriptView` continues the stored transcript
with the turn's entries for whatever reads it during the turn. Since
[258](258-what-was-said-is-lost.md), every message a turn says stays, however the turn ends - so the
reason to keep them apart, that a turn given up should leave the active path untouched, is gone.

## Why it matters

Two places hold what an agent said, and every reader of the agent's context has to put them
together (`TranscriptView`, `Turn::full_context`, the warning in `record_messages` that the
transcript changed during a turn). The working state stored after every step carries the turn's
messages a second time.

## Considerations

- **Push each message into the transcript and history as it is said** (`Turn::say`), and keep in
  `TurnState` only what is staged - notes, memory - and the turn's bookkeeping. The prompt parts a
  turn places among its messages (issue 49) are referred to by the digest of the entry they follow,
  which works on the transcript as well.
- `ContextRef::turn_entries` counts a turn's entries; with the messages carrying their `turn_id`,
  it can be derived (issue 235, open question 3).
- A turn resumed after a restart then finds its messages in the transcript, not in its state.
- Without behaviour change: a restructuring of its own.

## Done when

- A turn's messages are in the agent's transcript and history from the moment they are said, and
  `TurnState` holds no message.

## History

- **2026-10-08** — Opened when closing [258](258-what-was-said-is-lost.md), from part J of its plan.
