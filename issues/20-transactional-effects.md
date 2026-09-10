# 20 — Transactional effects

**Status:** open
**Area:** Open architecture questions

## The question

A turn of the agent cognitive cycle does several things that ought to happen
together or not at all: it persists agent state, it executes tool calls, it
emits events, and it appends to history. Today each of those happens
independently. There is no boundary around them and no defined behaviour if
the process dies in the middle.

The concrete failure: an agent suspends on an approval, persists its pending
tool calls, is granted approval, executes the tool — and crashes before
recording the result. On restart it re-executes a tool that has already run.

## Why it matters

Tool effects are the part of the system that reaches outside it. A duplicated
message send or a duplicated write to an external service is not something the
runtime can undo, and it is the one class of bug where "it recovered
gracefully" is not good enough.

The question also gates [08](08-retry-and-edit-past-answers.md): rewinding a
conversation is only coherent if there is a defined notion of what a turn
did.

## Considerations

- A database transaction spanning state, history and event emission is
  achievable once [04](04-postgres-state-store.md) and
  [05](05-postgres-event-recorder.md) exist and share a backend. It covers
  everything except the tool effect itself, which is the part that matters
  most.
- External effects cannot be rolled back, so the realistic goal is
  **at-most-once** or **idempotent** execution, not atomicity. That likely
  means a durable record of "this tool call was dispatched" written *before*
  dispatch, and an idempotency key that tools can honour — which pushes a
  requirement onto every tool, including remote MCP tools that may not
  cooperate.
- The alternative is to accept at-least-once and make it visible: record the
  ambiguity and surface it, rather than pretending it did not happen.
- The scope of a transaction needs deciding: one tool call, one cognitive
  turn, or one envelope. They give different guarantees and different costs.
- This is a question, not a task. The output of the first pass should be a
  written decision here, not code.

## Done when

- The chosen consistency guarantee is stated here, together with what it
  explicitly does not cover.
- The crash-during-tool-execution case has a defined, documented outcome.

## History

- **2026-09-10** — Issue created from `TODO.md`, where it appears under
  *"BIG questions"*.
