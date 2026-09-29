# 99 — The component factory is synchronous while `build` is async, and tags are dead

**Status:** done
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
- **2026-09-29** — Decided with the maintainer, together with issues 98
  and [127](127-tool-executor-not-replaceable.md): components are wired the
  way Spring Boot wires beans.
  - A kind declares, when it is registered, which facets it provides (the
    counterpart of a bean's class); after building, the container checks
    that it did.
  - What a component needs is not declared: its factory asks its context
    for it by type (`context.all::<dyn PromptProvider>().await`), which
    builds the providing components on demand, recursively, in the order
    of the configuration. A component being built that provides the type
    asked for is a real cycle and an error naming both. Fully lazy - not
    declaring what a kind provides either - was rejected: then "all of a
    type" is unknown while a component is being built, and two components
    that each collect others (context and executor) would be a false cycle
    or silently miss one.
  - A facet type may be one of which there is exactly one
    (`ContextProvider`, `ToolExecutor`), registered with a default kind in
    code. Two components providing it are an error before anything is
    built; none, and the default is built when it is first asked for
    (`one::<T>()`) - so the commands of a session, which never ask for an
    executor, get none. The configuration can replace a default for all
    personalities (`AppConfig::component_defaults`), a personality with a
    node of its own: personality before configuration before code.
  - `deps` goes: nothing is wired by hand any more, so nothing can be
    forgotten. The factory becomes async (a synchronous `register` stays
    for the many factories that do not wait), and `mcp_tool` no longer
    blocks a thread.
  - Tags stay, although nothing reads them yet: the maintainer expects to
    use them. `get_all` keeps its tag parameter, but can no longer fail.
  - `EmitOutProcessor` stays, as an extension point named in ARCHITECTURE.md.
- **2026-09-29** — Done as decided above. `ComponentKind::new(kind).provides::<dyn …>()`
  declares a kind's facets; `ComponentRegistry::register` takes a factory
  that does not wait, `register_async` one that returns a future. The
  container plans the components (`ref`s resolved, the facets of each
  known), refuses duplicate ids and two providers of a single, and builds
  them lazily: `ComponentContext::all::<T>()` builds and returns every
  other component providing `T` in the order of the configuration,
  `one::<T>()` the single one or its default; a component being built that
  is asked for is `ComponentError::Cycle`. Singles: `ComponentRegistry::single::<T>(default_kind)`,
  `verify` (the default kind provides it) and `set_defaults` for
  `AppConfig::component_defaults`, both called by `RuntimeManager::new`,
  which therefore returns a `ComponentResult` and takes the defaults as an
  argument like the commands. No single is registered yet - the
  `ContextProvider` becomes the first with issue 98, the executor with
  issue 127.
  `deps` is gone from `ComponentDescriptor`, `ComponentContext` and
  `seed.yaml`, with `UnknownDependency`, `NodeNotFound`, `CycleDetected`
  and the topological sort; `state_context_provider` asks for every
  `PromptProvider` instead, so a prompt part can no longer be left out by
  forgetting it (checked against a running server: Sassy's system prompt
  holds her persona and the planning prompt of `create_plan_tool`, with no
  `deps` anywhere). `get_all` keeps its tag and can no longer fail.
  `mcp_tool` discovers its tools in an async factory, without
  `block_in_place`; its integration tests run on the default test runtime
  instead of `multi_thread`. `MessageProcessor::name`, which had no caller,
  is gone; `EmitOutProcessor` stays, named in ARCHITECTURE.md as an
  extension point.

