# 83 — `AgentActor` runs the turn; `Turn` is only a view of the state

**Status:** done
**Area:** Code structure
**Epic:** [74](74-refactoring-epic.md)
**Summary:** The logic of a turn lives in `AgentActor`, which drives `Turn` through eight to ten setters per step.

## The problem

Every method of `Turn` is `self.agent_channel_state.<field>`
(`caelumora-runtime/src/agent_turn.rs`). The logic of a turn lives in `AgentActor`: `execute_tools`
(`actor_impl/agent.rs:328-412`, collecting results, building the tool message), `request_approvals`
(`785-839`, creating `ToolCallState`s), `advance_plan` (`463-488`), `plan_step_message` and
`plan_finalize_message` (`414-453`). Of the actor they need only `tool_executor`, `agent_id` and the
agent's name. The actor calls eight to ten turn setters per step.

`AgentActorState::turn()` (`agent.rs:68-75`) takes the agent's name from
`turn_event_context.sender_name`; the source is `agent_controller.name()`.

## Why it matters

AGENTS.md names this shape - one object making another act on its behalf - as the sign that
something sits on the wrong object. `Turn` has 45 public methods because the actor reaches into it
from outside, and every new piece of turn logic lands in a 1400-line file.

## Considerations

- Move what only reads and writes turn state onto `Turn`: `Turn::advance_plan`,
  `Turn::collect_tool_results(...) -> Option<Message>`, `Turn::register_tool_calls(states)`, the
  plan messages. The actor keeps I/O: the model call, the executor, `output`. About 250 lines move;
  no external callers.
- Do it after [84](84-agent-state-machine.md) and [85](85-turn-state-in-agent-channel-state.md),
  which reduce what has to move.
- The setters that remain only for the actor become `pub(crate)` ([107](107-public-interface.md));
  the component API (`set_context`, `record_knowledge`, `stage_*`, `extend_*`, plan access,
  `request_state`, `agent_state`, `classification`, `publication`, `security_context`) stays `pub` -
  issue [65](65-generic-component-state.md) builds on it.

## Done when

- `AgentActor` calls no `Turn` setter for tool results, tool call states or plan progress; it asks
  the turn to do it.
- `Turn`'s public surface is the component API plus a `pub(crate)` half for the actor.

## History

- **2026-09-28** — From the structure review (`work/2026-09-28-code-review.md`, 3.1.2).
- **2026-09-29** — Done, after [84](84-agent-state-machine.md) and
  [85](85-turn-state-in-agent-channel-state.md). On `Turn` now: `advance_plan(next)` - pushes the
  step or finalize instruction, sets the progress and returns the `PlanStepStarted` to announce,
  instead of sending it itself - with the two plan messages as private functions;
  `register_tool_calls(states)`, which returns the approval requests; `calls_to_run()`, which
  answers denied and timed-out calls and returns the approved ones; `record_tool_result`; and
  `collect_tool_results`, which builds the tool message once every call has its result (without the
  `unwrap` the actor had). The actor keeps the executor calls - it prepares the call states, runs
  the calls, sends what the turn returns - and lost `advance_plan`, `plan_step_message`,
  `plan_finalize_message` and the bookkeeping in `execute_tools` and `request_approvals`.
  `set_tool_calls`, `take_tool_calls` and `last_message` are gone, `set_plan_progress` is private,
  and the half of `Turn` only the actor calls is `pub(crate)`. The `advance_plan` tests moved along
  and need no event sender any more; a new test walks a step's tool calls through registering, a
  denial, a result and the tool message.
- **2026-09-29** — Addendum at the maintainer's request: the tool-call bookkeeping left `Turn`
  again, into a view of its own, `crate::tool_calls::ToolCalls`, which `Turn::tool_calls()` hands
  out for one use at a time (running a call needs the whole turn in between). It borrows the turn's
  working state and the agent's id and name, and has `register`, `decide`, `to_run`, `record`,
  `collect` and `is_empty`; `Turn` lost the seven methods it had for this. The data stays in
  `TurnState::tool_calls`, where it is stored. Considered and not done: a wrapper over `&mut Turn`
  like `AgentNotes` (it would have needed associated functions and a raw accessor for the `Vec`),
  and a model type (the texts the model gets for a denied or timed-out call are the runtime's, issue
  57). The test moved to `tool_calls.rs` and works on a bare `TurnState`.
