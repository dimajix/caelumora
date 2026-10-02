# 78 — `McpToolConfig.requires_approval` has no effect

**Status:** done
**Area:** Tools & approvals

## The problem

`McpToolConfig` has `requires_approval: bool`
(`caelumora-runtime/src/component_impl/mcp_tool.rs:56`), but `discover_tools` sets
`requires_approval: false` on every discovered `Tool` (`:133`). The configuration value is never
read.

## Why it matters

An MCP tool with side effects runs without approval, whatever the operator configured. Issue
[20](20-transactional-effects.md) will want tools to declare their effects; until then the flag is
the only guard.

## Considerations

Pass the configured value through in `discover_tools`. One line plus a test; the existing test at
`:331` asserts the wrong thing and must change.

## Done when

An MCP component configured with `requires_approval: true` yields tools that require approval.

## History

- **2026-09-28** — Found in the structure review (`work/2026-09-28-code-review.md`, section 1, error
  4).
- **2026-09-29** — Done. `discover_tools` takes the component's `McpToolConfig` and sets
  `requires_approval` on every discovered tool from it. A correction to the description: the unit
  test at `:331` was right - it checks that the flag *defaults* to `false` when parsing; what was
  missing was a test that the flag reaches the tools. The integration test
  `a_configured_approval_reaches_every_discovered_tool` now covers that against a real MCP server,
  and the existing one checks that no approval is required unless configured.
