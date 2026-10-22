# 03 — Postgres implementation of the `EntityStore`

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Persistence
**Summary:** `EntityStore` has only in-memory and JSON-file implementations; a Postgres one is the prerequisite for everything multi-process.

## The problem

`EntityStore` — the store for personalities, session and agent templates, and the
`Agent`/`Session`/`User` identities — has two implementations today: `MemoryStore` (tests) and
`JsonFileStore` (a directory of JSON files). Neither survives contact with more than one process or
more than a handful of concurrent writers.

`JsonFileStore` rewrites whole files. There is no transaction, no constraint, and no way for two
runtime instances to share a store.

## Why it matters

This is the first of the three storage issues that has to be solved, because it holds the entities
everything else refers to. It is also the prerequisite for [21](21-scaling.md): a second runtime
process is impossible while identity lives in a local directory.

## Considerations

- The trait boundary already exists and is the right one — this is an implementation, not a
  redesign. Keep the `EntityStore` trait unchanged if at all possible; if it has to change, the
  change should be justified here.
- Every method takes a `SecurityContext` and is gated through the `SecurityManager`. A SQL backend
  tempts one to push filtering into the query; whatever is done must not bypass that gate. See
  [11](11-agent-ownership.md), which will want exactly such filtering-by-owner.
- Schema-versus-document: templates and personalities are nested, evolving structures and are
  natural `jsonb` columns; identities have relations and benefit from real columns. A hybrid is
  likely right.
- Migrations need an owner from the start. `sqlx` with checked-in migration files is the obvious fit
  for the workspace's async stack.

## Done when

- A `PostgresStore` implements `EntityStore` and passes the same test suite as `MemoryStore` and
  `JsonFileStore`.
- Schema migrations are checked in and applied on startup or by a documented command.

## History

- **2026-09-10** — Issue created from `TODO.md`.
- **2026-10-22** — [446](446-stores-check-access-each-and-are-enumerated.md) made the seam a
  Postgres store is built on: a crate of its own whose plugin registers a backend of the stores
  (`dyn Persistence`), keeping each kind's documents in a `DocumentStorage` - by id, with the name
  unique in its scope (`Document::name`) and the mark of a deletion beside the body, which a table
  keeps as columns -, while the checks stay the model's (`CheckedStore`). It runs the store suite
  (`caelumora_model::store::suite`, feature `test-support`) as `caelumora-store-json` does. Still
  open here: a list pushed into the query (the trait has only ids and single reads), the key of a
  name for a unique index, and the secrets the connection needs, which the backends of the stores
  are not yet given (`RuntimeBuilder::from_config` makes them before the `SecretManager`).
