# 154 — The model step mixes stamping, plan mode, streaming and plan progress

**Status:** open
**Area:** Code structure

## The problem

`AgentActor::handle_llm_turn` (about 120 lines) does four things:

- **Stamping:** `sender_name`, `sender_id`, `label` and `internal` are set
  twice, for a partial and a complete message; `internal` a third time
  where it is already set.
- **Plan mode:** whether messages are internal is derived from
  `has_plan()` and `plan().map(|p| p.progress)`, asked three ways.
- **Streaming** with the cancel token - correct, and stays.
- **Plan progress** after an answer without tool calls: a `match` on the
  progress with `unreachable!` for "no plan", `take_plan` on finalizing,
  `PlanStepFinished` plus `advance_plan` on a step, and two fixed events
  where `event_for(message, false)` would derive the same from `internal`.

## Why it matters

Every change to planning or labelling reads through all four. The plan
bookkeeping is turn state and belongs on `Turn`, as
[83](83-turn-logic-on-agent-actor.md) wanted for the rest of it.

## Considerations

- `fn stamp(&self, message, turn, complete)` for sender, label and
  `internal`.
- `Turn::plan_progress() -> Option<PlanProgress>` instead of `has_plan`
  and `plan().map(|p| p.progress)`.
- `Turn::plan_converged(&mut self) -> Vec<ActorEvent>`: on a step, the
  `PlanStepFinished` and what `advance_plan(idx + 1)` returns; on
  finalizing, `take_plan` and nothing; without a plan, nothing. The actor
  sends the answer with `event_for`, then the events, and returns
  `Planning` while a plan runs. The `unreachable!` goes.
- `take_plan` stays for the `create_plan` tool.

## Done when

- `handle_llm_turn` streams and sends; it does not decide plan progress.
- A message is stamped in one place.

## History

- **2026-09-30** — From the review of the agent actor
  (`work/2026-09-30-code-review-agent.md`, A2), epic
  [151](151-agent-refactoring-epic.md).
