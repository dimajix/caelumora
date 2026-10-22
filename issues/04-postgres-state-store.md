# 04 — Postgres implementation of the `StateStore`

**Status:** in progress
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

## Steps

1. **States and transcripts.** The migration of every table of the states; the four documents,
   written whole, and deleting them; the chains a row per node (`chains`), the transcript and what
   was given written by the put of the agent's state, the history's chain by that of the channel's;
   the `TranscriptStore`. The store suite of the states, the work under way, the transcripts and
   what was given against the database.
2. **Histories.** The items and the memberships, derived by the put of the channel's state; the
   `HistoryStore`; the suite of the histories.
3. **Done.** An agent suspended on an approval survives a restart; the backend `postgres` for the
   states in the documents and `examples/config.yaml`.

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
- **2026-10-22** — Decided with the maintainer, beside 03: the states are kept in tables of their
  own shape - `agent_state`, `agent_channel_state`, `channel_state`, `user_channel_state`,
  `history_entries`, `history_items`, `history_memberships`, `transcript_entries`, and
  `prompt_texts`, `offered_tools` and `frames` written with `ON CONFLICT DO NOTHING` -, each put
  in one transaction. The index of the work under way (388) is a column `has_work_under_way` of
  `agent_channel_state` with a partial index, written with the state, not a table of its own.
- **2026-10-22** — Decided with the maintainer, on how the decision before is built: the
  `StateStore` writes everything, the `HistoryStore` and the `TranscriptStore` only read what its
  puts wrote, so all three are one type, `PostgresStore`, each in a module that owns its tables
  (`states`, `histories`, `transcripts`) and is handed the transaction of the put - as the
  `JsonFileStore` is split. A state's document is written whole by every put, not a column per part
  as 193 foresaw: what grows - the chains, what was given - is kept apart, and a column per part
  would be a second image of every state's shape. The heads of the chains are kept in tables of
  their own, `histories` and `transcripts`; a history's memberships are kept whole, as `jsonb` in
  `histories`, read, taken further by `Memberships::take` and written back in the put - no table
  `history_memberships`, which would build the rule of what a share before a join gives a second
  time in SQL.
