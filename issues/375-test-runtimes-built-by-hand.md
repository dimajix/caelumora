# 375 — About twenty tests build a runtime by hand, each writing out what it is built from

**Status:** done
**Area:** Code structure

## The problem

A runtime is built from seven or eight things: a state store and an entity store, a
`SecretManager`, an access policy, a confidentiality policy, an `AppConfig`, a component registry,
and the plugins or backends (`RuntimeManager::new`, `RuntimeBuilder::new`). The runtime's test
support has fixtures for it - `session::testing::test_runtime()` and, since
[371](371-tool-executor-construction.md), `test_runtime_under(access_policy, confidentiality,
config)`, `owned_runtime(rules)`, `testing::World` - but about twenty tests in the runtime, the
agents' crate, `caelumora-mcp` and `caelumora-rest-server` still build one by hand, writing out
all of it:

- the runtime: `actor/approval.rs`, `actor/user.rs`, `channel/tests/mod.rs` (twice, one of them
  `scripted_channel`), `session/manager.rs`, `session/testing.rs` (`owned_runtime` beside
  `test_runtime_under`), `workspace/testing.rs`, `view/mod.rs`, `plugin.rs`;
- the agents: `plugin.rs` (twice), `channel_tests/mod.rs`, `channel_tests/external_agent.rs`
  (twice), `components/agent_memory.rs`, and the three channel prompt providers;
- `caelumora-mcp/tests/mcp_tool_integration.rs` and `caelumora-rest-server/src/routes/mod.rs`.

What they vary is mostly the store - one the test filled before, or one under policies -, the
registry, the plugins and a section of the configuration; the secrets, the policies and the rest
are the same everywhere. `test_runtime_under` cannot serve most of them, since it makes a store
of its own.

Related: eleven tests write `Arc::new(CommandExecutor::empty(Arc::clone(runtime.access_policy())))`,
nearly always right before `ChannelRuntime::outside_a_session`, which takes the user commands of
the session it stands in for and almost never gets any.

## Why it matters

- A change to what a runtime is built from - a new service, another argument of
  `RuntimeManager::new` - is a change to twenty tests in four crates.
- A reader of a test has to read the whole line to see what the test sets apart from the default,
  if anything.

## Considerations

- **Survey first, as in [372](372-runtime-test-actors.md):** list what each of the twenty sets
  apart from `test_runtime()`, and see which differences have a reason.
- **One fixture more general:** `test_runtime_under` takes the store as well (or a variant
  does), and the plugins - so that it serves the tests that fill a store first or add a plugin;
  `owned_runtime` and `scripted_channel` become calls of it.
- **`outside_a_session` without commands:** it makes the empty `CommandExecutor` itself, and the
  few tests that need commands give theirs in another way (a second constructor, or the session's
  commands set afterwards).
- **Leave it.** Each of the lines is written once and read rarely; the cost is the next change to
  `RuntimeManager::new`.

## History

- **2026-10-15** — Done, by the maintainer's decision, after the survey: the tests differ in the
  store (one filled first, one under policies, a second one for the states), the policies, the
  configuration, the registry and the plugins, each for a reason of its own, and only the secrets
  were the same everywhere - so no single fixture with more parameters, but `testing::TestRuntime`:
  the parts of a runtime as public fields with defaults, of which a test names those it differs in
  (`TestRuntime { plugins: vec![&AgentsPlugin], ..Default::default() }`), built by `build()`, or by
  `try_build()` for a test of what refuses to be built. It replaces `test_runtime_under` and
  `owned_store`; `test_runtime()`, `owned_runtime` and `World` are built with it, and so are the
  tests of the runtime and the agents and those of `caelumora-mcp` and `caelumora-rest-server`,
  which have the runtime's `test-support` among their dev-dependencies now. The tests of
  `RuntimeBuilder` in `plugin.rs` need no builder of their own: a plugin claiming a section twice is
  two plugins in the list. Where the agents' tests named the agents' plugin with the agents'
  component kinds, they name `TestAgents`, which brings the same, unless they register stand-ins of
  their own or test the plugin itself. `ChannelRuntime::outside_a_session` makes the empty
  `CommandExecutor` itself; the one test whose channel has commands builds its runtime with
  `ChannelRuntime::new`. Left as they are: the program's own test in `caelumora-app`, which builds
  the runtime as the program does.
