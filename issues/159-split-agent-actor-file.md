# 159 — `actor_impl/agent.rs` holds four roles in one file

**Status:** done
**Area:** Code structure
**Epic:** [151](151-agent-refactoring-epic.md)
**Summary:** `actor_impl/agent.rs` holds the run loop, the model step, the tool step and the end of a turn in 1100 lines.

## The problem

`actor_impl/agent.rs` has about 1100 lines of production code and 250 of tests. The actor has four
clearly separate roles: the run loop with the waiting states and `cancel`/`stop`/`wakeup`; the model
step (`Planning`); the tool step (approvals, executing, waiting for approval); ending a turn and
reading the agent's state for others.

## Why it matters

Every change to one step is made in a file that holds all of them; the review's goal of putting
functionality in places of its own is met here by files, not by new types.

## Considerations

- `actor_impl/agent/{mod.rs, run.rs, model_step.rs, tool_step.rs, ending.rs}`, each an `impl
  AgentActor` block. No new types.
- Only after [152](152-three-signals-end-a-turn.md) to
  [155](155-approval-admission-on-the-actor.md), which move and shrink what would otherwise be moved
  twice. Whether it is still worth it then is to be decided.

## Done when

- Each step of the cycle is a file of its own, or this issue records why it was not done.

## History

- **2026-09-30** — From the review of the agent actor (`work/2026-09-30-code-review-agent.md`, A10),
  epic [151](151-agent-refactoring-epic.md).
- **2026-10-01** — First step, without splitting the file: what needs no actor is no longer a method
  of it. `prepare_resume` is a free function in `actor_impl/agent.rs`; `apply_stage_processors` and
  `event_for`, which have nothing agent-specific, are in `actor_impl/mod.rs`; `state_span` is
  `logging::agent_span`, next to `envelope_span`. Whether the file is split is still open.
- **2026-10-01** — `stamp` and `label`, which read of the actor only the personality's reasoning
  visibility, are free functions too, taking it as a parameter and the sender from the turn;
  `assemble_context` is inlined into `request_response`, its only caller.
- **2026-10-01** — What the run loop shares with `cancel`, `stop` and `wakeup` - the processing
  state, the turn's cancellation token and the interrupt - is `RunControl` in
  `agent_run_control.rs`, with the rules of cancel and stop and tests of its own, instead of three
  fields of the actor and their handling spread over six places. How the run loop decides the next
  state is unchanged.
- **2026-10-01** — Done, without splitting the file. The tests of other modules moved to them (the
  two appenders, `StateContextProvider`, `Turn`); `execute_tools` is inlined into
  `handle_executing_tools`; the named lifetimes of `Turn` are gone; `stamp` and `label` have tests
  of their own. What is left is about 815 lines of production code with clearly separate parts, and
  the run loop's choice of the next state is not made into a function of its own: it is in one place
  and covered by the channel tests. The file is not split - `agent/{run, model_step, tool_step,
  ending}.rs` would cost a `pub(super)` on most methods for little gain.
