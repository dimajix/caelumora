# 330 — The setup of the log is part of the runtime library

**Status:** done
**Area:** Code structure
**Summary:** The runtime library installs the `tracing` subscriber for the binaries and so depends on `tracing-subscriber`, though setting up the log is the application's decision.

## The problem

`caelumora-runtime/src/logging.rs` holds two things: the spans the runtime opens itself
(`envelope_span`, `agent_span`), which tie a message's way through the actors together, and
`logging::init`, which installs the `tracing` subscriber - its filter, format and destination from
`AppConfig::logging` - and is called only by the binaries (`caelumora-rest-server/src/main.rs`,
`caelumora-tui/src/main.rs`). For the second, the runtime depends on `tracing-subscriber`.

## Why it matters

- A library emits records; which subscriber writes them, where and how, is the application's
  decision. A crate that embeds the runtime - a test harness, a plugin's tests, a later binary -
  gets a dependency it does not use and a setup it may not want.
- With issue [323](323-kinds-enumerated-in-the-core.md) the composition roots become
  `caelumora-app` and `caelumora-tui`; that is where the setup belongs.

## Considerations

- **Where `init` goes.** Into `caelumora-app` and `caelumora-tui` each - but the two share it today,
  and a copy in each is the repetition `AGENTS.md` warns of. Or into a small crate both depend on
  (`caelumora-logging`), which is a crate of one function. Or it stays in the runtime behind a
  feature (`log-setup`) the binaries switch on. Recommended: the feature - it keeps one place and
  takes nothing from the runtime's users that they do not ask for -, unless the composition of
  issue 323 brings a crate the two binaries share anyway, which would then be its place.
- **The spans stay.** `envelope_span` and `agent_span` are the runtime's own records and need only
  `tracing`.
- **`LoggingConfig`** stays in `AppConfig`: it is configuration of the application, as before.
- Small, and independent of the other issues; it can be done at any time.

## Done when

- The runtime depends on `tracing-subscriber` no longer unless asked to; the binaries set up the
  log as before.
- `docs/ARCHITECTURE.md` or `CODING-GUIDELINES.md`, wherever logging is described, says where the
  setup lives.

## History

- **2026-10-12** — Opened from the discussion of which subjects besides skills could move into
  crates of their own (issue 323).
- **2026-10-12** — Done, with a crate of its own instead of the recommended feature. Weighed again
  after issue 323, which brought no crate the two binaries share: a feature would only have hidden
  the dependency, while the runtime stayed the owner of the application's setup - and whatever
  joins it later (an export, a rotation of the files) would have grown there too. The new crate
  `caelumora-logging` holds `init` and `LogTarget` and is the only one that depends on
  `tracing-subscriber`; `caelumora-app` and `caelumora-tui` call `caelumora_logging::init`. The
  spans stay in `caelumora_runtime::logging`, whose callers now import them instead of writing
  their path. Its test, which checks that the envelope ids reach the JSON log, moved along and
  takes the runtime as a dev-dependency. `CODING-GUIDELINES.md`, `ARCHITECTURE.md` and the README
  name the crate.
