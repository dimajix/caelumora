# 180 — What an agent sends to an MCP server leaves the system unchecked

**Status:** open
**Area:** Tools and approvals

## The problem

The confidentiality rules decide what may flow *into* a channel: a tool
result is classified and withheld from the agent if it does not flow
(`GuardedToolExecutor`, issue [133](133-classification-model.md)), and an
`mcp_tool` can be configured with a `classification` for what its server
returns. Nothing decides what may flow *out*: the arguments an agent writes
into a call of a remote MCP server are sent as they are. An agent in a
`strictly_confidential` channel can hand what it read there to any server
its personality is wired to, and no rule is asked.

## Why it matters

- The flow rule is only half applied: information is guarded on the way
  in and free on the way out. The sources an organisation reaches through
  MCP are also destinations.
- Issue [179](179-external-agents-via-a2a.md) decides the same question
  for external agents: the roles of an agent's personality limit what it
  is cleared for, asked as `JoinChannel` before it is let into a channel.
  An MCP server is a remote side of the same kind, and should be judged by
  the same measure rather than by a second one.

## Considerations

- **Join, not per call.** As for external agents (179): a tool whose
  remote side is not cleared for the channel is not offered in it, rather
  than every call being checked for what the model put into the arguments.
  Sufficient for the same reason - nothing in the channel is classified
  above it - and the model is not offered a tool it may not use, which
  issue [32](32-unavailable-tools-invisible-to-model.md) asks for anyway.
  Where that is asked: when the agent's tools are assembled for a turn
  (`GuardedToolExecutor`), with the channel the turn began in.
- **Per agent or per tool.** 179 gives every personality `roles` that
  limit its agents at the door - internal ones too. That already covers an
  agent with MCP tools, coarsely: the whole agent stays out of a channel
  its personality is not cleared for. Finer would be per tool - offered or
  not in a channel - which needs a clearance per remote side, read without
  building the component (179 rejected roles on every
  `ComponentDescriptor` and roles in a kind's `params` for that reason).
  Whether the coarse rule is enough is the first question here.
- **Not only MCP.** Any component that talks to the outside is a
  destination: a web search, a mail tool. The check belongs where tools
  are offered, not in `mcp_tool`.
- **Reclassification.** Raising a channel ends nothing for tools today; a
  turn running when its channel is raised keeps the tools it was offered.
  Whether that is acceptable, or the tool list is re-judged per step as the
  turn's flow is (issue 141), is to be decided.

## Done when

- A tool whose remote side is not cleared for the channel is not offered
  to the agent there, and the agent is told so (issue 32).
- The rule is one with the external agent's (179), documented in
  CONFIDENTIALITY.md.

## History

- **2026-10-02** — Opened while deciding the confidentiality of external
  agents (issue 179), which made visible that MCP arguments leave the
  system with no rule asked.
