# 84 — The agent's state machine has states and handlers that do nothing of their own

**Status:** done
**Area:** Code structure

## The problem

- `ActorState::Stopping`: `handle_stopping` (`actor_impl/agent.rs:710-713`) logs and goes to
  `Reflecting`; the doc above it ("fall through to Idle") disagrees with the code.
  `handle_executing_tools` (`1014-1021`) reads `requested_state()` only to detect `Stopping`.
- `handle_idle` and `handle_sleep` (`559-603`, `666-692`) are the same structure; the only
  difference is whether `ResponseRequested` starts a turn. `state_after_sleep` exists to bridge
  them.
- `respond_partial`/`respond_complete` (`166-198`) are identical except for `push_message`, and the
  choice `internal → IntermediateMessageGenerated, else CompleteMessageGenerated` is written four
  times (`903-907`, `926/936`, `958-962`, `989-993`).
- `AgentActor::handle_global_event(_, _tx)` carries an unused parameter; `perceive` discards its
  result (`let _ = perceived`, `154-163`).

## Why it matters

Every state is a branch in `run`, a span, a handler, and a place to get the resume logic
([75](75-agent-resumes-half-turn.md)) wrong.

## Considerations

- Drop `Stopping`: `requested_state == Some(_)` means "reflect after this tool step". `stop_tool`
  requests `Idle`. Cost: model 3, `agent.rs` 3, `stop_tool` 1.
- One `handle_waiting(state)` with `match (state, event)` for `Idle` and `Sleeping`; ~35 lines less.
- One `fn event_for(message, partial) -> ActorEvent`.
- Keep `Reflecting` and the order store → announce → `FullResponseGenerated`: issue
  [55](55-notes-by-reflection.md) hangs the reflection call there.

## Done when

- `ActorState` has no state whose handler only forwards.
- The event for a generated message is chosen in one place.

## History

- **2026-09-28** — From the structure review (`work/2026-09-28-code-review.md`, 3.2.6, 3.2.7).
- **2026-09-29** — Done.
  - `ActorState::Stopping` and `handle_stopping` are gone. A requested state (`Turn::request_state`,
    set only by the stop tool) now means: the turn ends with this tool step, and reflecting takes
    the state up. `silence` requests `Idle`, `stop`/`bye` request `Sleeping`. This changes `stop`
    and `bye`: they used to let the model answer once more after the tool result before the agent
    fell asleep; now the turn ends right after the call, like `silence` - which is what their
    descriptions say ("no further input", "stop the conversation"); the good-bye is in the message
    that makes the call.
  - `handle_idle` and `handle_sleep` are one `handle_waiting(asleep, …)`; after an event it
    continues in the live state, which also covers a `wakeup()` that arrived while waiting -
    `state_after_sleep` is gone.
  - `event_for(message, partial)` chooses the event in one place (three sites; the two plan sites
    that send a fixed event keep it on purpose). `respond_complete` is `respond_partial` plus
    keeping the message.
  - `handle_global_event` and `handle_waiting_for_approval` lost their unused sender parameter;
    `perceive` no longer clones its input for a result it discards. `Reflecting` and the order store
    → announce → answer are unchanged (55).
