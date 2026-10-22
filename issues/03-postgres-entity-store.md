# 03 — Postgres implementation of the `EntityStore`

**Status:** done
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

## Steps

1. **The crate.** `caelumora-store-postgres` with its plugin, the backend `postgres`, its
   configuration, the pool per secret and schema, the migrations, the tables of the kinds and the
   storage of the documents - every filter judged by the store alone; the store suite of the
   entities against the database.
2. **Filters in the query.** What a filter can be translated into is asked of the database - at
   least what is asked, never less.
3. **In the programs.** `caelumora-app` and `caelumora-tui` bring the plugin; the documents.

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
- **2026-10-22** — Decided with the maintainer: the store is a crate of its own whose plugin
  registers the backend `postgres`, behind the seam of 446 as prepared by
  [447](447-store-seam-not-fit-for-a-database.md). **One table per kind**, `entity_<kind>` - the
  prefix keeps a kind clear of the words of SQL (`user`, `group`) -, each with the columns `id`
  (`text`, ordered `COLLATE "C"`), `name_key`, `deleted` and `body` (`jsonb`), a partial unique
  index on `name_key WHERE NOT deleted` and a GIN index on `body`; a subtype within a kind - a
  personality internal or external, a group a team or a session's participants - is a field of the
  document, not a table. The tables of the core's kinds are made by the migrations, those of a
  plugin's kinds when the backend is opened; changes to their common shape are migrated in code,
  over every table, tracked in `kinds(name, table, schema_version)`. Names stay unique as 250
  decided, for every kind; the column `name_key` holds the key the model makes of a document's
  name (`ScopedName::key`). Lists are scanned in batches - a storage may hand out more than a
  filter asks, the store judges again -, and the connection is named by a secret, resolved when
  the backend is opened at start; one pool per secret, shared by every backend of the plugin, the
  user backend of 26 among them.
- **2026-10-22** — [448](448-persistence-inherits-the-state-stores.md) made a backend of the stores
  a factory that inherits none of them (`PersistenceBackend`), so that the backend `postgres` keeps
  the entities alone and refuses the state stores (`state_store`, `history_store`,
  `transcript_store`) until [04](04-postgres-state-store.md).
- **2026-10-22** — Decided with the maintainer, revisiting the decision above: **every table of a
  kind is made in code**, when the backend is opened or the kind first used, and entered in
  `kinds(name, table_name, schema_version)` - the core's kinds as a plugin's, since they all have
  one form; the migrations checked in (`sqlx`) make the tables of their own, `kinds` among them.
  A plugin registers no migrations for its kinds: it would bring SQL of a backend it does not know,
  the same for every kind; one that brings a Postgres backend of a service of its own may, once
  there is one. The configuration names the secret of the connection's URL (`url_secret`, as
  `auth.jwt_secret` names its key) and optionally a `schema` - its tables there, as the
  `search_path` of its connections - and `max_connections`; one pool per secret and schema. Two
  processes that change one entity at once are
  [450](450-store-overwrites-a-change-made-meanwhile.md)'s, not solved here. The tests run against
  the database `test/docker-compose.yml` starts, named by `CAELUMORA_TEST_POSTGRES_URL`, each in a
  schema of its own; without it they are skipped.

- **2026-10-22** — Step 1 done: the crate `caelumora-store-postgres`, with `sqlx` 0.9 (queries
  checked at run time, migrations compiled in), its plugin `PostgresStorePlugin` registering the
  backend `postgres` (`url_secret`, `schema`, `max_connections`) and `PostgresStore`, which keeps
  the entities alone and refuses the state stores. A `Connection` per secret and schema, shared by
  the plugin's backends of one runtime, resolves the secret, connects with the schema as its
  `search_path`, makes the schema and applies the migration of `kinds` - once, when first needed,
  which `open` is. `PgDocuments` makes the table of its kind, under an advisory lock of the kind,
  unless `kinds` records it, and refuses one of another form (`KIND_TABLE_VERSION`); a unique
  violation of the index of the names is `NameTaken`, of the id `Conflict`; `replace` writes as an
  upsert, as the memory does. A scan hands out every document of the mark after the cursor, the
  filter judged by the store alone (step 2). Since sqlx 0.9 takes a query built at run time only
  as audited (`AssertSqlSafe`), they are made by one function (`sql`) of SQL and identifiers quoted
  by `quoted`, every value bound. The tests run the store suite of the entities, and that two
  backends of one schema - as two processes - share their documents and their names, that a table
  of another form and a secret that names nothing fail the opening, and that the backend is made
  from its parameters.
- **2026-10-22** — Step 2 done: a scan asks the database what it can judge of a filter
  (`Condition`). Each condition is translated as `Filter::matches` judges it, of a document as serde
  writes it - which every document in a table is, since the store alone writes them -, a field that
  is not there made `false` (`COALESCE`), so that one negated holds; an element of a collection by
  `EXISTS` over `jsonb_array_elements`; what compares names folded (`NamePrefix`, `Contains`) is
  `true`, and so is the negation of what holds for more besides, its parameters taken back. What
  can be asked as containment is asked so besides (`body @> …`), which the GIN index answers. A
  test holds, for documents and filters of every form, that a scan hands out what a filter meets,
  and exactly that unless it compares names folded.
- **2026-10-22** — Step 3 done, and with it the issue: `caelumora-app` and `caelumora-tui` list
  `PostgresStorePlugin` (`caelumora_store_postgres::plugin`, as the JSON store's). A test builds a
  runtime from its configuration - the secrets from the environment, the entities in Postgres, the
  states in memory - and finds the tables of the core's kinds made at start and an entity in its
  table. `docs/STORAGE.md` (*In Postgres*), `docs/CONFIGURATION.md` and `docs/ARCHITECTURE.md` say
  so. Of what was to be done: `PostgresStore` is a backend of the stores rather than an
  `EntityStore`, since 446 the model's alone, and passes the store suite of the entities; the
  migrations are checked in and applied when the backend is opened at start.
