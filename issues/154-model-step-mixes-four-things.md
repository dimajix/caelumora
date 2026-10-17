# 154 — The model step mixes stamping, plan mode, streaming and plan progress

**Status:** done
**Area:** Code structure
**Epic:** [151](151-agent-refactoring-epic.md)
**Summary:** The model step mixes stamping messages, plan mode, streaming and plan progress in one function of 120 lines.

## The problem

`AgentActor::handle_llm_turn` (about 120 lines) does four things:

- **Stamping:** `sender_name`, `sender_id`, `label` and `internal` are set twice, for a partial and
  a complete message; `internal` a third time where it is already set.
- **Plan mode:** whether messages are internal is derived from `has_plan()` and `plan().map(|p|
  p.progress)`, asked three ways.
- **Streaming** with the cancel token - correct, and stays.
- **Plan progress** after an answer without tool calls: a `match` on the progress with
  `unreachable!` for "no plan", `take_plan` on finalizing, `PlanStepFinished` plus `advance_plan` on
  a step, and two fixed events where `event_for(message, false)` would derive the same from
  `internal`.

## Why it matters

Every change to planning or labelling reads through all four. The plan bookkeeping is turn state and
belongs on `Turn`, as [83](83-turn-logic-on-agent-actor.md) wanted for the rest of it.

## Considerations

- `fn stamp(&self, message, turn, complete)` for sender, label and `internal`.
- `Turn::plan_progress() -> Option<PlanProgress>` instead of `has_plan` and `plan().map(|p|
  p.progress)`.
- `Turn::plan_converged(&mut self) -> Vec<ActorEvent>`: on a step, the `PlanStepFinished` and what
  `advance_plan(idx + 1)` returns; on finalizing, `take_plan` and nothing; without a plan, nothing.
  The actor sends the answer with `event_for`, then the events, and returns `Planning` while a plan
  runs. The `unreachable!` goes.
- `take_plan` stays for the `create_plan` tool.

## Done when

- `handle_llm_turn` streams and sends; it does not decide plan progress.
- A message is stamped in one place.

## History

- **2026-09-30** — From the review of the agent actor (`work/2026-09-30-code-review-agent.md`, A2),
  epic [151](151-agent-refactoring-epic.md).
- **2026-09-30** — Done.
  - `AgentActor::stamp(message, turn, complete)` sets sender, labels and `internal` for partial and
    complete messages alike; the third `internal` assignment is gone.
  - `Turn::plan_progress()` replaces `has_plan` and every `plan().map(|p| p.progress)`;
    `Turn::in_plan_step()` says whether what the model produces now is internal - a step under way,
    not the final answer.
  - `Turn::plan_converged()` moves the plan on after an answer without tool calls and returns the
    events to announce: `PlanStepFinished` and the next `PlanStepStarted`, or nothing once the final
    answer ends the plan. `handle_llm_turn` sends the answer with `event_for`, then those events,
    and asks the model again while a plan runs. The `match` with `unreachable!` and the two fixed
    events are gone; a step index outside the plan no longer panics.
  - Tests: `plan_converged` on the turn, and - new, the actor's plan flow had none - a channel test
    that runs a two-step plan through approval, both steps and the final answer. It passes against
    the code before this change too: the flow is unchanged.
