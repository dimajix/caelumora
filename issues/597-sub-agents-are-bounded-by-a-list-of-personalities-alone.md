# 597 — Sub-agents are bounded by a list of personalities alone

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Identity and access management
**Summary:** What a sub-agent may is bounded only by the personalities its component lists (issue 558): it acts with its owner's rights, not the main agent's, and nothing bounds how many sub-agents an agent starts or how deep sub-agents start sub-agents of their own.

## The problem

`caelumora-subagents` (issue [558](558-no-agent-hands-a-task-to-a-sub-agent.md)) starts an agent
of one of the personalities its component is configured with; the agent is owned by the work's
owner and made in the channel's scope, and each operation of the kernel checks itself in the work's
context. What 558 left open (*Bounding its rights*):

- **Rights.** A sub-agent's tools and permissions are its personality's and its owner's - possibly
  more than the main agent's own.
- **Fan-out.** Nothing bounds how many sub-agents one agent starts, side by side or one after
  another.
- **Depth.** A sub-agent whose personality offers the tools again starts sub-agents of its own,
  without a limit.

## Considerations

- Rights bounded by the main agent's: the intersection of tools, or the sub-agent's personality
  checked against what the main agent may do.
- Limits configured on the component (`max_open`, `max_depth`), counted from the threads'
  delegations in `ChannelState`.
- Whether a limit is a refusal of the tool (`Forbidden`) or a parameter error the model can act on.

## Done when

- How a sub-agent's rights, the number of sub-agents and their depth are bounded is decided and
  implemented, with tests of the refusals and PERMISSION-CHECKS.md updated.

## History

- **2026-11-18** — Opened from issue 558's open points (*Bounding its rights*) and the depth and
  fan-out found on review of step 5b of 557's 2b.
