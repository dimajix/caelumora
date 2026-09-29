# 100 — `DefaultToolExecutor` scans its providers per call, and a tool result has three representations

**Status:** done
**Area:** Tools & approvals

## The problem

- The executor scans all providers and calls `provider.tools()` on every
  `describe`, `is_permitted` and `execute` (`tool.rs:224-226, 258-260`; for
  `McpToolProvider` each is `cached_tools.clone()`). `request_approvals`
  (`agent.rs:791-808`) scans twice more; `AgentActor::label` calls
  `tool_executor.tools()` per streamed chunk (`agent.rs:271, 900`). `new`
  (`:175-186`) already iterates the tools for the duplicate check.
  `CommandExecutor` (`command.rs:77-160`) keeps a table of `CommandEntry`
  and is the pattern.
- `execute` returns a `ToolCallResult`; the loop takes only `.content`
  (`agent.rs:351`), stores it next to the status in `ToolCallState { result:
  Option<String>, result_status }`, and rebuilds a `ToolCallResult` by hand
  (`agent.rs:381-390`, with `c.result.unwrap()`). `ToolExecution` and
  `ExecutionStatus` (`tool.rs:125-142`) are built into a local `_execution`
  that is never read. `ToolExecutor::providers` has no caller.

## Why it matters

One scan per streamed chunk is work the model's latency pays for; three
shapes for one result are three places to disagree. Issue
[20](20-transactional-effects.md) will give `ToolCallState` a lifecycle - it
should start from one result type.

## Considerations

- `entries: Vec<ToolEntry { tool, component, provider }>` like `CommandEntry`;
  `tools()`, `describe`, `is_permitted`, `execute` look it up. Only `tool.rs`.
  The eight defensive `_ => Err(UnknownTool)` arms in the providers become
  provably unreachable.
- `ToolCallState.result: Option<ToolCallResult>` in the model - the status
  travels with the content, `unwrap` and `result_status` go. Alternative
  with less change: `execute` returns `(ToolResultStatus, String)` and maps
  `ToolError` itself (then the loop need not know `available`).
- Remove `ToolExecution`, `ExecutionStatus`, `ToolExecutor::providers`;
  `successful_result` and friends become private or move into the executor.
- `is_permitted` before approval and the check in `execute` stay both -
  "policy before approval" is deliberate.

## Done when

- The executor holds a table and scans nothing per call.
- A tool result has one type from `execute` to the model's tool message.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 4.2.1, 4.2.2, 4.3 and 3.4 "Tool-Call-Ergebnis").
- **2026-09-29** — Done in part, and in part rejected by the maintainer.
  *Rejected: a table of tools in the executor.* A first version collected
  every provider's tools when the executor was built. The maintainer
  objected that an MCP server can be replaced by a newer one while the
  agent runs; the tool list belongs to the provider, which may cache and
  refresh it. So the executor keeps its providers and asks them each time
  (`tools()`, and the lookup in `describe` and `execute`); a provider makes
  that cheap by caching. The duplicate check at build time stays. Making
  MCP discovery actually dynamic is issue
  [126](126-mcp-tools-discovered-once.md). (Labelling a streamed response
  already asks for the tools once per model call, not per chunk.)
  *Done: one result type.* `ToolExecutor::execute` always answers with a
  `ToolCallResult` - what the tool gave, or the error turned into what the
  model is told (`ToolError::to_result`) - built by
  `ToolCallResult::of(call, status, content)`, which carries the call's id,
  tool and labels. `ToolCallState.result` is `Option<ToolCallResult>`;
  `result_status` is gone, and so are the rebuilding of the result in
  `ToolCalls::collect` and the error handling in the agent's loop, which
  now records what `execute` returns. Removed: `ToolExecution`,
  `ExecutionStatus` and `ToolExecutor::providers`. `is_permitted` before
  approval and the check in `execute` both stay.
