# 368 — `component.rs` holds the registry, the container, the context of a factory and their tests in one file

**Status:** open
**Area:** Code structure

## The problem

`caelumora-runtime/src/component.rs` has 1541 lines - after `view/mod.rs` the largest file of the
runtime outside its tests. It holds what wires components (issue 99), in parts that each have a
responsibility of their own:

| Lines | What |
|---|---|
| 1-120 | the description, `ComponentError`, `ComponentResult`, and the helpers a factory reads its parameters with (`flag_of_params`, `config_of_params`) |
| 121-232 | `ComponentContext`, what a factory is given while it builds - the security context, the services (`service_of`), the facets it may ask for |
| 233-642 | `ComponentKind`, `ComponentFacets`, the factories, the singles, the preconfigured `ref`, and `ComponentRegistry`, where kinds are registered and a configuration is planned |
| 643-989 | `ComponentImpl`, and what builds a container's components: `ComponentNode`, `Planned`, `Graph` and its states, the cycle error, `ComponentContainer` |
| 990-1541 | 21 tests, of the preconfigured `ref`, of wiring, of what is declared and asked for, of singles and defaults |

## Why it matters

Whoever reads how a component is built reads past how kinds are registered, and the tests of both
are one block of 550 lines. A change to the container - 321 and 337 touched it - is reviewed in a
file that is mostly something else.

## Considerations

- **A module `component/`**, the paths unchanged through re-exports
  (`crate::component::ComponentRegistry`, and for the plugins `caelumora_runtime::component::…`,
  used by nine other crates):
  - `mod.rs`: the description, `ComponentError`, `ComponentResult`, the parameter helpers, and the
    re-exports;
  - `context.rs`: `ComponentContext` and `service_of`;
  - `registry.rs`: `ComponentKind`, `ComponentFacets`, the factories, the singles, the
    preconfigured `ref`, `ComponentRegistry`;
  - `container.rs`: `ComponentImpl`, `ComponentNode`, `Planned`, `Graph`, `ComponentContainer`.
  The tests go with what they test; the helpers they share become a `#[cfg(test)]` module of
  `component/`.
- **Open:** whether `ComponentImpl` - a built component with its id and fail policy, held by every
  owner of components - is the container's or a file of its own; and whether the parameter
  helpers go with `ComponentContext`, since a factory uses both.
- **Not here:** `component_impl/`, the built-in components, stays a module of its own; what moves
  out of it is [367](367-prompt-module.md).
- Moves only, no behaviour changes; `component.rs` through `git mv` to the file that keeps most of
  it.

## Done when

- `component.rs` is a module `component/` as above, every path into it stays as it was, and its
  tests lie with what they test.
- No behaviour changes; clippy and the tests pass.

## History

- **2026-10-15** — Opened at the maintainer's request, under epic [349](349-core-review-epic.md),
  with the other moves of files ([364](364-managers-in-different-places.md),
  [366](366-runtime-rs-holds-four-types.md), [367](367-prompt-module.md)).
