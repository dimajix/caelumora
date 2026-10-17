# 412 — A plan has to fit into the model calls of one turn

**Status:** done
**Type:** feature
**Priority:** medium
**Area:** Context and knowledge
**Summary:** A plan is carried out within one turn, and a turn may ask its model 25 times at most, so a plan of nine steps has fewer than three calls per step - too few for a step that writes a file, finds its mistake and writes it again - and a plan that runs out ends unfinished.

## The problem

`create_plan` sets the turn's plan, and its steps are carried out in the same turn: each step ends
with an answer without tool calls, and the next begins with another model call. All of it counts
against `MAX_MODEL_CALLS` (25, `caelumora-agents/src/turn/mod.rs`), which was set for a turn that
answers one message, before plans existed at that size.

In session `s2` on 2026-10-17, Jonathan's plan of nine steps - translate five Python files to
Node.js, set up the project, run it - used its 25 calls by step 6: a step that writes a file takes
at least two calls (the call and the answer that ends the step), more when the model gets the
arguments wrong (three `write_file` calls without `path` there) or checks what it did. Issue
[411](411-call-limit-ends-turn-silently.md) makes such an end visible; this one is about whether the
plan should have got that far at all.

A model also tends to do the work of several steps in one, so that the plan's progress lags behind
what was done - which matters as soon as the progress decides anything, as a budget per step would.

## Why it matters

Planning (`create_plan_tool`, issues [49](49-plan-step-system-messages.md) and
[263](263-plan-steps-not-in-chat.md)) is for tasks too large for one answer. A limit that a medium
task reaches makes planning useless exactly where it is needed, and the safety net against a
looping model becomes the usual end of a plan.

## Considerations

*   **A higher limit for a turn with a plan.** Simplest: e.g. 25 calls plus a number per step.
    Keeps the safety net, but grows with a plan the model chose itself - a model that loops can
    plan a hundred steps.
*   **A budget per step.** Each step gets its own calls, e.g. 10; a step that uses them up ends,
    and the plan goes on or is given up. Bounds a looping step, not a long plan, and needs the
    progress to be right.
*   **A plan that outlives its turn.** Each step is a turn of its own, the plan kept in the agent's
    state in the channel and the next step begun when the last ended. The limit stays as it is,
    per turn; a restart, a cancel and asking anew then concern a step, not the whole plan. The
    largest change: the plan leaves `Turn`, and what a step is told of the plan, and how the users
    see a plan spread over turns, change with it.
*   **Configurable.** Whatever the rule, its numbers may belong to the personality or to the model
    (a weaker model needs more calls), not to a constant.

To be decided; the third is the structural answer and should be weighed against the cost.

## Done when

*   The decision is recorded here, and a plan of the size of the one in `s2` runs to its end with a
    model that makes the usual mistakes.

## History

- **2026-10-17** — Opened with [411](411-call-limit-ends-turn-silently.md), after Jonathan's plan in
  session `s2` ran out of model calls at step 6 of 9.
- **2026-10-17** — Decided: a budget per step. Each step of a plan, and its final answer, may call
  the model `MAX_MODEL_CALLS` times, as a turn without a plan may - the count begins anew in
  `Turn::advance_plan`; a step that would call it once more gives the turn up as `CallLimit`
  (issue [411](411-call-limit-ends-turn-silently.md)) rather than going on to a step that builds
  on results it does not have. What a plan may cost is bounded by its steps: `create_plan` refuses
  a plan of more than `Plan::MAX_STEPS` (20), those it is extended by included. A plan that
  outlives its turn was not taken: it solves the budget no better and changes much else - worth an
  issue of its own once a plan should outlive its turn for other reasons. The numbers stay
  constants for now; a value of the personality is the next step if one is needed.
