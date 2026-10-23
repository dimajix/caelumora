# 246 — An MCP server's hints on its tools are ignored

**Status:** done
**Area:** Tools and approvals
**Summary:** An MCP server's hints on its tools - read-only, destructive - are ignored, so all tools of a server need approval alike.

## The problem

Whether a call of an MCP tool needs approval is set per server: `requires_approval` of the
`mcp_tool` component, `true` unless configured (issue
[150](150-impersonation-towards-the-agent.md)). Every tool of a server asks alike - the one that
lists files as much as the one that deletes them. MCP lets a server say more of each tool, in its
annotations: `readOnlyHint` (it changes nothing), `destructiveHint` (what it changes may be lost),
`idempotentHint` and `openWorldHint`. The `mcp_tool` component reads none of them.

Issue [13](13-approval-modes.md) made this one of its points - the hints may set a tool's default,
the configuration keeps the last word - but none of its steps took it up, and 13 is done.

## Why it matters

A server-wide `true` makes every reading call ask, which is the approval fatigue issue 13 was
opened against; a server-wide `false` lets the destructive tools of the same server run unasked. A
person's `Allow` rule relaxes one tool at a time (issue 13), but each person has to find out which
tools are harmless themselves, when the server already says so.

## Considerations

- **The hints are untrusted.** They come from the server, which may lie or be wrong; MCP itself
  says a client must not decide on them alone for a server it does not trust. So they can only set
  a default within what the configuration allows, never override it.
- **The configuration must be able to say "trust the hints".** With `requires_approval` a bool
  defaulting to `true`, there is nothing left for a hint to decide: `true` asks for everything,
  `false` for nothing. Options:
  - **(a)** A third value, e.g. `requires_approval: hints` - a tool the server calls read-only does
    not ask, every other one does; `true` and `false` keep their meaning, and the default stays
    `true`.
  - **(b)** Hints only ever tighten: under `false`, a tool marked destructive asks anyway. Cheap,
    and needs no trust in the server at all, since a lie can only make it ask more; but it leaves
    the case of 13 - relaxing reading tools - unsolved.
  - **(c)** Both: `hints` as in (a), and under `false` a destructive tool asks as in (b).
- **Which hint counts how.** `readOnlyHint` relaxes; `destructiveHint` tightens (MCP's default for
  it is `true` where the tool is not read-only, so a tool without annotations counts as
  destructive). `idempotentHint` and `openWorldHint` do not bear on approval; `openWorldHint` may
  matter for the confidentiality of what leaves the system (issue
  [180](180-outbound-flow-unchecked.md)), not here.
- **Per tool, not per server.** `Tool::requires_approval` is already per tool; the component sets
  it from the server's setting for every tool alike today. It would set it per tool from the
  setting and the annotations when it lists the tools. A rule of a place (`Ask`, `Deny`, issue 13)
  still overrides it, and a person's `Allow` still relaxes it.
- **Shown where the tools are shown.** The tools panel (issue [194](194-tool-sets.md)) marks a tool
  that needs approval; where that came from a hint, it could say so.

## Done when

The `mcp_tool` component can be configured to take an MCP server's annotations into account for
whether a tool's calls need approval, within what the configuration allows, and the default stays
that every call asks.

## History

- **2026-10-06** — Opened when issue [13](13-approval-modes.md) was closed: its plan made the hints
  one of its points, but no step took them up.
- **2026-10-07** — Done with option (a), decided by the maintainer: `requires_approval` of the
  `mcp_tool` component takes `true` (the default), `false` or `"unless_read_only"` (`Approval`);
  with the third, a tool the server marks `readOnlyHint` needs no approval, every other one does -
  one without the mark too, as MCP says. Only `readOnlyHint` counts: `destructiveHint` would matter
  only for (b), which was left out, since under `false` the configuration keeps the last word and
  (b) can be added later without changing (a). `discover_tools` sets `Tool::requires_approval` per
  tool from the setting and the annotations; the rules of a place and of a person apply on top as
  before. The tools panel shows the result per tool as it did; it does not say that a hint decided
  it, which would have needed a field of `Tool` for no use beyond the panel.
