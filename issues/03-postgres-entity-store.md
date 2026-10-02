# 03 — Postgres implementation of the `EntityStore`

**Status:** open
**Area:** Persistence

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
