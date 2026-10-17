# 263 — The chat does not show a plan's steps, nor which message belongs to which

**Status:** done
**Area:** User interaction
**Summary:** The chat shows a plan only as the raw arguments of its call and its steps' messages as one block, without saying which message belongs to which step.

## The problem

When an agent makes a plan, the web UI shows the `create_plan` call like any other call: its
arguments as raw JSON, the steps inside them. While the agent works through the plan, everything
its steps say is one block, folded under "works on the plan" (`PlanSteps`, `chatRows`, issue
[258](258-what-was-said-is-lost.md)). Which step a message belongs to is not shown, and the
channel's history could not tell anyway: only the messages are recorded (`internal`, `turn_id`),
not where a step begins.

Only while a step runs does the web UI name it: `PlanStepStarted` sets the step of the answer under
way, which is shown above the chat as "Name · Step 2 of 3" with the step's description (issue
[203](203-concurrent-answers-in-view.md)). `PlanStepFinished` takes it away again, and a reload
cannot restore it. That is probably the display that seems to have been lost.

## Why it matters

A plan is the agent's account of what it is about to do. Without the steps in the chat, a user can
neither follow during the work which step the agent is on, nor see later what each step did and
where it went wrong.

## Considerations

Two things are wanted: the plan itself, shown once it is made (and again when it is extended), and
the messages of the plan grouped by step, each group headed by its step.

**The plan itself** is in the history already: the `create_plan` call is a message said, and its
arguments are the plan. What is missing is how it is shown. Tools declare how their calls are
shown (`ToolPresentation`, issue 165; `shell` exists). A `plan` presentation, which the web UI
renders as the summary with the numbered steps, needs no new event and also shows an extension,
which is a second call. A call whose content is the owner's has its arguments redacted
(`Message::published`); the plan is then not shown, as for any other call.

**Where a step begins** is not recorded anywhere a reader can get at. Three ways to make it known:

- **A. A history entry `PlanStepStarted`.** The channel records the agent's `PlanStepStarted` as an
  entry of its history, as it records `TurnFinished` and `TurnGivenUp` (issue 258,
  `ChannelLoop::append_history`). `Chain<HistoryEntry>::items` returns it, REST and the snapshot
  serve it as a `HistoryItem`, and the web UI starts a group for it, live from the event, after a
  reload from the item. The event would carry its `turn_id` - the turn knows it when it announces
  the step - because agents answering at once interleave their messages (issue 203), so a step
  belongs to an actor's turn, not to what is next to it. The entry carries the description as the
  event does: like the answer text, it is the agent's own wording and carries the channel's
  classification. `PlanStepFinished` needs no entry: a step ends where the next begins, at the
  final answer or at the turn's end.
  Costs: a variant of `HistoryEntry` (its digest, `items`, the `said` match), of the REST
  `HistoryItem` and of the web UI's type. Old clients ignore an unknown item (`chatMessages` returns
  nothing for it).
- **B. Each message names its step.** `Message` gets `plan_step: Option<usize>` (instead of
  `internal`, which would then be `plan_step.is_some()`), stamped where `internal` is stamped now.
  Every message says on its own where it belongs, with no ordering involved. But `Message` is the
  type of the transcript, of what models are given and of every frontend; it grows for a question
  of display. The description is not in the message, so the web UI has to rebuild the plan from
  the `create_plan` calls of the turn, extensions included, and repeat the runtime's rule for which
  index belongs to which step.
- **C. Only the web UI.** A step converges when the model answers without calling a tool
  (`Turn::plan_converged`), so the web UI could split the internal messages there and take the
  descriptions from the `create_plan` calls. No change to the backend - but the web UI would
  repeat the runtime's rules for convergence and for extending a plan (a plan extended after its
  last step starts again at the first new step), and would show something else as soon as one of
  them changes. That is logic the owner should provide.

**How the web UI groups** the messages is a question of its own:
- one block per step, headed "Step 2 of 3 · description", the step under way open and the
  finished ones folded; or
- the block per plan as now, folded, with a heading per step inside it.

In both cases the banner above the chat (issue 203) would either stay, as a short notice of where
the agent stands, or go, since the open group of the current step already shows it.

## Recommendation

A plus the `plan` presentation. A follows the pattern issue 258 set for how a turn ends: the
agent says it, the channel records it, every reader gets the same from the history. B burdens
`Message` with display and still leaves the web UI to rebuild the plan; C puts the runtime's rules
into the web UI.

To be decided: the grouping in the web UI, and whether the banner above the chat stays.

## History

- **2026-10-08** — Decided with the maintainer: A plus the `plan` presentation, one block per step,
  the banner above the chat goes. Done:
  - `ActorEvent::PlanStepStarted` and `PlanStepFinished` carry their `turn_id`. The channel
    records `PlanStepStarted` as `HistoryEntry::PlanStepStarted` before the step's messages
    (`MessageHistory::start_plan_step`); `items` returns it, REST gives it as `plan_step_started`.
    An agent's own history does not record it: nothing reads steps from it.
  - `create_plan` is presented as `ToolPresentation::Plan`; the web UI shows its summary and its
    steps numbered.
  - The web UI turns the event and the item alike into a mark (`planStepMark`) that begins a block
    of its own (`PlanSteps`, `chatRows`), headed by the step; the step a turn is in is open while
    it runs. The banner of issue 203 is gone.
