# 157 — The actor holds turn state and a constant behind a mutex, and reads its state twice

**Status:** done
**Area:** Code structure
**Epic:** [151](151-agent-refactoring-epic.md)
**Summary:** The actor keeps the turn's count of model calls, which a resumed turn loses, wraps a constant in its mutex, and reads its state for others twice alike.

## The problem

- **`loop_count` and `max_iterations`** are two atomics on `AgentActor`, reset when a turn begins
  and counted per model call. They are the turn's: a turn resumed after a crash counts from 0 again,
  so the limit of 25 model calls does not hold across a restart.
- **`AgentActorState`** wraps `AgentChannelState` and an `Arc<AgentStateHandle>` in one mutex; the
  handle is never replaced after `new` and only cloned in the loop.
- **`message_context` and `given_up_turns`** begin alike: load the agent's attributes, require
  `ReadAgentContext`, load the state, map the errors. `GivenUpTurnsError { Forbidden, Failed }` is a
  subset of `MessageContextError`.

## Why it matters

The first is a behavioural gap; the other two are code that exists only because the right place was
not used.

## Considerations

- `TurnState::model_calls`, stored with the turn, and `Turn::count_model_call() -> bool`; the limit
  a constant in `agent_turn.rs`. The stored form gains a field (serde `default`).
- `agent_state` a field of the actor, `state: Mutex<AgentChannelState>`; `AgentActorState` goes.
- `readable_state(caller) -> Result<AgentChannelState, ReadStateError>` for both reads;
  `GivenUpTurnsError` becomes `ReadStateError`, and `MessageContextError` takes it over with `From`.
  The REST mapping stays.

## Done when

- A resumed turn keeps its count of model calls.
- `AgentActorState` is gone, and the agent's state is read for others in one place.

## History

- **2026-09-30** — From the review of the agent actor (`work/2026-09-30-code-review-agent.md`, A5,
  A6, A7), epic [151](151-agent-refactoring-epic.md).
- **2026-09-30** — Done, as considered.
  - `TurnState::model_calls` (serde `default`: a turn stored before counts from 0) is counted by
    `Turn::count_model_call`, against `MAX_MODEL_CALLS` (25) in `agent_turn.rs`. `loop_count` and
    `max_iterations` are gone from the actor.
  - `AgentActor` holds `agent_state: Arc<AgentStateHandle>` and `state: Mutex<AgentChannelState>`;
    `AgentActorState` is gone.
  - `AgentActor::readable_state(caller)` asks `ReadAgentContext` and loads the state for
    `message_context` and `given_up_turns`. `GivenUpTurnsError` is `ReadStateError`, and
    `MessageContextError` takes it over with `From`. For that, `MessageContextError::Failed` no
    longer names the message (it is in the request anyway); both say "What {actor} saw cannot be
    read". The REST mapping is unchanged.
  - Test: a turn stored after 24 model calls and loaded again may make one more, not 26.
