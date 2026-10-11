# 309 — Fields are optional that production code always sets

**Status:** open
**Area:** Code structure

## The problem

Some fields are an `Option` although production code never leaves them `None`. Only tests do, or
nobody does. Every reader then handles a case that does not occur, and the type no longer says
what the field really holds.

Two such fields were found:

1. **`Turn::views: Option<ViewsFactory>`** (`caelumora-runtime/src/agent/turn/mod.rs`) and with it
   **`AgentWork::views: Option<&ViewsFactory>`** (`caelumora-runtime/src/agent/work.rs`). Their
   doc comments say it: `None` "for a turn outside of a channel - in tests". In production a
   turn always has its views:
   - `AgentActor::turn` (`actor/agent/mod.rs`) and the command path in `actor/commands.rs` call
     `Turn::of(...)` and then `.with_views(...)` at once. The turn is built in two steps only so
     that tests can skip the second.
   - `ExternalAgentActor` passes `Some(&self.views)` to `AgentWork::new`.
   - Only tests pass `None`: `Turn::begun` (`#[cfg(test)]`), the many `Turn::of` calls in tests
     without `with_views`, and `AgentWork::new(..., None)` in `external_context_provider.rs`.

   What it costs in production code:
   - `AgentWork::session` starts with `self.views?`.
   - `skills.rs` has `session_of`, whose doc says "a turn outside of one - in tests - has no
     skills". It also has an error message for that case.
   - The two-step `Turn::of` + `with_views` exists only because of the tests.

   `AgentWork::session` itself stays an `Option`: `ViewsFactory::session` is `None` while the
   channel is not attached to its session yet, or once it is gone. That reason comes from
   production. The fallbacks of the prompt providers and of `notes`/`private_notes` for a missing
   session keep that reason. What disappears is the second, test-only reason for the same `None`.

2. **`ComponentContainer::graph: Option<Arc<Graph>>`** (`caelumora-runtime/src/component.rs`). It
   is `None` only in a `ComponentContainer::default()`. Nothing calls that, not even a test.
   `from_config` and `with_defaults`, the only constructors, always set `Some(graph)`. Yet
   `components`, `get_all` and `one` each have a branch for the empty container. `one` even has an
   error of its own for it ("an empty container has no …").

## Why it matters

- The production code carries branches, error messages and doc comments for a state that only
  tests create, or nobody does. A reader has to work out that the branch is dead.
- The two-step construction of `Turn` is easy to get wrong: a new caller who forgets `with_views`
  gets a turn without any error. It reads nothing from other channels, finds no skills and gets
  an empty roster.
- The production code alone should decide the shape of a type. A test that needs less should get
  it from a fixture, not from an `Option` in the type.

## Considerations

### `Turn` / `AgentWork`

`Turn::of` takes the `ViewsFactory` as a parameter, and `AgentWork::new` takes a
`&ViewsFactory`. `with_views` goes away, and so does the `Option`. The open question is where tests
get a `ViewsFactory` from that reaches no session:

- **A detached actor runtime.** `ViewsFactory::of_actor` for an actor whose channel is not
  attached to a session. Its `session` is then `None`, for exactly the production reason. This
  adds no new code path, but each test needs a runtime, which is heavy for the unit tests of
  simple tools (calculator, sleep, stop).
- **`ViewsFactory::in_session` with a bare session.** The `#[cfg(test)]` constructor already
  exists, and so does `bare_session`. But the turn is then *in* a session. Tests that rely on
  "no session" would test something different.
- **A `#[cfg(test)]` variant `FactoryRoot::Nowhere`.** Cheap for the tests, but it brings a
  test-only arm back into the production `match` - the same problem in another place.

Recommendation: the first, behind a single test fixture (e.g. `ViewsFactory::detached()` under
`#[cfg(test)]`, built on a detached actor runtime). Then `Turn::begun` and the test calls of
`Turn::of` get it in one place. Cost: about 90 call sites in 25 files, almost all of them tests.

### `ComponentContainer`

Make `graph` an `Arc<Graph>` and remove `#[derive(Default)]`. The three empty-container branches
and their error go away. This needs no decision.

### Looking for more

The search covered the `Option` fields of structs in all crates. Configuration and wire types
(`config.rs`, `models.rs`, `wire.rs`) were left out on purpose: there, `None` is a value a user
or a remote system may send. The candidates checked and kept because production has its own
reason for `None`: `ChannelLanguagePromptProvider::told` and `ChannelMembersPromptProvider::told`
(config `changes_only`), `LocalProvider::commands` (config `shell`), `RunningActor::sender`
(an actor that is leaving), `ChannelEntry::task`, `ChannelView::running`/`SessionView::running`,
`ControlTokens::matcher`.

## Done when

- `Turn` and `AgentWork` hold their views without an `Option`. `Turn::with_views` is gone, and
  no doc comment and no error message in production code mentions "in tests" as a reason for a
  missing session.
- `ComponentContainer` holds its graph without an `Option` and has no `Default`.
- `cargo test` and `cargo clippy --workspace --all-targets` pass.

## History

- **2026-10-11** — Opened at the maintainer's request after a search for `Option` fields in structs
  that only tests leave `None`.
