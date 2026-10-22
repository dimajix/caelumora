# 448 — A backend of the stores is every state store at once, and the names of the two layers are swapped

**Status:** open
**Type:** refactoring
**Priority:** medium
**Area:** Persistence
**Summary:** The trait `Persistence` inherits `StateStore`, `HistoryStore` and `TranscriptStore` to add two methods, so a backend configured for the entities has to be a state store as well, the runtime holds two `StoreBackend`s of which half is unused each - and the checked layer the runtime uses is called the backend, the backend `Persistence`.

## The problem

Issue [446](446-stores-check-access-each-and-are-enumerated.md) made two layers of the stores: the
backends, which keep without asking anything - `Persistence`, made by the `Backends` -, and
`StoreBackend`, which checks, made of a backend and the policies, and is what the runtime uses.
Two things about them do not fit:

- **A backend is every state store.** `Persistence: StateStore + HistoryStore + TranscriptStore`
  adds `documents` and `open` to the three traits. The configuration names two backends,
  `persistence.entity_store` and `persistence.state_store`, each a `Persistence`; the documents are
  taken from the one, the states from the other. So a backend that keeps only entities - Postgres
  in [03](03-postgres-entity-store.md), before [04](04-postgres-state-store.md) - has to be a state
  store all the same, and `RuntimeBuilder::from_config` makes two `StoreBackend`s, `states` and
  `backend`, of which the one never hands out a document and the other never a state, and both
  carry the policies.
- **The names are swapped.** `StoreBackend` is not a backend but the stores the runtime uses over
  one; `Persistence`, named after the section of the configuration, is the backend.

## Considerations

The three state stores are not independent: `HistoryStore` and `TranscriptStore` read what the
`StateStore` writes - `put_channel_state` the chain of the history, `put_agent_channel_state` the
transcript, the prompt texts and the tool sets. They can be implemented apart, but are taken from
one backend. The entities are independent of them.

Weighed were:

- **Two services of the `Backends`**, one for the documents of every kind, one for a struct of the
  three state stores, and opening as something a plugin registers to be run at start: one
  registration per role, a struct that only bundles, and a mechanism of opening of its own.
- **A struct of the three state stores** handed out by one method of the backend: it bundles what
  only has to come from one backend, which taking all three from the one configured as
  `state_store`, in one place, ensures already.
- **A backend as a factory** - chosen: one trait, which inherits nothing and offers each store by a
  method of its own. A backend is registered once, opens what it keeps, and a backend that cannot
  serve a role refuses it explicitly, which fails the start, naming the store.

## Decided

With the maintainer, on 2026-10-22:

- **`PersistenceBackend`**, the trait of the backends, as the `Backends` make them: `documents(kind,
  document_of)`, `state_store()`, `history_store()`, `transcript_store()` - each fallible - and
  `async fn open()`. It inherits none of the stores; there is no default that refuses a role, a
  backend says so itself.
- **`Persistence`**, the struct in the model that is `StoreBackend` today: the stores the runtime
  uses, made of the backend of the entities, the backend of the states and the policies. It opens
  the checked store of a kind once, hands out the stores of the entities, the components and the
  states, and opens both backends - one, if they are the same. The runtime holds one of it, which
  `RuntimeBuilder::new` takes.

No change of behaviour or of the configuration.

## Steps

1. **The names.** `StoreBackend` becomes `Persistence`, the trait `Persistence` becomes
   `PersistenceBackend`, with the documents and the code that names them.
2. **The backend a factory.** `PersistenceBackend` inherits nothing and offers the three state
   stores by methods; `Persistence` is made of two backends and the policies; the runtime holds
   one.

## Done when

- No trait of a backend of the stores inherits a store.
- The runtime holds one `Persistence`, made of the two backends the configuration names.
- `docs/STORAGE.md` and `docs/ARCHITECTURE.md` say so.

## History

- **2026-10-22** — Opened from the discussion of [03](03-postgres-entity-store.md) with the
  maintainer, as the preparation of a Postgres store of the entities alone.
- **2026-10-22** — Step 1 done: `StoreBackend` is `Persistence`, its field `backend`; the trait of
  the backends is `PersistenceBackend`, registered as `dyn PersistenceBackend`. The code and
  `docs/STORAGE.md` and `docs/ARCHITECTURE.md` name them so; the issues keep the names of their
  time.
