# 93 — The `EntityStore` speaks three CRUD dialects

**Status:** open
**Area:** Persistence

## The problem

`caelumora-model/src/store.rs:31-77`: personalities, templates and
components have `put` (upsert) + `delete` + `list`; agents have
`create`/`update` and no `list`/`delete`; sessions and channels have
`create`/`update`/`delete`; users have `create`/`update`/`find` and no
`list`/`delete`. Session templates are stored by **name** (`memory.rs:19/165`,
`json.rs:113`) but deleted by **id** with a full scan (`json.rs:366-378`),
and there is no `get_session_template(id)`. `find_*_by_name` exists three
times in two constructions (`memory.rs:101-110` vs. `195-206`); the
`Find*` variants of `ActionType` are always allowed by the ownership policy.

In the JSON store, `read_json_opt::<Persisted<T>>(&path).await?.map(|p| p.value)`
appears 24 times, `Persisted { schema_version: 1, … }` 18 times, and
`schema_version` is never read; `create_*` checks `path.exists()`
synchronously between `tokio::fs` calls. `get_session_state` returns
`Default` in the JSON store and `NotFound` in the memory store, and
`SessionState` is neither read nor written anywhere.

## Why it matters

Issue [03](03-postgres-entity-store.md) adds a third implementation. A
shared test suite that all three pass is only possible with one dialect;
today every kind needs its own tests, and a Postgres store would inherit
seven small inconsistencies.

## Considerations

- One dialect for all seven kinds: `create/get/update/delete/list`, all by
  id. `find_by_name` for all or for none (callers can filter `list`;
  `Scope::contains` is used only there). The `Find*` action types go, which
  suits the total mapping `ActionType → (Kind, Verb)` that issue
  [23](23-acls-in-security-attributes.md) wants.
- `read_entity<T>`/`write_entity` helpers in the JSON store; `schema_version`
  checked or removed. `SessionState` and its four `StateStore` methods
  removed.
- Two implementations × ~7 methods, ~10 callers in runtime, REST and TUI.

## Done when

- Every entity kind has the same store methods.
- One store test suite runs against every implementation.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 3.2.3 and 3.2.10).
