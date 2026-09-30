# 159 — `actor_impl/agent.rs` holds four roles in one file

**Status:** open
**Area:** Code structure

## The problem

`actor_impl/agent.rs` has about 1100 lines of production code and 250 of
tests. The actor has four clearly separate roles: the run loop with the
waiting states and `cancel`/`stop`/`wakeup`; the model step (`Planning`);
the tool step (approvals, executing, waiting for approval); ending a turn
and reading the agent's state for others.

## Why it matters

Every change to one step is made in a file that holds all of them; the
review's goal of putting functionality in places of its own is met here
by files, not by new types.

## Considerations

- `actor_impl/agent/{mod.rs, run.rs, model_step.rs, tool_step.rs,
  ending.rs}`, each an `impl AgentActor` block. No new types.
- Only after [152](152-three-signals-end-a-turn.md) to
  [155](155-approval-admission-on-the-actor.md), which move and shrink
  what would otherwise be moved twice. Whether it is still worth it then
  is to be decided.

## Done when

- Each step of the cycle is a file of its own, or this issue records why
  it was not done.

## History

- **2026-09-30** — From the review of the agent actor
  (`work/2026-09-30-code-review-agent.md`, A10), epic
  [151](151-agent-refactoring-epic.md).
