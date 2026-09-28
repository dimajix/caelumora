# 99 — The component factory is synchronous while `build` is async, and tags are dead

**Status:** open
**Area:** Code structure

## The problem

- `ComponentFactory` is a synchronous closure (`component.rs:91`) but
  `ComponentRegistry::build` is `async` (`:133`). `mcp_tool::register`
  therefore uses `block_in_place` + `block_on` and demands a multi-threaded
  runtime (`mcp_tool.rs:263-278`).
- `ComponentContainer::get_all(tag)` is called with `None` at all six
  sites; `nodes(tag)`, `ComponentNode.tags` and the `STAGE_*` constants
  (`caelumora-model/src/component.rs:6-8`) have no reader. The container is a
  `HashMap + node_order` (`:183-184`) so that `ComponentError::NodeNotFound`
  exists for a case that cannot happen, and `get_all` is fallible (six `?`).
- `EmitOutProcessor` has no implementation; the loop runs the empty list
  (`agent.rs:170-198`). `MessageProcessor::name` has no caller.

## Why it matters

A factory that has to block a thread to discover tools is the wrong shape
for the one component that does I/O at build time. Dead parameters are read
by every new component author.

## Considerations

- `ComponentFactory = Box<dyn Fn(&ComponentDescriptor, &ComponentContext) ->
  BoxFuture<'_, ComponentResult<ComponentFacets>>>`, plus a `register_sync`
  helper that wraps the 14 synchronous factories unchanged.
- Drop the tag parameter and filter; `ComponentContainer` as
  `Vec<ComponentNode>` in build order; `get_all` infallible. `tags` in the
  config may stay as `#[serde(default)]` or go.
- `EmitOutProcessor`: keep and name it in ARCHITECTURE.md as an extension
  point, or remove it - decide, do not leave it implicit.

## Done when

- No component blocks a thread to build.
- `get_all` takes no tag and cannot fail.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 4.1.2, 4.1.3, 4.1.5).
