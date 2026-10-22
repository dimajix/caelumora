# 447 — The seam of the stores is not fit for a database

**Status:** done
**Type:** refactoring
**Priority:** medium
**Area:** Persistence
**Summary:** A storage of documents is listed by reading every id and then each document on its own, compares names as values only it can hold, and a backend cannot open asynchronously or reach a secret, so a database behind the seam of issue 446 would read a kind row by row, could keep no unique index of names and could not connect.

## The problem

Issue [446](446-stores-check-access-each-and-are-enumerated.md) put every backend of the stores
behind one seam: a `DocumentStorage` per kind, under the one `CheckedStore`, and a `Persistence`
made by the `Backends`. Three things about that seam fit the memory and the JSON files, but not a
database ([03](03-postgres-entity-store.md), [04](04-postgres-state-store.md)):

- **Lists read one document at a time.** `DocumentStorage` offers `ids()` and `get(id)`;
  `CheckedStore::page` sorts the ids, reads the documents one by one, judges the filter and asks the
  policy until the page is full. In a database that is one query for every id of the kind and one
  per document. `find_by_name` lists every document of the kind to find one.
- **Names are compared as values.** A `Document` carries its name as a `ScopedName`, which is
  compared by its scope and its folded name (`Name`'s key, private). A database keeps a name unique
  by a unique index over a column, which needs the name as one value it can compare - and the rule
  how it is formed is the model's, not every backend's.
- **A backend is made synchronously and before the secrets.** A factory of the `Backends` is a
  plain function, `RuntimeBuilder::from_config` is synchronous and makes the stores before the
  `SecretManager`. A database connects, applies its migrations and resolves the secret its
  connection is named by - all asynchronously, and the last with the secrets at hand.

## Decided

With the maintainer, on 2026-10-22, while discussing 03:

- **Lists: a scan in batches.** `DocumentStorage::scan` hands out documents in the order of their
  ids, as `str` orders them (in Postgres `COLLATE "C"`), after a cursor, up to a number, among what
  is deleted or not, and meeting a filter - or more: a storage may hand out a superset, never less,
  and the `CheckedStore` judges every document against the whole filter again before it asks the
  policy, reading the next batch until the page is full. So a database translates what it can of a
  filter and leaves the rest - `NamePrefix` and `Contains`, which compare as names are folded - to
  the store. The filter crosses the seam without the kind's field type, by the paths of its fields.
  A document a storage cannot read as it is kept - a file of the JSON store that holds no JSON - is
  left out and logged, as one that cannot be read as its type is, so that one broken document does
  not break a list; any other failure of the storage fails the scan. `find_by_name` asks the
  storage for the document of a name (`named`), and then the policy.
- **Names: a key made by the model.** `ScopedName::key()` is the one form a name is compared by as
  a single value - its scope and its folded name -, equal exactly where the names are. A `Document`
  keeps its `ScopedName`; a storage that needs one value takes the key, so that a database keeps
  names unique by a partial unique index (`WHERE NOT deleted`). The rule of
  issue [250](250-unique-names-per-name-space.md) stays as it is, for every kind. A key depends on
  how names are folded; should that change, the stored keys are stale, and stored data is deleted.
- **Backends: opened asynchronously, with the secrets at hand.** `Persistence` gets an
  `async fn open(&self)`, which does nothing unless a backend says otherwise.
  `RuntimeBuilder::from_config` becomes `async`: it makes the secrets first - they need only the
  policies -, then the backends of the stores, which may ask for the `SecretManager` among the
  services they are given, then registers the plugins - which open the stores of their kinds -, and
  then opens the backends, so that a database creates what the kinds opened need, applies its
  migrations and connects before anything runs, and a wrong secret fails at start.

For 03 itself it was decided at the same time: one table per kind, named `entity_<kind>`, each
with the columns `id`, `name_key`, `deleted` and `body` (`jsonb`); see 03.

## Steps

1. **Names by key.** `ScopedName::key()`, for a storage that needs a name as one value.
2. **Lists by scan.** `DocumentStorage::scan` and `named` in place of `ids`; `CheckedStore` reads
   in batches and judges again; the filter by paths of fields.
3. **Backends opened asynchronously.** `Persistence::open`, `RuntimeBuilder::from_config` `async`,
   the secrets before the stores and offered to their backends.

Each without a change of behaviour.

## Done when

- A storage is listed in batches, and a name is found by its key.
- The key of a name is made in the model alone.
- A backend of the stores is opened asynchronously at start, after the plugins registered, and can
  reach the secrets.
- `docs/STORAGE.md` and `docs/ARCHITECTURE.md` say so.

## History

- **2026-10-22** — Opened from the discussion of 03, 04 and 26 with the maintainer, as the
  preparation of a Postgres store behind the seam of 446.
- **2026-10-22** — Step 1 done: `ScopedName::key()` - the scope's key (`Scope::key`, its variant
  and ids) and the folded name, apart by a line break. A test holds that two scoped names have one
  key exactly if they are equal. Decided with the maintainer, in place of a `Document` that carries
  the key as its name: a string would lose what the name is; the memory and the JSON storage keep
  comparing `ScopedName`s by `==`, and only a storage that needs one value - a column of a unique
  index - takes the key.
- **2026-10-22** — Step 2 done: `DocumentStorage::scan` and `named` in place of `ids`; the filter
  crosses the seam as `Filter<FieldPath>` (`Filter::by_path`, `FieldPath` the path of a field, no
  type of its own); `CheckedStore::page` reads batches of `SCAN_BATCH` (100), judges each document
  against the whole filter, reads it as its type and asks the policy, and `find_by_name` asks for
  the document of the name, after `List` as before. `Access::page` is gone - the store orders the
  steps, `Access` answers the questions -, and `query::ordered_after` takes a key and is public,
  for the storages. The memory storage judges filter and mark; the JSON storage the filter alone,
  since it would have to read a document as its type to tell the mark. Observable: a page that
  stops at its limit hands out no cursor where the scan showed nothing follows (before: whenever
  any document of the kind followed); a list fails where the storage fails - a file of the JSON
  store that cannot be read, not one that holds no JSON, which is left out as before; and finding
  by name asks the policy of the one document of the name alone, not of every one of the kind,
  which the security suite now states. A file that holds no JSON is taken as a document the
  storage cannot read as it is kept, left out and logged, not as a failure of the storage, so that
  one broken file does not break a list.
- **2026-10-22** — Step 3 done, and with it the issue: `Persistence::open`, asynchronous, nothing
  unless a backend says otherwise, reached through `StoreBackend::open`.
  `RuntimeBuilder::from_config` is `async`: the secrets first, then the backends of the stores,
  given the policies and the `SecretManager` as their services, then the plugins, then both backends
  opened, a failure named by the store (`Cannot open the entity store`). Both programs await it; the
  tests that build from a configuration are asynchronous. A runtime made by `RuntimeBuilder::new` is
  not opened by it - its stores are its maker's, as in the tests, whose memory needs nothing. A test
  holds that a backend of the stores is made with the secrets at hand; that `open` runs after the
  plugins registered holds by the order of `from_config` alone, untested until a backend does
  something in it (03).

