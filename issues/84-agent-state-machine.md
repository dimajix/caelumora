# 84 — The agent's state machine has states and handlers that do nothing of their own

**Status:** open
**Area:** Code structure

## The problem

- `ActorState::Stopping`: `handle_stopping` (`actor_impl/agent.rs:710-713`)
  logs and goes to `Reflecting`; the doc above it ("fall through to Idle")
  disagrees with the code. `handle_executing_tools` (`1014-1021`) reads
  `requested_state()` only to detect `Stopping`.
- `handle_idle` and `handle_sleep` (`559-603`, `666-692`) are the same
  structure; the only difference is whether `ResponseRequested` starts a
  turn. `state_after_sleep` exists to bridge them.
- `respond_partial`/`respond_complete` (`166-198`) are identical except for
  `push_message`, and the choice `internal → IntermediateMessageGenerated,
  else CompleteMessageGenerated` is written four times (`903-907`, `926/936`,
  `958-962`, `989-993`).
- `AgentActor::handle_global_event(_, _tx)` carries an unused parameter;
  `perceive` discards its result (`let _ = perceived`, `154-163`).

## Why it matters

Every state is a branch in `run`, a span, a handler, and a place to get
the resume logic ([75](75-agent-resumes-half-turn.md)) wrong.

## Considerations

- Drop `Stopping`: `requested_state == Some(_)` means "reflect after this
  tool step". `stop_tool` requests `Idle`. Cost: model 3, `agent.rs` 3,
  `stop_tool` 1.
- One `handle_waiting(state)` with `match (state, event)` for `Idle` and
  `Sleeping`; ~35 lines less.
- One `fn event_for(message, partial) -> ActorEvent`.
- Keep `Reflecting` and the order store → announce → `FullResponseGenerated`:
  issue [55](55-notes-by-reflection.md) hangs the reflection call there.

## Done when

- `ActorState` has no state whose handler only forwards.
- The event for a generated message is chosen in one place.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 3.2.6, 3.2.7).
