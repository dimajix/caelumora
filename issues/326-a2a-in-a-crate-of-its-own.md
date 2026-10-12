# 326 — The A2A protocol is part of the runtime crate

**Status:** done
**Area:** Code structure

## The problem

The facet through which an external agent is reached, `ExternalAgent` (`caelumora-runtime/src/
external/mod.rs`), says nothing of a protocol, and the `ExternalAgentActor` knows only the facet.
The one protocol, A2A, is the component kind `a2a_agent` in `external/a2a_agent/` - the wire types,
their translation, the HTTP client and the component, about 1350 lines since issue
[210](210-a2a-agent-structure.md) - registered by `a2a_agent::register` from
`external::register_all`. It lives in the runtime crate although nothing of the core names it.

Of the runtime it uses the facet and its types (`ExternalAgent`, `ExternalEvent`,
`OutboundMessage`, `Said`, `ExternalStream`, `ExternalAgentResult`), the component registry, and
`crate::remote::describe_request_error`, which is `pub(crate)` and also used by Open Terminal and
the Ollama client. The tests of the external agent in the channel use `ScriptedExternalAgent`, not
A2A.

## Why it matters

- A protocol is what is exchanged: a second one - an agent reached otherwise than by A2A - is a
  crate beside it, and the core keeps the facet alone. The boundary is enforced by the compiler
  only if A2A is outside.
- The pattern is the one of issue [325](325-mcp-in-a-crate-of-its-own.md): a component kind with no
  stored data, no routes and no wire types of the REST API.
- Unlike MCP, it brings no dependency of its own that would leave the core - `reqwest` and
  `sse-stream` are used by the core besides. The gain is the boundary, not the build.

## Considerations

- **The shared remote helpers.** `describe_request_error` and `describe_chain` in
  `caelumora-runtime/src/remote.rs` are what remote integrations share (issue
  [188](188-remote-helpers-on-llm-error.md)). Either they move to `caelumora-model::remote`, beside
  `RemoteEndpoint`, which every such crate already uses - at the price that the model then depends
  on `reqwest` for its error type, which it does not today -, or they become public in the
  runtime. Decided when this issue is taken up; Open Terminal and Ollama follow the same.
- **The facet's types** become the runtime's API for plugins; what was said of that in issue 325
  holds here.
- **When:** together with or right after issue 325, after step 9 of issue
  [323](323-kinds-enumerated-in-the-core.md).
- **What moves:** `external/a2a_agent/` with its tests, as `caelumora-a2a`. The kind keeps its name
  `a2a_agent`.

## Done when

- `caelumora-a2a` holds the `a2a_agent` kind and its tests; the runtime compiles and passes its
  tests without it and names no protocol of external agents.
- The binaries list it; external personalities that name `a2a_agent` work as before.
- `docs/ARCHITECTURE.md` and `docs/COMPONENTS.md` say where the kind lives.

## History

- **2026-10-12** — Opened from the discussion of which subjects besides skills could move into
  crates of their own (issue 323).
- **2026-10-12** — Done, as the third plugin. `caelumora-a2a` holds the kind (`agent`, with its
  `client`, `translation` and `wire` as before) and its tests; `A2aPlugin` registers the kind alone
  and is listed by `caelumora-app` among the plugins without routes and by `caelumora-tui`. The
  runtime's `external` keeps the facet, its types and `register_all`, which registers the facet
  without a kind. Decided by the maintainer for the shared remote helpers: `remote` of the runtime
  becomes public, rather than moving to `caelumora-model::remote` - describing the error of an HTTP
  client is the runtime's, and the model needs no dependency on `reqwest` for it; Open Terminal
  and Ollama stay as they are. Nothing else had to be made visible. The paths written out in the
  production code of the kind are imports now. `docs/COMPONENTS.md` names the facet's types and
  `remote` among what a component kind outside the core may use.
