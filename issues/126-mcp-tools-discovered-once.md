# 126 — An MCP server's tools are discovered once, when the agent is built

**Status:** done
**Type:** feature
**Priority:** high
**Area:** Tools and approvals
**Summary:** An MCP server's tools are discovered once, when the agent is built, so a replaced or temporarily unreachable server is not followed.

## The problem

`McpToolProvider` (`caelumora-runtime/src/component_impl/mcp_tool.rs`) asks its server for the tool
list once, while the component is built, and keeps it in `cached_tools` for the life of the agent's
actor. A server that is replaced by a newer one - with tools added, removed or changed - is not seen
until the agent is built again. A server that could not be reached at that moment gives the agent no
tools at all, for good ("starting with no tools available").

## Why it matters

MCP servers are deployed and updated independently of the agents that use them. The tools an agent
is offered, and what a call is checked against, should follow the server - within a bounded delay -
not the moment the agent happened to start.

## Considerations

- **The provider owns the list and its caching** (the maintainer's direction, issue
  [100](100-tool-executor-table-and-result.md)): the executor asks its providers each time, so a
  provider that refreshes its cache is followed without further change.
- **When to refresh**: after a time-to-live; when the server announces a change
  (`notifications/tools/list_changed`, which needs a connection that listens - today every call
  opens a fresh one); after a call fails because the tool is unknown to the server; and, for a
  server that was unreachable, on the next use rather than never.
- **Refreshing without blocking**: `tools()` is synchronous and asked per model call; a refresh runs
  in the background and swaps the cached list (`ArcSwap`), serving the old one meanwhile.
- **Duplicates later**: the executor refuses two tools of the same name when the agent is built
  (issue 37); a refreshed list can bring a clash afterwards. The namespacing (`<component>__<tool>`)
  makes that unlikely between servers, but the executor should notice and log it rather than pick
  one silently.
- **What changes in a turn**: a tool that disappears between the model's call and its execution
  answers as `unknown_tool`, which the model can act on; a changed `requires_approval` should apply
  from the next call.

## Done when

An agent sees an MCP server's current tools - including after the server was replaced or was
unreachable at the start - within a configured delay, without being rebuilt.

## History

- **2026-09-29** — Opened with the maintainer while working on issue 100, which was about to fix the
  tool list in the executor for good.
- **2026-10-19** — Decided with the maintainer and done, in three steps. The provider had moved
  meanwhile to `caelumora-mcp/src/tool.rs` (issue 325).
  - **In whose name.** A refresh in the background, as considered above, would have needed the
    security context the component was built in, which a component does not keep
    (`ComponentContext`). So the tools are discovered when they are offered (`offered`, as a turn
    begins), in the name of whoever acts there - never while the component is built, which also
    no longer waits for a server - and again once what was discovered is older than the
    endpoint's new `refresh_secs` (300 s by default), and after a call the server answered with
    `INVALID_PARAMS`, MCP's "unknown tool". A discovery that fails leaves the tools discovered
    before, or without any the tool set unavailable, and is tried again at the next offer. Where
    the discovery reads its credential is tested in the plugin's tests, in the suite's world.
  - **Not in the background.** A turn waits for a discovery that is due; serving the old list
    while a task reads the new one was left out: one round trip every few minutes, against a
    task, its single flight and its own context.
  - **A generic cache.** What is read of a remote side and kept for a time is
    `ExpiringCache<T>` in `caelumora_runtime::remote`; the A2A agent's card is kept in it too,
    no longer read while the component is built and no longer kept for good: read again after
    `refresh_secs`, and after the agent could not be reached where its card said. The Ollama
    client's `RunningContextSizes` keeps a value for a time by itself, keyed by server and model;
    it was left as it is.
  - **Duplicates later.** A clash a changed list brings is found as a turn begins
    (`ToolProviders::tool_sets`): the component configured first keeps the name, a warning names
    both.
  - **`notifications/tools/list_changed`** needs a connection that listens: issue
    [433](433-mcp-tools-list-changed.md).
