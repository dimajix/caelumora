# 16 — Context compression

**Status:** open
**Area:** Context & knowledge

## The problem

`MessageHistory` grows for as long as a session lives, and the context
providers assemble a `MessageContext` from it on every turn. Nothing bounds
either. A long-running session eventually exceeds the model's context window,
and there is no strategy for what to drop or how to summarise before it does.

## Why it matters

This is the hard limit on session length. Everything else about a long-running
agent — memory, knowledge sharing, a session that spans days — presumes an
answer to this one.

It is also a cost issue well before it is a correctness issue: an unbounded
context is re-sent on every turn.

## Considerations

- The mechanism most likely already exists in the right shape. Context is
  assembled by components implementing `ContextProvider` / `PlanPreProcessor`,
  so compression is plausibly a component in that chain rather than a new
  subsystem — consistent with how the architecture handles composition
  elsewhere.
- The strategies differ in what they destroy:
  - **Truncation** — drop the oldest. Cheap, predictable, loses the beginning
    of the conversation, which is often where the task was defined.
  - **Summarisation** — replace old turns with a model-generated summary.
    Preserves gist, costs a model call, and introduces a lossy artifact that
    subsequent turns cannot distinguish from ground truth.
  - **Retrieval** — keep everything, select what is relevant per turn. Most
    capable, most machinery, and closest to [18](18-knowledge-sharing.md).
- Compression must not mutate the stored history — the record and the working
  context are different things. See
  [06](06-context-in-message-for-auditability.md), which needs the
  *uncompressed* truth to remain reachable.
- Tool results are frequently the bulk of a context and the least useful to
  keep verbatim; they may warrant their own rule.

## Done when

- A session can run past the model's context window without failing.
- The chosen strategy is pluggable per agent rather than hard-coded.

## History

- **2026-09-10** — Issue created from `TODO.md`.
- **2026-09-25** — Constraint from [06](06-context-in-message-for-auditability.md),
  which made the context a hash chain the history refers to: compaction
  must not rewrite the existing context. It produces a new context from
  it; history entries written before keep referring to the old one, which
  must stay resolvable, and only messages produced afterwards refer to the
  new one. The new context's origin is recorded as a relation of its own
  (not the chain's predecessor link, not part of its digest). Details in
  06's history.
