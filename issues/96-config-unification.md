# 96 — `AppConfig` mixes tagged and untagged enums, `Option` and `default`

**Status:** open
**Area:** Maintenance

## The problem

`PersistenceBackend` (`caelumora-model/src/config.rs:34-41`) is
`#[serde(untagged)]` with the variants `Json { json }`, `Memory { _dummy:
Option<String> }` and `Default`, while `SecretProviderConfig`,
`SecurityConfig` and `UserProviderConfig` are `#[serde(tag = "type",
rename_all = "snake_case")]`. `create_entity_store`/`create_state_store`
(`220-248`) are identical 14-line matches that build `MemoryStore` three
times. `persistence`/`secrets`/`users` are `Option<…>`; `auth`/`logging`/
`commands`/`security` have `#[serde(default)]`. `ComponentConfig` has one
field and a hand-written `Default` that `derive` would give.

## Why it matters

A configuration file is the one document users write by hand; two ways to
say "which backend" is one too many, and the `_dummy` field says so.

## Considerations

`enum PersistenceBackend { #[default] Memory, Json { path } }` with
`tag = "type"`; every section `#[serde(default)]`; one `store(backend)`
helper for both stores. Cost: `examples/config.yaml`, `local/*`, three tests.

## Done when

Every config enum is tagged the same way and every section has a default.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 3.2.9).
