# 83 — `AgentActor` runs the turn; `Turn` is only a view of the state

**Status:** open
**Area:** Code structure

## The problem

Every method of `Turn` is `self.agent_channel_state.<field>`
(`caelumora-runtime/src/agent_turn.rs`). The logic of a turn lives in
`AgentActor`: `execute_tools` (`actor_impl/agent.rs:328-412`, collecting
results, building the tool message), `request_approvals` (`785-839`, creating
`ToolCallState`s), `advance_plan` (`463-488`), `plan_step_message` and
`plan_finalize_message` (`414-453`). Of the actor they need only
`tool_executor`, `agent_id` and the agent's name. The actor calls eight to
ten turn setters per step.

`AgentActorState::turn()` (`agent.rs:68-75`) takes the agent's name from
`turn_event_context.sender_name`; the source is `agent_controller.name()`.

## Why it matters

AGENTS.md names this shape - one object making another act on its behalf -
as the sign that something sits on the wrong object. `Turn` has 45 public
methods because the actor reaches into it from outside, and every new piece
of turn logic lands in a 1400-line file.

## Considerations

- Move what only reads and writes turn state onto `Turn`:
  `Turn::advance_plan`, `Turn::collect_tool_results(...) -> Option<Message>`,
  `Turn::register_tool_calls(states)`, the plan messages. The actor keeps
  I/O: the model call, the executor, `output`. About 250 lines move; no
  external callers.
- Do it after [84](84-agent-state-machine.md) and
  [85](85-turn-state-in-agent-channel-state.md), which reduce what has to move.
- The setters that remain only for the actor become `pub(crate)`
  ([107](107-public-interface.md)); the component API (`set_context`,
  `record_knowledge`, `stage_*`, `extend_*`, plan access, `request_state`,
  `agent_state`, `classification`, `publication`, `security_context`) stays
  `pub` - issue [65](65-generic-component-state.md) builds on it.

## Done when

- `AgentActor` calls no `Turn` setter for tool results, tool call states or
  plan progress; it asks the turn to do it.
- `Turn`'s public surface is the component API plus a `pub(crate)` half for
  the actor.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 3.1.2).
