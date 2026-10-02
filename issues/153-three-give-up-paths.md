# 153 — A turn is given up along three paths, with two enums for one fact

**Status:** done
**Area:** Code structure

## The problem

`AgentActor::fail_turn`, `give_up_lowered` and the stop branch in `run` do the same: decide the
reason, ask how executed calls are carried (`carried_calls`), `Turn::give_up_turn`, send
`TurnFailed` (except on a stop), warn if sending fails. Around it:

- `TurnFlow { Flows, Lowered(c), NoChannel }` in the actor and `CarriedCalls { InFull, NamesOnly {
  classification } }` in `agent_turn.rs` carry the same classification; `carried_calls` translates
  one into the other, and `GivenUpReason::Lowered { channel }` holds it a third time.
  `give_up_lowered` sets `NamesOnly` itself instead of asking.
- The texts that tell the agent what ran in a turn given up are four free functions in
  `agent_turn.rs` (`executed_calls`, `given_up_because`, `executed_in_full`, `executed_by_name`);
  the last two are the same text but for one sentence. `executed_calls` - which calls among these
  messages ran - is a question the REST API and the web UI ([146](146-show-given-up-turns.md)) will
  ask of a turn given up too.

## Why it matters

A fourth way to end a turn - a branch switch ([08](08-retry-and-edit-past-answers.md))
- would be a fourth copy.

## Considerations

- `CarriedCalls` becomes `Option<Classification>`: `None` in full, `Some(c)` names only, classified
  `c`.
- One `AgentActor::give_up(turn, reason, told: Option<String>, output)` that logs, asks
  `carried_calls`, gives the turn up and sends `TurnFailed` if `told` is set. `fail_turn`, the
  lowering and the stop are calls of it.
- `TurnFlow` keeps three values: `NoChannel` is honest, and `run` needs the difference. Treating it
  as `Flows` and carrying conservatively would save a variant.
- `executed_calls` moves into the model, on `GivenUpTurn` or over `&[Message]` in `transcript.rs`;
  the two texts become one function with `in_full: bool`. The texts stay in the runtime
  ([57](57-prompt-catalogue.md)).
- Best done with or after [152](152-three-signals-end-a-turn.md), which changes the stop branch.

## Done when

- Giving a turn up is one method of the actor, whatever the reason.
- `CarriedCalls` is gone.

## History

- **2026-09-30** — From the review of the agent actor (`work/2026-09-30-code-review-agent.md`, A4,
  T2), epic [151](151-agent-refactoring-epic.md).
- **2026-09-30** — Done.
  - `AgentActor::give_up(turn, reason, output)` is the one way to give a turn up: it asks
    `turn_flow` how executed calls are told, gives the turn up and sends `TurnFailed` with what the
    reason says - the error's user message, or that the channel was lowered - and nothing on a stop.
    `fail_turn` logs and calls it; the lowering and the stop in `run` call it; `give_up_lowered` and
    `carried_calls` are gone. The lowering asks the policy once more instead of passing what the run
    loop found; the answer is the same, and were the channel raised in between, the calls would be
    told in full, as they then may be.
  - `CarriedCalls` is `Option<Classification>` (`names_only`) on `Turn::give_up_turn`. `TurnFlow`
    keeps its three values.
  - `executed_calls` is `caelumora_model::message::executed_calls` over `&[&Message]`, with a test
    of its own; the two texts are one `told_of_executed(reason, executed, in_full)`, still in the
    runtime.
