# 155 — The actor decides how a tool call is admitted, and builds its state by hand

**Status:** open
**Area:** Code structure

## The problem

`AgentActor::request_approvals` builds every `ToolCallState` as a struct
literal and then patches it (`status = Approved`, `result = Some(..)`), and
it holds the rule for calls that cannot succeed: forbidden by the policy,
an unknown tool or invalid arguments are answered at once; others are
described for the approver, or run without asking if their tool needs no
approval. Everything that rule is made of - `is_permitted`, `describe`,
`ToolError::to_result`, the turn's tools - is in the `GuardedToolExecutor`
([127](127-tool-executor-not-replaceable.md)).

`ToolCalls` (`tool_calls.rs`) borrows the agent's id and name only to
stamp the tool message in `collect`, which `Turn` could do with what it
has.

## Why it matters

How a call is admitted is a decision of the runtime's guard, not of the
actor; the actor should only hand calls in and send the requests that come
back.

## Considerations

- `GuardedToolExecutor::admit(call, turn) -> Admission` with
  `Ask { description }`, `Run` and `Answered(ToolCallResult)`, and
  constructors on `ToolCallState` in the model. `request_approvals` becomes
  a dozen lines. Costs one enum.
- Optional: `ToolCalls::collect` returns the results and `Turn` builds the
  tool message; the view then borrows only the calls and the
  classification. The view itself stays, as decided in 83.

## Done when

- The actor neither constructs nor patches a `ToolCallState`.
- Which calls are asked, run or answered at once is decided in the guard.

## History

- **2026-09-30** — From the review of the agent actor
  (`work/2026-09-30-code-review-agent.md`, A3, T5), epic
  [151](151-agent-refactoring-epic.md).
