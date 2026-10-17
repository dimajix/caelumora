# 373 — The agents' test world mirrors the four constructors of the runtime's `World`

**Status:** done
**Area:** Code structure
**Summary:** The agents' test world mirrors the four constructors of the runtime's `World`, each a shorthand for the next, so a new way to set up a world is added in eight places.

## The problem

The security suite and the plugins' tests run in a world, `caelumora_runtime::testing::World`,
built by one of four constructors, each a shorthand for the next:

| Constructor | What it adds |
|---|---|
| `World::new()` | nothing |
| `World::with_kinds(register)` | component kinds |
| `World::with(plugins, register)` | plugins |
| `World::configured(plugins, sections, register)` | sections of the configuration |

Since [370](370-agent-logic-in-a-crate-of-its-own.md), the agents' world is a second set of four,
in `caelumora_agents::testing`, that adds the agents' plugin and the models of a test to the
same: `world()`, `world_with_kinds`, `world_with` and `world_configured`, each again a shorthand
for the next. Measured in the workspace: `world` is called 37 times, `world_with_kinds` 12,
`world_with` 5 and `world_configured` 3.

## Why it matters

A new way to set up a world - another argument, another default - is to be added to eight
functions in two crates, in the same order, or the two sets drift apart. The shorthands save a
test one or two empty arguments (`&[]`, `|_| {}`), and `world_with` and `world_configured` are
called eight times between them.

## Considerations

- **Two of each:** the world as it is (`World::new`, `world()`) and the one with everything
  (`World::configured`, `world_configured`), the shorthands between them removed. A test that
  adds kinds writes `world_configured(&[], |_| BTreeMap::new(), register)`.
- **A builder for the world** - `World::builder().plugin(…).kinds(…).sections(…).build()` -, and
  the agents' world as `World::builder()` with the agents' plugin and the test models added:
  one place for every way to set up a world, and the agents' crate adds only what is its own. A
  new type, whose responsibility would be to set up the world in steps.
- **Leave it.** The functions are short and the two sets are written alike; the cost is
  maintenance in two places.

## History

- **2026-10-15** — Done, by the maintainer's decision, otherwise than the considerations: counted
  again, `World::with_kinds` and `World::with` were called by nobody but the next shorthand, and
  the agents' set differed from the runtime's in two things only - the agents' plugin and a model
  that is not to be asked as the default. Both are a plugin now,
  `caelumora_agents::testing::TestAgents`, which a test gives to the runtime's `World` with its own
  plugins (`World::with(&[&TestAgents, &SkillsPlugin], …)`), as plugins are given to it already.
  `world_with_kinds`, `world_with` and `world_configured` are gone; `world()` stays as the one
  shorthand of the agents' tests, for its 37 callers, and `World::with_kinds` is gone, as nothing
  calls it any more: a new way to set up a world is added in the runtime alone. `TestAgents` names
  the default model, which a plugin otherwise leaves to the program (PLUGINS.md): it stands in for
  the program of the tests. Two of each and a builder were not taken: the first would have written
  out empty arguments in 17 calls and kept both sets, the second added a type for what a plugin
  does.
