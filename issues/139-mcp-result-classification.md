# 139 — An MCP server cannot classify its results one by one

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Tools and approvals
**Summary:** An MCP server cannot classify its results one by one, only be configured with one classification for all of them.

## The problem

Since step 3 of [133](133-classification-model.md), a tool classifies each of its results
(`ToolOutput::classification`), and what may not flow into the agent's channel is withheld from the
agent by the `GuardedToolExecutor`. The built-in tools classify nothing: their results are the
channel's. `mcp_tool` can be configured with one `classification` for every tool of its server, but
the server itself has no say. A server that reaches sources of different sensitivity - a knowledge
base, a document store, a CRM - either has all its results classified as its most sensitive source,
and most of them withheld, or as its least, and the sensitive ones let through.

## Why it matters

- The sources of an organisation are reached more and more through MCP; a classification by server
  is too coarse to be useful for them.
- Labels carried by the data, not by their source, are what allow external sources to respect the
  rules rather than the rules being re-implemented at every source
  ([133](133-classification-model.md), "Why it matters").

## Considerations

- **Where the server says it.** MCP results carry `_meta`, a free-form object; a key such as
  `classification` with `level` and `compartments` would be read by `mcp_tool` into
  `ToolOutput::classification`. Structured content (`structuredContent`) is the tool's own schema
  and not the place.
- **Trust.** A server's classification is believed as the server is: the configured `classification`
  could act as a floor, so that a server can only classify a result higher than configured, never
  lower - or it is believed as it is, like a built-in tool.
- **Unknown compartments.** A compartment the configuration does not know never flows anywhere, so
  the result is withheld - safe, but the server's vocabulary has to match the administrator's.

## Done when

- An MCP server can classify each of its results, and `mcp_tool` passes the classification on; how
  far it is believed is decided and documented.

## History

- **2026-09-30** — Opened while implementing step 3 of [133](133-classification-model.md), which
  gave tools a classification per result and `mcp_tool` a configured one, but left MCP servers
  without a way to say theirs.
