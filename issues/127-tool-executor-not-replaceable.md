# 127 — The tool executor is fixed in the runtime and cannot be replaced by a component

**Status:** done
**Area:** Tools and approvals
**Summary:** The tool executor is fixed in the runtime, so how tools are executed cannot be tried out by configuring a different component.

## The problem

`ActorRuntime::new_tool_executor` always builds a `DefaultToolExecutor` from the agent's
`ToolProvider`s (`runtime.rs`). How tools are executed - one after another, in parallel, with
retries, with a cache - cannot be tried out without changing the runtime, while everything else an
agent does comes from components of its personality.

## Why it matters

The maintainer wants the agent assembled from components the way Spring Boot assembles an
application from beans, so that alternative implementations can be tried by configuration. The
executor is the one part of the agent loop that is not a component.

## Considerations

- With issue [99](99-component-factory-and-tags.md), a facet type can be one of which there is
  exactly one, with a default kind, and a component can have the components that provide a facet
  injected by type. The executor becomes such a facet: `ToolExecutor` with the default kind
  `default_tool_executor`, which has the `ToolProvider`s injected. A personality replaces it with a
  node of another kind; `component_defaults` in the configuration replaces it for all of them.
- The permission check must not become replaceable with it. Personalities are stored entities, and
  whoever may create one would otherwise choose the code that decides about permissions. So the
  runtime wraps the configured executor: the wrapper checks `CallTool` (`Enforcement::require`, and
  `is_permitted` before approval) and then delegates. A component decides how a tool is executed,
  never whether.
- Follows issue [98](98-context-provider-facet.md), which does the same for the `ContextProvider`.

## Done when

- The agent takes its `ToolExecutor` from its components, a default one if its personality
  configures none.
- The `CallTool` check happens in the runtime, whatever executor is configured.

## History

- **2026-09-29** — Opened from the discussion of issues 98 and 99 with the maintainer: executor as a
  component, permission check kept in the runtime.
- **2026-09-29** — Done. `ToolExecutor` is a facet of which an agent has exactly one
  (`ComponentRegistry::single`, default kind `default_tool_executor`); it has lost `is_permitted`
  and checks nothing. `DefaultToolExecutor` moved to `component_impl/default_tool_executor.rs` and
  has every `ToolProvider` injected (`context.all`), keeping the check for two tools of the same
  name, the lookup, the timing and the mapping of errors to results. The agent takes its executor
  with `components.one::<dyn ToolExecutor>()` and holds it behind the new `GuardedToolExecutor` in
  `crate::tool`, which checks that the caller is an agent and `CallTool` - in `is_permitted` before
  approval and again before it delegates `execute` - so a configured executor only ever sees
  permitted calls. `ActorRuntime::new_tool_executor` is gone. The `CommandExecutor` stays as it is,
  a runtime type that checks permissions itself: how a command runs is the command's own business,
  and there is nothing about running commands to try out; the difference is documented on it and in
  AGENTS.md.

