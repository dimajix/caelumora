# 36 — The configuration is parsed with a crate that is no longer maintained

**Status:** done
**Area:** Maintenance
**Summary:** The configuration is parsed with `serde_yaml`, which is deprecated and will receive no further fixes.

## The problem

`caelumora-model` reads the application configuration with `serde_yaml` 0.9. Its author declared it
unmaintained in March 2024; the last release is `0.9.34+deprecated`, and there will be no further
fixes. It is the only YAML dependency in the workspace, and it is used in one place:
`AppConfig::load` (`caelumora-model/src/config.rs`) plus that module's unit tests. It also leaks
into the public API, since `ConfigError::Parse` wraps `serde_yaml::Error`.

## Why it matters

The configuration file is the first thing every binary reads, and it is written by hand, so the
parser sees malformed input more than any other. A parser that no longer receives fixes — its YAML
parsing sits on `unsafe-libyaml`, a machine translation of libyaml to unsafe Rust — is a liability
that only grows: a future advisory would have no fixed version to move to, and the crate will
eventually fall behind `serde` itself. Today it works, which is why this is maintenance and not a
bug.

## Considerations

- **Candidates** (as of 2026-09-24):
  - `serde-saphyr` — actively released (1.3, September 2026), a pure-Rust parser built on `saphyr`,
    no `unsafe-libyaml`, and aimed at panic-free parsing with good error locations. Different API
    surface from `serde_yaml`, so it is a small port rather than a rename.
  - `serde_yaml_ng` / `serde_norway` — forks of `serde_yaml` that keep its API, so the switch is a
    rename. But both still sit on `unsafe-libyaml`, and neither has had a release since 2024; that
    swaps one stale dependency for another.
  - `serde_yml` — ruled out: its own crate description declares it deprecated and unmaintained.
- **What the config actually needs from the parser.** `config.rs` uses an `#[serde(untagged)]` enum
  and two internally tagged enums (`#[serde(tag = "type")]`, e.g. the user providers). Both go
  through serde's buffered `Content` representation, and that is exactly where YAML deserializers
  differ (scalars that arrive as strings, numbers that do not match an untagged variant). The
  existing unit tests in `config.rs` cover these shapes and are the acceptance test;
  `examples/config.yaml` should load unchanged.
- **Error messages.** `ConfigError::Parse` is what a user sees when the config is wrong. The
  replacement's error must still say where (line and column) and what, and — per `AGENTS.md` — must
  not carry secret values. Worth checking what the new error includes before it is exposed.
- **Public API.** `ConfigError::Parse(#[from] serde_yaml::Error)` names the crate in a public type.
  Changing it is a breaking change for anyone matching on it; within the workspace nothing does.
  Wrapping the error in an own type would keep the next switch from repeating this.
- **Not in scope:** changing the configuration format itself.

## Done when

- `serde_yaml` is no longer a dependency of any crate in the workspace.
- The unit tests in `config.rs` pass and `examples/config.yaml` loads as before.
- A malformed config file still produces an error that names its location.

## History

- **2026-09-24** — Opened during the dependency update (`ed5b3e0`), which left `serde_yaml` at
  `0.9.34+deprecated` because there is no in-place upgrade.
- **2026-09-24** — Done with `serde-saphyr` 1.3, with only its `deserialize` feature. Parsing goes
  through the new `AppConfig::from_yaml`, which `load` and the unit tests share, so the tests
  exercise the real parser options. One option had to change: by default the crate renders the
  offending lines of the input into its error, and a configuration may hold credentials, so
  `with_snippet` is off — the message keeps line and column ("unclosed bracket '[' at line 2, column
  21"), and a new test checks that it does not quote the input. Values the parser rejects (an
  unknown variant) are still named, as they were with `serde_yaml`. All existing config tests passed
  unchanged and `examples/config.yaml` loads. `ConfigError::Parse` now wraps `serde_saphyr::Error`;
  the own error type considered above was not introduced, since nothing matches on the variant and
  the switch touched one line of it.
