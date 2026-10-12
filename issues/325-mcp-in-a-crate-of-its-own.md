# 325 — The MCP integration is part of the runtime crate

**Status:** open
**Area:** Code structure

## The problem

`mcp_tool` (`caelumora-runtime/src/tool/mcp_tool.rs`) is a component kind like any other: it is
registered by `mcp_tool::register`, called from `tool::register_all` and so from
`ComponentRegistry::built_in`, and the core knows nothing of it beyond that call. Still it lives in
the runtime crate, and with it `rmcp`, as a dependency of everything built on the runtime and, with
the server transport, as a dev-dependency of its tests.

Of the runtime it uses only what a tool provider uses: `component::{config_of_params,
ComponentFacets, ComponentKind, ComponentRegistry}`, `tool::{ApprovalMode, ToolError, ToolOutput,
ToolProvider, ToolResult}` and the `Turn` it is called with (`security_context`). Its integration
test, `caelumora-runtime/tests/mcp_tool_integration.rs`, already reaches it through the public API
alone.

## Why it matters

- It is the cheapest case of moving something out of the core: the seam - a component kind - is
  open already, and none of the cuts of issue [323](323-kinds-enumerated-in-the-core.md) is needed.
  Done before skills, it tries out the composition of issue 323 (`Plugin`, `RuntimeBuilder`) with
  a plugin that brings nothing but a component kind, apart from the question whether the seams of
  a stored kind hold.
- It forces the question every plugin raises: what the runtime offers as its API to a plugin. Today
  that is answered by whatever happens to be `pub`; issue [107](107-public-interface.md) trimmed
  the `pub` items to those with callers outside, and a plugin crate is such a caller.
- `rmcp` is the heaviest dependency of the runtime, and a binary that reaches no MCP server need
  not build it.
- A tool server of another protocol - OpenAPI, issue [252](252-openapi-tool.md) - is of the same
  shape and should begin as a crate of its own; this issue gives it the pattern.

## Considerations

- **The runtime's API for plugins.** Made visible is what the signatures of the facets need, and
  nothing else: no prelude module that re-exports at large, since it would hide which types a
  plugin depends on. `COMPONENTS.md` says what a component kind outside the core may use.
- **Before or after the composition of issue 323.** Before its step 9, the binaries call
  `caelumora_mcp::register(&mut registry)` beside `ComponentRegistry::built_in`; after it, the
  crate is an `impl Plugin`. Recommended: right after step 9, as its first plugin, so that no
  registration is written that step 9 replaces at once.
- **Layers.** The crate holds no stored kind, no routes and no wire types, so the features of issue
  323, cut 11, are not needed: one crate, `caelumora-mcp`, depending on the runtime.
- **What moves with it:** `mcp_tool.rs`, its tests, `tests/mcp_tool_integration.rs`, `rmcp` and its
  dev-dependency with the server transport. The kind keeps its name `mcp_tool`, so no
  configuration changes.
- **What stays:** `ApprovalMode` and the rest of `tool`, which other tool kinds use as well.

## Done when

- `caelumora-mcp` holds the `mcp_tool` kind and its tests; the runtime depends on `rmcp` no longer
  and compiles and passes its tests without it.
- The binaries list it; personalities that name `mcp_tool` work as before.
- `docs/COMPONENTS.md` and `docs/TOOLS.md` say where the kind lives and what a component kind
  outside the core may use of the runtime.

## History

- **2026-10-12** — Opened from the discussion of which subjects besides skills could move into
  crates of their own (issue 323), as the cheapest of them and a first plugin for the composition.
