# 04 — Postgres implementation of the `StateStore`

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Persistence
**Summary:** `StateStore` likewise — and it holds the state whose loss is most expensive, including agents suspended on an approval.

## The problem

`StateStore` holds the frequently changing runtime state: an agent's cognitive state, its
`History`, its `MessageContext`, and session state. As with the `EntityStore`
([03](03-postgres-entity-store.md)), only in-memory and JSON-file implementations exist.

The access pattern is different from the entity store's, and harder. Agent state is written on every
turn of the cognitive cycle, message history grows without bound, and the suspend/resume path around
approvals persists state at a point where losing it means losing an in-flight tool call.

## Why it matters

Agent state is the part of the system where a crash is most expensive: an agent that loses its
history resumes as a different agent. A file store makes that a real risk under concurrent access.

## Considerations

- Append-only message history is a better fit than rewriting a serialized blob on each turn — it
  bounds write amplification and makes [06](06-context-in-message-for-auditability.md) cheap to add.
- The `WaitingForApproval` suspend/resume path is the one place that needs a genuine durability
  guarantee. Whether that means a transaction spanning state write and event emission is part of
  [20](20-transactional-effects.md).
- History will need a retention or archival answer eventually. It does not have to be solved here,
  but the schema should not preclude it. See [16](16-context-compression.md).

## Done when

- A Postgres `StateStore` passes the existing store test suite.
- An agent's history and context survive a runtime restart mid-session, including one suspended on
  an approval.

## History

- **2026-09-10** — Issue created from `TODO.md`.
- **2026-10-02** — Issue [193](193-external-state-rewritten-whole.md) decided the shape this store
  inherits: every state is `Journaled`, its changes one enum with a variant per part, a chain's
  as a `Stored<E>` - the nodes new or changed in their place, and the head - the small parts
  whole. A Postgres `put` is a `match` over that enum: a node is a row upserted by its digest
  (digest, parent, entry, and the branch remembered where it is not the newest), the head and a
  small part a column of the state's row; a `get` reads the rows in the order they were added and
  builds the chain through `Chain::apply`. The suite of `store_impl::suite` checks the contract.
- **2026-10-22** — [446](446-stores-check-access-each-and-are-enumerated.md): the states are kept
  by the same backend as in 03 (`Persistence`, which is `StateStore + HistoryStore +
  TranscriptStore`); the rules a store applies in memory - folding a chain, what a reader sees, a
  page - are public functions of the model for a store that does not translate them into a query.
