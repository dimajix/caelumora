# 280 — `ToolExecutor` is a replaceable facet with a single implementation that cannot do what it was made replaceable for

**Status:** open
**Area:** Code structure

## The problem

Issue [127](127-tool-executor-not-replaceable.md) made the way an agent runs its tool calls a
component: the trait `ToolExecutor` (`caelumora-runtime/src/tool/mod.rs`) is a facet of which an
agent has exactly one (`registry.single::<dyn ToolExecutor>`), with the default kind
`default_tool_executor` (`DefaultToolExecutor`, `tool/default_tool_executor.rs`), replaceable by a
personality or by `AppConfig::component_defaults`. The runtime's `GuardedToolExecutor` holds it and
checks permissions, shapes and classifies the result before and after it delegates.

`DefaultToolExecutor` is its only implementation, and none other is in sight. All five methods -
`execute`, `describe`, `questions`, `matcher`, `tool_sets` - find the `ToolProvider` of a tool by its
name and hand the call on.

## Why it matters

- **The variation it was made for does not fit its shape.** 127 named running calls in parallel,
  with retries or with a cache. `execute(&self, call, answers, turn: &mut Turn)` is handed one call
  with exclusive access to the turn: calls cannot run in parallel inside it - that would have to
  happen in `AgentActor`, which runs them one after another. Retries and a cache are a matter of
  the single tool, i.e. of its `ToolProvider`, rather than of an executor for all tools.
- **Tools and commands differ for no reason.** AGENTS.md holds tools up as the pattern for commands
  (a `…Provider` facet, and something that receives the ready providers and checks permissions),
  but for commands that something is a concrete type, `CommandExecutor<P>`; for tools it is a
  replaceable trait behind a wrapper.
- **It is the only such facet with a single implementation.** For the other facets of which there
  is exactly one, replacing them is in use: `ContextProvider` (one for agents run here, one for
  external agents), `LLMClient` (several providers), `Moderator` (several).
- **The name "Guarded" only marks a difference.** `GuardedToolExecutor` is called so to set it apart
  from the executor it wraps. With one executor left, the name describes nothing.

## Considerations

**Keep the trait.** It costs little - a trait, a component kind, a facet registration - and keeps
the executor replaceable by configuration, as 127 decided at the maintainer's wish. But it promises
a variation point that cannot deliver its main use, and every reader has to find out that there is
only one implementation behind it.

**Remove the trait** (the maintainer's proposal). The part of 127 that matters stays: the
permission check and what the model is told of a result remain the runtime's and never the choice
of whoever may write a personality.

- The trait `ToolExecutor`, the component kind `default_tool_executor` and its
  `registry.single::<dyn ToolExecutor>` go.
- `DefaultToolExecutor` becomes a crate-private concrete type named after what it does - e.g.
  `ToolProviders`: it refuses two providers of one tool name, finds the provider of a tool, and
  makes the tool sets. The executor builds it itself from `components.all::<dyn ToolProvider>()`.
- `GuardedToolExecutor` is renamed `ToolExecutor`, as `CommandExecutor` is called.
- The documents follow: COMPONENTS.md (the facet tables, *Facets of which there is exactly one*,
  `default_tool_executor`), TOOLS-AND-COMMANDS.md (*Running tool calls*), CONFIGURATION.md
  (`component_defaults`), ARCHITECTURE.md, SECURITY.md and CONFIDENTIALITY.md where they name
  `GuardedToolExecutor`, and the example in AGENTS.md.

If replacing the executor is wanted again, the trait comes back with a signature made for the
variation that is actually wanted - one that hands over all calls of a step, say.

**Decision:** remove the trait and rename `GuardedToolExecutor` to `ToolExecutor` (the maintainer,
2026-10-09).

## Stored data

A stored personality that configures a node of the kind `default_tool_executor` explicitly can no
longer be built - the kind is unknown. Such nodes have to be removed from it; the example seed has
none.

## Done when

- There is no `ToolExecutor` trait and no `default_tool_executor` kind; the agent's
  `ToolExecutor` is a concrete type of the runtime that finds the tools of its providers itself.
- `GuardedToolExecutor` is gone as a name, in the code and in the documents.
- The tests of the former `DefaultToolExecutor` (two providers of one name, an unknown tool) still
  run, against the new type.

## History

- 2026-10-09: Opened at the maintainer's request: the `ToolExecutor` trait has only
  `DefaultToolExecutor` as its implementation and can go; `GuardedToolExecutor` is to be renamed
  `ToolExecutor`.
