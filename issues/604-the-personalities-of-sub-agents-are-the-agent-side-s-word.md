# 604 — The personalities a sub-agent may be of are the agent side's word

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Identity and access management
**Summary:** Which personalities an agent may start a sub-agent of is listed on its component `subagents`, but only the tool checks the list, on the agent side, which is trusted for nothing; the kernel's writer starts an agent of any internal personality its owner finds, as long as the limits of issue 597 hold.

## The problem

The component `subagents` (issue [558](558-no-agent-hands-a-task-to-a-sub-agent.md)) is configured
with the personalities its tools may start an agent of (`personalities`). That list is checked in
`StartSubagentTool::execute` (`caelumora-subagents/src/component/tool.rs`), a library of the agent
side, which [PLUGINS.md](../docs/PLUGINS.md) and issue
[516](516-the-runtime-trusts-the-agents-linked-into-it.md) trust for nothing. The writer the plugin
binds on the kernel's side (`SubAgents::start`, `caelumora-subagents/src/writer.rs`) takes any
personality name. It checks the following:

- the owner finds the personality from the channel (`Views::personality_named`);
- it is internal;
- it is limited at least as the starting agent (`SecurityContext::covers_limit`, issue
  [597](597-sub-agents-are-bounded-by-a-list-of-personalities-alone.md));
- the owner may create the agent (`CreateAgent`);
- the depth and number allowed (`plugins.subagents`, issue 597).

A misbehaving library can therefore start a sub-agent of any internal personality the owner reads,
not only those listed.

## Why it matters

The list is what whoever writes the main agent's personality chooses as the sub-agents' tools and
approvals - the one bound of 597 that is not the kernel's. Within the others the harm is bounded:
the owner could make such an agent by hand, and it is cleared no wider than the main agent.

## Considerations

- **The writer reads the list itself.** The kernel knows the work's agent and its personality; the
  writer could find the component `subagents` among its components (and the agent's own) and
  refuse a personality it does not list. The list would then be read twice, by the tool to describe
  itself and by the writer to check.
- **The list moves into the plugin's reach**, e.g. a field the kernel holds for the agent rather
  than a component's parameter.
- **Accepted as is**, as every tool's own configuration - its `requires_approval` too - is the agent
  side's word; then [SECURITY.md](../docs/SECURITY.md) says so where it names the list. Related to
  issue [559](559-labels-of-what-an-agent-writes-are-its-word.md), which asks the same of labels.

## Done when

- It is decided whether the kernel enforces the list; if so, implemented with a test of the refusal
  through the writer, and PERMISSION-CHECKS.md updated.

## History

- **2026-11-25** — Opened from a finding of issue 597: its limits are checked by the writer, but
  the list of personalities stays the tool's alone.
