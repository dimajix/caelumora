# 490 — 63 functions are longer than 100 lines once rustfmt lays them out

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** Laid out by rustfmt (issue 487), 63 functions - 8 of production code, 55 tests - are longer than the 100 lines `clippy::too_many_lines` allows, and carry an `expect` of the lint instead of being split.

## The problem

`clippy::too_many_lines` allows no function longer than 100 lines (issue
[452](452-guideline-rules-not-checked-by-lints.md)). The limit was set while the code was written
in long lines; laid out by rustfmt (issue [487](487-code-not-formatted-by-rustfmt.md)), which
breaks every line longer than 100 characters and a broken list into one element per line, 63
functions grew past it - the code grew by about 38 % in all. The limit stays at clippy's default,
which assumes code laid out by rustfmt; the 63 carry an `expect` of the lint whose reason names
this issue, and which fires once a function is short enough. The five that were longer before the
lint carry an `expect` of issue 452 and are not counted here.

Production code, 8:

- `Envelope::entry` (182 lines) and `Envelope::describe` (187), `caelumora-model/src/actor.rs` -
  one `match` over every kind of event each;
- `ChannelLoop::admission` (155), `caelumora-runtime/src/channel/run.rs`;
- `session_routes` (117), `caelumora-rest-server/src/routes/mod.rs`;
- `AgentActor::run` (116), `caelumora-agents/src/actor/agent.rs`;
- `RuntimeBuilder::new` (116), `caelumora-runtime/src/runtime/builder.rs`;
- `ArtifactManager::publish` (115), `caelumora-artifacts/src/manager.rs`;
- `SkillsComponent::execute` (101), `caelumora-skills/src/component.rs`.

Tests, 55, up to 278 lines (`entity_store_behaves` of the store suite): most of the security
suites of `caelumora-runtime` and `caelumora-agents`, the store suite of `caelumora-model`, and
tests of the views, the channel loop and the session manager. Their length is mostly assertions
with a message, which rustfmt lays out over five lines each.

## Why it matters

- A function that does not fit on a screen is read in parts; the lint is there to keep it whole.
- 63 `expect`s are 63 places where a rule does not hold; as long as they stand, a function that
  grows further is not noticed.

## Considerations

- **Production code first.** `Envelope::entry` and `Envelope::describe` are each one `match` over
  every kind of actor event: a method of each kind, or a table of them, makes them short - a
  design question of its own. The others split into the steps they already name in comments.
- **Tests by what they show.** A test that shows several things is several tests; a long setup
  that tests repeat belongs into a helper of its module or the test world
  (`caelumora_runtime::testing`). A test is not split only to be shorter: what it shows is.
- **Step by step**, a crate or a module at a time; each step removes the `expect`s of the
  functions it splits.

## Done when

- No function carries an `expect` of `clippy::too_many_lines` of this issue.

## History

- **2026-10-29** — Opened at the maintainer's request in the work on issue 487: the maintainer
  chose to keep the limit at 100 lines and to mark the functions rustfmt made longer, instead of
  raising the limit to about 140, which would have hidden 44 of them.
