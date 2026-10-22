# 446 — Every store checks access itself and is opened by a closed enum, so no store can live in a crate of its own

**Status:** in progress
**Type:** refactoring
**Priority:** medium
**Area:** Persistence
**Summary:** The memory and the JSON store each implement every operation on entities with its access checks, and the model opens them by a closed enum, so a third store - Postgres - would repeat the checks and could not live in a crate of its own.

## The problem

There are two stores, `MemoryStore` and `JsonFileStore`, both in `caelumora-model`. Each of them
implements `EntityStore<T>` for every kind on its own (`MemoryEntityStore<T>`,
`JsonEntityStore<T>`): it reads what is stored, asks `store::access::Access` - the access policy,
the clearance, the mark of a deletion -, checks that a name is free (`name_free`) and writes. The
order of these steps, which of them run under which lock and what is answered when, is written
twice, around the same helpers, which are `pub(crate)`. Each store also holds the policies and
caches the stores of the kinds it opened, as `StoreBackend` does once more above them.

The model opens the stores itself: `PersistenceBackend`, a closed `enum` of the configuration, and
`StoreBackend`, which holds a closed `enum Backend { Memory, Json }` and matches on it for every
face it hands out. Issue [324](324-backends-enumerated-in-the-core.md) opened every other backend
through `caelumora_runtime::backend::Backends`, and decided that the persistence stays closed: its
stores are generic over the kinds they keep (`EntityStoreFactory::entity_store<T>`), so a factory
registered by type could not open them.

## Why it matters

- A Postgres store ([03](03-postgres-entity-store.md), [04](04-postgres-state-store.md)) would be
  the third copy of the entity operations, with the checks wired around its queries once more - or
  `sqlx` would be built into the model and into everything that uses it.
- Neither store can be moved into a crate of its own: the model names both, and a crate outside it
  reaches neither `Access` nor `name_free` nor the store suite. Making them public would hand every
  store the checks to wire up itself - logic the owner should provide.
- The reason 324 gave for keeping the persistence closed is a consequence of where the checks sit,
  not of the persistence: what a backend has to do for a kind does not depend on the kind's type.

## Considerations

### One `EntityStore` over a storage without types

The checks belong to the model, once, and what a backend does for a kind is to keep documents:

- **`DocumentStorage`** - the documents of one kind as a backend keeps them, serialized
  (`serde_json::Value`), without asking anything, as an `ArtifactStorage` keeps content: read one,
  list the ids, insert, replace, remove. It is not generic, so a backend opens it for a kind named
  by its `EntityKind`, and is a trait object.
- **One `EntityStore<T>`** in the model, over a `DocumentStorage`: it serializes and reads back,
  asks `Access` and orders the steps - for every kind and every backend alike. `MemoryEntityStore`
  and `JsonEntityStore` are gone; `Access` and `name_free` stay `pub(crate)`. It also serves the
  preconfigured components (`ComponentStore`), which are kept the same way and removed for good.
- **Names unique in their scope** are kept by the storage, in the step that writes: the store hands
  it the key of the name with the document (none for a deleted one), and the storage refuses one
  another document has (`NameTaken`). In memory that is under the write lock, in the JSON files
  under the kind's lock, in Postgres a unique index - so that a database does not read every
  document of a kind for every write, and the check holds across processes too. A storage that
  does not keep the keys beside the documents - the JSON files - is given, when it is opened, how
  to read the key and the mark of a deletion from a document (the kind's type is known only
  there).
- **Removing for good only what is marked** (`purge`) is the storage's too, in the same step, for
  the same reason.
- **Lists** stay as they are: the ids in order, each document read when it is its turn, filtered by
  the query and then by `Access`. A storage that can do more - Postgres, a `WHERE` and a limit -
  is 03's and [332](332-policy-as-a-predicate.md)'s to add to the trait; the trait is the
  workspace's own and changes with them.

The memory store then keeps documents serialized, as the JSON store does, instead of the values
themselves: every read deserializes, and what does not survive a round trip through serde is lost
in memory as it is in the files. That is the point - the two can no longer differ there - and the
cost in tests is small.

### The policies belong to the store, not to the backend

A backend keeps; the `StoreBackend` checks. It is made of a backend and the two policies, opens the
`EntityStore<T>` of a kind once and hands the same one out again, and refuses a kind whose name
another kind has - all of which it does already, beside the backends doing it as well. The backends
lose their policies, their caches of kinds and `EntityStoreFactory`.

Tests use the memory store as the store of everything - entities, components, states - almost four
hundred times, `MemoryStore::new()` with the policy that allows everything, a few with a policy of
their own. They go through `StoreBackend` too (`StoreBackend::in_memory()`, or `StoreBackend::new`
with a memory store and policies): a mechanical change of many tests, without a second way to make
a checked store.

### What a backend is

A backend - the memory, the JSON files, later Postgres - is one trait object, `Persistence`, named
after the section of the configuration: the storages of the kinds (`documents(kind)`), and the
states, histories and transcripts (`StateStore + HistoryStore + TranscriptStore`, which it is). The
state side keeps its three traits as they are: what it needs of the model besides - folding a chain,
deciding what a reader sees, paging - are functions of the model that evaluate its rules in memory,
which a store that does not translate them into a query needs, and which are made public with the
move of 2, not before.

### Opened by the `Backends`

With `Persistence` a trait object, the reason of 324 is gone: the configuration names a backend by
`type` with its parameters, a `BackendConfig` like every other one - the YAML stays as it is
(`type: json`, `path`, `journal_floor`) -, and the factory registered for it makes the
`Persistence`; the runtime makes the `StoreBackend` of it with the policies. `PersistenceBackend`,
`AppConfig::open_entity_store` and `open_state_store` are gone.

### The JSON store in a crate of its own

`caelumora-store-json`, with `JsonFileStore`, which implements `Persistence` and nothing else, and a
plugin that registers the backend `json` (`Plugin::backends`), as `caelumora-open-terminal`
registers its storage. The memory store stays in the model: every test of every crate uses it, and
it needs nothing outside the model. The store suite becomes public behind a feature
`test-support`, so that every backend's crate runs it, Postgres too. What the binaries register is
theirs: `caelumora-app` brings the plugin.

## Steps

1. **The seam.** `DocumentStorage` and the one `EntityStore<T>` over it; the memory and the JSON
   store keep documents and states only; `StoreBackend` holds a `Persistence` and the policies;
   the tests make their stores through it. No change of behaviour.
2. **Opened by the `Backends`.** `Persistence` is a service of the `Backends`, `memory` and `json`
   registered by the core; `PersistenceBackend` is a `BackendConfig`. No change of behaviour or of
   the configuration.
3. **`caelumora-store-json`.** The JSON store moves, with its plugin; the suite and the functions
   of the state side it needs become public. No change of behaviour or of the configuration.

## Done when

- The access checks of the entity stores are written once, in the model, and no backend holds or
  asks a policy.
- The stores are opened through the `Backends`; the model names no backend but the memory store.
- `JsonFileStore` lives in `caelumora-store-json` and runs the store suite there.
- `docs/STORAGE.md`, `docs/ARCHITECTURE.md` and `docs/CONFIGURATION.md` say so.

## History

- **2026-10-22** — Opened while looking at [03](03-postgres-entity-store.md),
  [04](04-postgres-state-store.md) and [26](26-database-user-backend.md) with the maintainer, as
  the preparation of a Postgres store, decided as option B there: the seam first, so that the JSON
  store - and later Postgres - is a backend of plain documents and states, and the checks stay the
  model's. Moving the JSON store first was weighed and set aside: it would have needed `Access` and
  `name_free` public, to be taken back with the seam. Revisits the decision of 324 that the
  persistence stays a closed enum, whose reason - stores generic over their kinds - the seam
  removes.
- **2026-10-22** — Step 1 done: `store::documents` holds `Document`, `DocumentStorage` and the one
  `CheckedStore<T>`, which serves every kind's `EntityStore` and the `ComponentStore`; `name_free`
  is gone from `store::access`, the rule of names is the storages', in the step that writes. The
  memory and the JSON store are backends of the trait `Persistence` - `documents(kind,
  document_of)` beside the three traits of the states - and hold no policy, no cache of kinds and
  no `EntityStoreFactory` any more; `MemoryStore::with_policies` is gone. `StoreBackend` is a
  `Persistence` and the policies (`StoreBackend::new`, `StoreBackend::in_memory`), opens each kind
  once and gives the faces by upcasting; `component_store` can fail now. `PersistenceBackend::open`
  opens the stores of the core's kinds and the components at once, as `JsonFileStore::new` did.
  The tests make their stores through `StoreBackend`. Observable besides: the JSON files of
  entities are written from the document as `serde_json::Value`, so their keys are sorted; what is
  read is the same, and nothing stored has to be deleted.
- **2026-10-22** — Step 2 done: `dyn Persistence` is a service of the `Backends`; the core
  registers `memory` and `json`, whose parameters are `JsonStoreConfig` beside the JSON store.
  `persistence.entity_store` and `state_store` are `BackendConfig`s, `memory` unless configured;
  `PersistenceBackend`, `AppConfig::open_entity_store`, `open_state_store` and `ConfigError::Store`
  are gone. `RuntimeBuilder::from_config` makes the two backends first (`Backends::make_one`, new
  beside `make_named` and `make_chain`) and a `StoreBackend` of each with the policies. The YAML is
  unchanged; what changed is when a wrong one is refused: an unknown type or parameter of a store is
  refused as the runtime is built, naming the store - `the entity store (json)` -, as for every
  backend, no longer as the file is read.
