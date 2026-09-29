# 96 — `AppConfig` mixes tagged and untagged enums, `Option` and `default`

**Status:** done
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
- **2026-09-29** — Done, as agreed with the maintainer. Every enum that
  picks an implementation is tagged with `type`: the store backend is now
  `{ type: json, path }` or `{ type: memory }` (the default);
  `JsonConfig`, `_dummy` and the `Default` variant are gone, and
  `SecretProviderConfig::File` has its `path` inline like `Json` rather
  than a struct of its own (the YAML stays the same). `LogFormat` stays a
  plain value: it is a setting, not a choice of implementation. No
  section is an `Option` any more: `AppConfig` is `#[serde(default)]`,
  and its hand-written `Default` is the one place the defaults live -
  including the secret chain (`local/secrets.json`, then env), which
  moved there from `create_secret_manager`; `users` defaults to no
  provider. `PersistenceBackend::open` opens a store for both
  `create_entity_store` and `create_state_store`, over a private
  `Store: EntityStore + StateStore`. `ComponentConfig` derives `Default`.
  Beyond the issue: every config struct and enum is
  `deny_unknown_fields`, so a misspelt key - which used to fall back to a
  memory store without a word - is an error naming the key and its line.
  Errors still quote no values; the test for that now puts its marker in
  a value, since keys are named. `examples/config.yaml` is on the new
  format and loaded by a test.
