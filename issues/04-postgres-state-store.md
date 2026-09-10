# 04 — Postgres implementation of the `StateStore`

**Status:** open
**Area:** Persistence

## The problem

`StateStore` holds the frequently changing runtime state: an agent's cognitive
state, its `MessageHistory`, its `MessageContext`, and session state. As with
the `EntityStore` ([03](03-postgres-entity-store.md)), only in-memory and
JSON-file implementations exist.

The access pattern is different from the entity store's, and harder. Agent
state is written on every turn of the cognitive cycle, message history grows
without bound, and the suspend/resume path around approvals persists state at
a point where losing it means losing an in-flight tool call.

## Why it matters

Agent state is the part of the system where a crash is most expensive: an
agent that loses its history resumes as a different agent. A file store makes
that a real risk under concurrent access.

## Considerations

- Append-only message history is a better fit than rewriting a serialized blob
  on each turn — it bounds write amplification and makes
  [06](06-context-in-message-for-auditability.md) cheap to add.
- The `WaitingForApproval` suspend/resume path is the one place that needs a
  genuine durability guarantee. Whether that means a transaction spanning
  state write and event emission is part of
  [20](20-transactional-effects.md).
- History will need a retention or archival answer eventually. It does not
  have to be solved here, but the schema should not preclude it. See
  [16](16-context-compression.md).

## Done when

- A Postgres `StateStore` passes the existing store test suite.
- An agent's history and context survive a runtime restart mid-session,
  including one suspended on an approval.

## History

- **2026-09-10** — Issue created from `TODO.md`.
