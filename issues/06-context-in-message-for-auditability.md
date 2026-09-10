# 06 — Persist the context a message was generated from

**Status:** open
**Area:** Persistence / Audit

## The problem

An agent's reply is produced from a `MessageContext` assembled by the context
providers for that turn. The reply is persisted; the context it was built from
is not — the context is mutable state that the next turn overwrites.

So for any past message there is no way to answer "what did the agent actually
see when it wrote this?".

## Why it matters

This is the single most common question when an agent behaves oddly, and the
one the system currently cannot answer. It also matters for anything
resembling an audit trail: an approval decision made on the basis of a tool
call is only reviewable if the state that produced the tool call is
recoverable.

The cost is that context is large and largely redundant between consecutive
turns.

## Considerations

- Embedding the full context in every message is the simple answer and is
  probably too expensive at any real message volume — the context is dominated
  by history that is already stored once.
- A reference is cheaper than a copy: store a context *snapshot id* on the
  message and the snapshot separately, deduplicated. This interacts directly
  with [16](16-context-compression.md), which will make the context a
  derived artifact with its own lifecycle.
- Overlaps with [05](05-postgres-event-recorder.md): if the envelope stream is
  recorded, part of this is answered by replay rather than by storage. Decide
  which of the two carries the burden before building both.
- Whatever is stored inherits the sensitivity of the context — it may contain
  tool output and secrets-adjacent material.

## Done when

- For any persisted agent message, the context that produced it can be
  reconstructed.
- The storage cost of doing so is bounded and understood.

## History

- **2026-09-10** — Issue created from `TODO.md`, where it was phrased as
  *"Put 'context' into message for auditability?"* — the question mark is
  part of it: the design is not settled.
