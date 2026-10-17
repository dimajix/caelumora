# 20 — Transactional effects

**Status:** rejected
**Type:** question
**Area:** Persistence
**Summary:** A cognitive turn persists state, runs tools and emits events with no boundary around them; a crash mid-tool re-executes it on restart.

## The question

A turn of the agent cognitive cycle does several things that ought to happen together or not at all:
it persists agent state, it executes tool calls, it emits events, and it appends to history. Today
each of those happens independently. There is no boundary around them and no defined behaviour if
the process dies in the middle.

The concrete failure: an agent suspends on an approval, persists its pending tool calls, is granted
approval, executes the tool — and crashes before recording the result. On restart it re-executes a
tool that has already run.

## Why it matters

Tool effects are the part of the system that reaches outside it. A duplicated message send or a
duplicated write to an external service is not something the runtime can undo, and it is the one
class of bug where "it recovered gracefully" is not good enough.

The question also gates [08](08-retry-and-edit-past-answers.md): rewinding a conversation is only
coherent if there is a defined notion of what a turn did.

## Considerations

- A database transaction spanning state, history and event emission is achievable once
  [04](04-postgres-state-store.md) and [05](05-postgres-event-recorder.md) exist and share a
  backend. It covers everything except the tool effect itself, which is the part that matters most.
- External effects cannot be rolled back, so the realistic goal is **at-most-once** or
  **idempotent** execution, not atomicity. That likely means a durable record of "this tool call was
  dispatched" written *before* dispatch, and an idempotency key that tools can honour — which pushes
  a requirement onto every tool, including remote MCP tools that may not cooperate.
- The alternative is to accept at-least-once and make it visible: record the ambiguity and surface
  it, rather than pretending it did not happen.
- The scope of a transaction needs deciding: one tool call, one cognitive turn, or one envelope.
  They give different guarantees and different costs.
- This is a question, not a task. The output of the first pass should be a written decision here,
  not code.

## Done when

- The chosen consistency guarantee is stated here, together with what it explicitly does not cover.
- The crash-during-tool-execution case has a defined, documented outcome.

## History

- **2026-09-10** — Issue created from `TODO.md`, where it appears under *"BIG questions"*.
- **2026-09-25** — Two things added by the branching plan in [08](08-retry-and-edit-past-answers.md)
  (principles in [19](19-memory-tiers.md)): effects of an abandoned branch are carried into the new
  branch as facts, which needs tools to declare whether they have an effect or only read, the same
  distinction idempotency needs here. And two more kinds of effect leave the reversible zone:
  another channel reading a channel's notes, and committing knowledge beyond the session
  ([18](18-knowledge-sharing.md)).
- **2026-09-30** — A turn that fails or is stopped no longer loses its tool calls
  ([145](145-ended-turns-leave-a-record.md)): they stay in the agent's transcript on a dead branch,
  and the calls that ran are told on the active branch. What is told as having run is what has a
  `started_at`; the call a crash cut off (`prepare_resume`) has none and stays the open case of this
  issue.
- **2026-10-15** — Rejected by the maintainer: a conceptual question that is not being answered at
  present. What holds until then is what the runtime does today - a dropped or interrupted call
  counts as having run, and the model is told to check its effect ([TOOLS.md](../docs/TOOLS.md));
  the call a crash cut off stays without a defined outcome. That tools declare whether they have an
  effect or only read, which branching and idempotency would both need, goes with this issue; it
  becomes an issue of its own when something needs it - the first step of
  [226](226-concurrent-tool-calls.md), say, which changes what a tool returns.
