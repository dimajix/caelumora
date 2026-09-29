# 93 — The `EntityStore` speaks three CRUD dialects

**Status:** in progress
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
- **2026-09-29** — Agreed with the maintainer, in four steps: (1) remove
  `SessionState` and the JSON store's wrapper; (2) one dialect -
  `create/get/update/delete/list` by id for all seven kinds, no upsert,
  session templates by id; (3) find by name only for users, whose
  username is an identity - the other callers filter `list_x`, and the
  `Find*` actions go; (4) one store test suite for both stores.
- **2026-09-29** — Step 1: `SessionState` and `get/put/delete_session_state`
  are gone from the model, both stores and the runtime. The JSON store
  writes documents as they are; the `Persisted` wrapper with its
  `schema_version` (never read) and `persisted_at` is gone, so
  `read_json_opt`/`write_json_atomic` are the read and write helpers the
  considerations asked for. Files written with the wrapper still load:
  the fields were flattened in, and unknown fields are ignored.
- **2026-09-29** — Step 2: one dialect. `EntityStore` has `create`, `get`,
  `update`, `delete` and `list` for personalities, session templates,
  agents, sessions, channels, users and components; `put_*` is gone, and
  the seeding creates what is missing. The five operations are written
  once per store (`create_in`/`get_in`/... in the memory store,
  `create_file`/`get_file`/... in the JSON store) over a table of the
  actions that guard each kind (`store_impl::access::Actions`, with
  `ActionType::{ListAgents, DeleteAgent, ListChannels}` added). Session
  templates are kept by id in both stores - the JSON store names their
  files by id - and REST reads the one a session is made from with
  `get_session_template` instead of listing all. `create` checks for an
  existing file asynchronously now. A missing personality is reported as
  `personality`, no longer `agent_template`. Users keep their unchecked
  methods, now with `delete_user` and `list_users` (issue 94). JSON stores
  written before this keep templates under their names; being dev data,
  they are to be deleted rather than migrated.
- **2026-09-29** — Step 3: only users are found by name
  (`find_user_by_name` - a username is an identity, and the login needs
  it). `find_agent_by_name`, `find_personality_by_name` and
  `find_session_template_by_name` are gone, and with them
  `ActionType::{FindAgent, FindPersonality, FindSessionTemplate}`. The
  template instantiation looks the agent up in `list_agents` with
  `Scope::contains` - a refused or failed list is still an error, not a
  reason to create a second agent (77) - and the seeding in REST looks
  personalities and templates up in their lists. The three store tests of
  the name lookups went with them; `Scope::contains` has its own tests.
