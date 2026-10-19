# 433 — An MCP server's announcement that its tools changed is not heard

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Tools and approvals
**Summary:** An MCP server that announces a change of its tools (`notifications/tools/list_changed`) is not heard, so an agent follows the change only once its list of the server's tools expires.

## The problem

Issue [126](126-mcp-tools-discovered-once.md) has an `mcp_tool` ask its server for its tools again
once what it read is older than a configured time, and after the server did not know a tool that
was called. A change of the tools is followed within that time, not at once.

MCP lets a server announce the change itself: `notifications/tools/list_changed`, sent to a client
that declared it can hear it. Today no connection is held open to hear it: every call, and every
reading of the tools, opens a connection of its own and closes it again (`caelumora-mcp`,
`tool.rs`), which keeps the provider free of any stale-connection state.

## Considerations

- **A connection that listens**: a session held open per component, kept alive and reopened when
  it breaks - the state the stateless design avoids. Whether calls go over it too, or stay on
  connections of their own, is to be decided.
- **In whose name**: a component keeps no security context after it is built
  (`ComponentContext`); a connection held open is opened in someone's name, with a token resolved
  then. Who that is, and what happens when the token expires, decides whether this can be built at
  all as the rules stand.
- **What a notification does**: it only marks the list as outdated
  (`ExpiringCache::invalidate`); the list is read again at the next use, in the name of whoever
  uses it, as now.

## Done when

An agent follows a change its MCP server announces at its next turn, not only once the list expires.

## History

- **2026-10-19** — Opened while working on issue 126, which leaves the notification out: it needs
  a connection that listens, and today every call opens a fresh one.
