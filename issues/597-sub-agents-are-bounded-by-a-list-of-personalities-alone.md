# 597 — Sub-agents are bounded by a list of personalities alone

**Status:** in progress
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

## What exists, as of 2026-11-25

- **Where the bounds are kept.** The one bound today, the list of personalities, is checked by the
  tool alone (`StartSubagentTool::execute`), on the agent side, which is trusted for nothing
  (issue [516](516-the-runtime-trusts-the-agents-linked-into-it.md)); so is a tool's
  `requires_approval` - the kernel answers the policy and the rules of the place
  (`ActorRuntime::admit_tool_call`). The writer (`SubAgents::start`) checks only that the owner
  finds the personality and that it is internal.
- **Rights.** A sub-agent is an agent of the main agent's owner: it never gets more than the owner,
  but more than the main agent, by its personality's tools and approvals, and by its limiting
  roles. Its tools are the agent side's, where the kernel has no place to intersect them.
- **Depth.** It grows without a bound only where a listed personality carries the component
  `subagents` itself. The whole chain lies in one channel: a thread owns its sub-agent (issue
  598, `Opening::owned`), opened by the agent that started it.

## Decision

Decided by the maintainer on 2026-11-25.

**D1 - The limits are the plugin's.** `plugins.subagents` of the configuration holds `max_open` and
`max_depth`, set by the operator and checked by the writer, on the kernel's side - as
`plugins.artifacts.max_bytes` bounds what is published. By default `max_open: 5`, `max_depth: 1`:
sub-agents start none of their own unless the operator allows it.

**D2 - What is counted.** The depth of a sub-agent is how many threads that own an agent lie above
it, following each owned agent to the thread's opener. `max_open` counts the sub-agents open at
once in a tree - the first agent and all that descend from it -, not per giver, which would
multiply with the depth; one let go and not closed yet does not count, nor do those ended before:
a total over the channel's life would shut a long-lived channel. The count is taken after the
sub-agent's thread is opened, its own included, so that two starting at once never pass the
limit: at worst both are refused.

**D3 - How a limit is told.** `SubAgentError::TooMany` - the model can end one first - is a
parameter error of the tool; `TooDeep` - nothing the model does helps - is a refusal.

**D4 - Rights by the limiting roles (R1), as a step of its own.** A sub-agent's personality is at
least as limited as the main agent: its limiting roles lie within those of the work's context,
checked by the writer as it starts one; a wider one is refused as a parameter error, since another
personality may do. A check as it starts, not a chain of rights - SECURITY.md's "no delegation
chain" stands -; a personality widened later is not followed.

Rejected:

- **Limits on the component** (`max_open`, `max_depth` as its parameters) - the agent side's word.
- **The tools intersected with the main agent's** - no place in the kernel to do it.
- **The main agent's limit inherited by the sub-agent's context** (R2, in `resolved.rs`) - it
  follows a change, but needs the giver wherever a context is built, and turns the check into a
  chain of rights.
- **Leaving the rights as they are** (R0) - a participant could have the main agent start a
  sub-agent its limiting roles do not narrow, and read through it what the main agent may not.

Not in 597: the list of personalities checked by the kernel (a finding of 2026-11-25, see
History).

## Plan

### Step 1 - Depth and number, checked by the plugin

- **`caelumora-domain`, `ChannelState`:** `ownership_depth(actor)` - how many open threads that own
  an agent lie above `actor`, following each owned agent to its thread's opener; `owned_in_tree(
  actor)` - the agents owned by open threads, not let go, whose chain leads to the same first
  opener as `actor`'s.
- **`caelumora-kernel`, `ChannelLoop`:** the same two, read from its state.
- **`caelumora-subagents`:** `SubagentsConfig { max_open, max_depth }`, claimed by the plugin
  (`KernelBuilder::config`) and given to the writer; `Starting` refuses `TooDeep` before it makes
  the agent, and `TooMany` after the thread is opened - undone by its compensation;
  `SubAgentError::TooMany(usize)`, `TooDeep(usize)`, mapped as D3.
- **Tests:** domain tests of both counts; the plugin's tests of both refusals, in the suite's world
  (`World::configured`), and that a sub-agent let go does not count.
- **Documents:** CONFIGURATION.md (`plugins.subagents`), TOOLS.md, PERMISSION-CHECKS.md.

### Step 2 - A sub-agent no wider than its starter

- **`caelumora-subagents`, the writer:** the personality found refused unless its limiting roles
  (`Personality::limiting_roles`) lie within the work's context's (`SecurityContext::
  limiting_roles`); a new `SubAgentError` variant, a parameter error of the tool.
- **Where the comparison lives:** a method of the domain or of the context beside
  `SecurityContext::has_role`, decided at the step's interface review.
- **Tests:** a limited main agent refused a wider personality, given one as limited.
- **Documents:** SECURITY.md (the sentence "open in 558"), PERMISSION-CHECKS.md, TOOLS.md.

### Documents and stored data

Nothing stored changes.

## History

- **2026-11-18** — Opened from issue 558's open points (*Bounding its rights*) and the depth and
  fan-out found on review of step 5b of 557's 2b.
- **2026-11-25** — What exists traced; decided by the maintainer: the limits in `plugins.subagents`
  (`max_open: 5`, `max_depth: 1`), checked by the writer; counted per tree, open and not let go;
  `TooMany` a parameter error, `TooDeep` a refusal; the rights bounded by the limiting roles as
  they start (R1), as step 2. Found: the list of personalities is checked on the agent side alone.
- **2026-11-25** — Step 1 implemented: `ChannelState::ownership_depth` and `owned_in_tree`, on
  `ChannelLoop` too; `SubagentsConfig` (`plugins.subagents`), checked by the writer in `Starting`;
  `SubAgentError::TooMany` and `TooDeep`. The counts follow the actor - each owned agent to its
  thread's opener - instead of the thread first sketched (`delegation_depth(thread)`,
  `delegations_open_in_tree(thread)`): the kernel then counts from the work's caller, which it
  knows, not from the origin the agent side names. On review: `TooMany` refused before anything is
  made as well, so that a full tree leaves no agent deleted and no thread in the history - counted
  again once the thread is opened, for one started meanwhile; its message holds for a sub-agent
  whose siblings fill the tree; `SubagentsConfig` beside the writer that checks it.
